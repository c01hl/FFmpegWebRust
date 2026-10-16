/**
 * 任务日志查看器（对应 ui-spec §3.2 与参考实现 `Components/Shared/LogViewer.razor`）。
 *
 * 与参考实现的**有意差异**：日志不再由父组件传入整段字符串，而是本组件自己负责取数：
 * 1. 挂载时用 `GET /api/tasks/{id}/log?tailLines=200` 拉最近 200 行（契约 §1.5，重连补齐错过的行）；
 * 2. 订阅全局 WS 事件总线的 `task.log` 事件，把服务端已经合并过的增量行追加上去
 *    —— **绝不自己开 WebSocket**，连接由 AppShell 持有的单例负责（`api/ws.ts`）；
 * 3. 本地缓冲有上限（默认 800 行，与 `store/queue.ts` 的 MAX_LOG_LINES_PER_TASK 一致），
 *    超出后丢弃最早的行。
 *
 * 因为日志面板是「展开才渲染」，挂载即拉取、收起即释放，不需要参考实现里那套
 * 「最多缓存 12 个任务日志」的回收逻辑。
 *
 * 与参考实现一致的行为（逐字文案见 ui-spec §3.2）：搜索（250ms 防抖 + `<mark>` 高亮）、
 * 级别筛选（全部 / 仅错误 / 仅警告）、复制全部、下载、清空显示（可恢复）、自动滚动
 * （用户上滚超过 32px 后停止跟随，并出现「回到底部」）、行号 + 行着色 + 页脚统计。
 */

import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import type { CSSProperties } from 'react'
import { getErrorMessage } from '../../api/client'
import { tasksApi } from '../../api/endpoints'
import type { Id } from '../../api/types'
import { onWsEvent } from '../../api/ws'
import { COPY_FAILURE_MESSAGES, copyTextDetailed } from '../../lib/clipboard'
import { formatFileStamp } from '../../lib/format'
import { MAX_LOG_LINES_PER_TASK } from '../../store/queue'
import { toast } from '../../store/toast'
import { Button } from '../ui/Button'
import { IconButton } from '../ui/IconButton'

/** 搜索输入防抖时长（毫秒，ui-spec §3.2） */
const SEARCH_DEBOUNCE_MS = 250
/** 复制成功的 ✓ 图标持续时间（毫秒） */
const COPIED_FEEDBACK_MS = 1_500
/** 自动滚动的「贴底」阈值（像素） */
const STICK_THRESHOLD_PX = 32

/** 日志级别筛选（参考实现的 `LogFilter` 枚举，这里用字符串联合） */
export type LogLevelFilter = 'all' | 'errors' | 'warnings'

/** 一条带原始行号的日志行 */
interface LogLine {
  /** 1 起的行号（相对当前缓冲，不是文件里的绝对行号） */
  number: number
  text: string
}

/** 行着色结果（样式类名与参考实现一致） */
type LogLineKind = '' | 'error' | 'warning' | 'progress'

/** 错误行判定：按单词边界匹配，避免把文件名里的 error/failed 也染色 */
const ERROR_PATTERN = /\b(error|errors|failed|failure|invalid|fatal|unable|cannot|not found|no such file)\b/i
/** 警告行判定：同样按单词边界匹配 */
const WARNING_PATTERN = /\b(warning|warnings|warn|deprecated)\b/i

export interface LogViewerProps {
  /** 任务 id（日志端点与 WS 事件都按它过滤） */
  taskId: Id
  /** 标题，默认 `FFmpeg 日志` */
  title?: string
  /** 日志区最大高度（CSS 长度），默认 `320px` */
  maxHeight?: string
  /** 本地最多保留的行数，默认 800（与 store 一致） */
  maxLines?: number
  /** 首次拉取的行数（`tailLines`），默认 200 */
  tailLines?: number
  className?: string
}

/* ── 纯函数辅助 ────────────────────────────────────────────────────────── */

/** 整段日志文本 → 行数组（丢弃空行、去掉行尾 \r） */
function splitLines(text: string | null | undefined): string[] {
  if (!text) return []
  return text
    .replace(/\r\n/g, '\n')
    .split('\n')
    .map((line) => line.replace(/\r+$/, ''))
    .filter((line) => line.length > 0)
}

/** 只保留最后 maxLines 行 */
function trimLines(lines: string[], maxLines: number): string[] {
  const limit = Math.max(1, maxLines)
  return lines.length > limit ? lines.slice(lines.length - limit) : lines
}

/**
 * 把「首次拉取完成前收到的实时行」接到快照后面，并去掉两边重复的那一段。
 *
 * WS 推送与 REST 快照之间有一个很窄的重叠窗口：如果某些行在服务端已经写进
 * 快照里、同时又通过 WS 推了过来，这里用「快照尾部 == 待追加前缀」的重叠长度
 * 把它们去掉，避免日志里出现重复行。
 */
function mergePendingLines(snapshot: string[], pending: string[]): string[] {
  if (pending.length === 0) return snapshot
  if (snapshot.length === 0) return pending

  const maxOverlap = Math.min(snapshot.length, pending.length)
  for (let size = maxOverlap; size > 0; size -= 1) {
    let matched = true
    for (let index = 0; index < size; index += 1) {
      if (snapshot[snapshot.length - size + index] !== pending[index]) {
        matched = false
        break
      }
    }
    if (matched) return [...snapshot, ...pending.slice(size)]
  }
  return [...snapshot, ...pending]
}

/** 行着色（与 LogViewer.razor 的 GetLineClass 一致） */
function classifyLine(line: string): LogLineKind {
  if (line.length === 0) return ''
  if (ERROR_PATTERN.test(line)) return 'error'
  if (WARNING_PATTERN.test(line)) return 'warning'
  if (line.includes('time=') || line.includes('speed=')) return 'progress'
  return ''
}

/** 把一行按搜索词切成「命中 / 未命中」的片段（搜索是字面量、不区分大小写） */
function highlightSegments(text: string, query: string): Array<{ text: string; hit: boolean }> {
  if (query.length === 0) return [{ text, hit: false }]

  const segments: Array<{ text: string; hit: boolean }> = []
  const lowerText = text.toLowerCase()
  const lowerQuery = query.toLowerCase()
  let cursor = 0

  while (cursor < text.length) {
    const hit = lowerText.indexOf(lowerQuery, cursor)
    if (hit < 0) {
      segments.push({ text: text.slice(cursor), hit: false })
      break
    }
    if (hit > cursor) segments.push({ text: text.slice(cursor, hit), hit: false })
    segments.push({ text: text.slice(hit, hit + query.length), hit: true })
    cursor = hit + query.length
  }
  return segments
}

/* ── 样式常量（颜色一律走设计令牌，禁止写死颜色）───────────────────────── */

const LINE_STYLE: Record<LogLineKind, CSSProperties> = {
  '': {},
  error: { background: 'var(--danger-bg)', color: 'var(--danger-text)' },
  warning: { background: 'var(--warning-bg)', color: 'var(--warning-text)' },
  progress: { color: 'var(--info-text)' },
}

const MARK_STYLE: CSSProperties = {
  background: 'var(--warning-bg)',
  color: 'var(--warning-text)',
  padding: '0 2px',
  borderRadius: '2px',
}

const HEADER_STYLE: CSSProperties = {
  background: 'var(--surface-3)',
  borderBottom: '1px solid var(--border-color)',
}

const FOOTER_STYLE: CSSProperties = {
  background: 'var(--surface-3)',
  borderTop: '1px solid var(--border-color)',
  fontSize: '0.78rem',
  color: 'var(--text-muted)',
}

/**
 * 单任务日志查看器（页内嵌入式）。父组件只要给 `taskId` 即可
 * （日志面板收起时**不要**继续挂载，挂载即拉取）。
 */
export function LogViewer({
  taskId,
  title,
  maxHeight = '320px',
  maxLines = MAX_LOG_LINES_PER_TASK,
  tailLines = 200,
  className,
}: LogViewerProps) {
  const titleId = useId()
  const searchId = useId()
  const filterId = useId()

  /**
   * 日志缓冲。连 taskId 一起存：这样同一个组件实例换了任务时（父组件没有重新挂载），
   * 旧任务的日志不会被误当成新任务的，也不会在 effect 里同步 setState。
   */
  const [buffer, setBuffer] = useState<{ taskId: Id; lines: string[]; dropped: number; truncated: boolean }>(
    () => ({ taskId, lines: [], dropped: 0, truncated: false }),
  )
  // 记忆化：`buffer` 稳定时 `lines` 的引用也稳定，下面的 useMemo/useCallback 才不会每次渲染都失效
  const lines = useMemo(() => (buffer.taskId === taskId ? buffer.lines : []), [buffer, taskId])

  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [reloadToken, setReloadToken] = useState(0)

  const [searchText, setSearchText] = useState('')
  const [appliedSearch, setAppliedSearch] = useState('')
  const [level, setLevel] = useState<LogLevelFilter>('all')

  /** 清空显示（与 taskId 一起存，换任务后自动恢复显示） */
  const [cleared, setCleared] = useState<{ taskId: Id; on: boolean }>({ taskId, on: false })
  const isCleared = cleared.taskId === taskId && cleared.on

  const [copied, setCopied] = useState(false)

  /** 「贴底」状态：事件回调里改，effect 里只读，避免在 effect 同步阶段 setState */
  const stickRef = useRef(true)
  const [stick, setStick] = useState(true)

  const contentRef = useRef<HTMLDivElement | null>(null)
  const readyRef = useRef(false)
  const pendingRef = useRef<string[]>([])
  const copieTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  /* ── 1. 首次拉取（挂载 / taskId / tailLines / 手动重试时）────────────── */

  useEffect(() => {
    let cancelled = false
    readyRef.current = false
    pendingRef.current = []

    void (async () => {
      try {
        const result = await tasksApi.log(taskId, tailLines)
        if (cancelled) return
        const snapshot = trimLines(splitLines(result?.logOutput), maxLines)
        const pending = pendingRef.current
        pendingRef.current = []
        setBuffer({
          taskId,
          lines: trimLines(mergePendingLines(snapshot, pending), maxLines),
          dropped: 0,
          truncated: false,
        })
        setLoadError(null)
      } catch (error) {
        if (cancelled) return
        // 拉取失败不能让用户看到空面板却不知情：保留错误条 + 重试按钮
        setLoadError(getErrorMessage(error))
        const pending = pendingRef.current
        pendingRef.current = []
        setBuffer((previous) => ({
          taskId,
          lines: trimLines([...(previous.taskId === taskId ? previous.lines : []), ...pending], maxLines),
          dropped: previous.taskId === taskId ? previous.dropped : 0,
          truncated: previous.taskId === taskId ? previous.truncated : false,
        }))
      } finally {
        if (!cancelled) {
          readyRef.current = true
          setLoading(false)
        }
      }
    })()

    return () => {
      cancelled = true
    }
  }, [taskId, tailLines, maxLines, reloadToken])

  /* ── 2. 实时增量：只用共享事件总线，不开自己的 WebSocket ───────────── */

  useEffect(() => {
    return onWsEvent('task.log', (event) => {
      if (event.data.taskId !== taskId) return
      const incoming = event.data.lines ?? []
      const dropped = event.data.dropped ?? 0
      const truncated = Boolean(event.data.truncated)
      if (incoming.length === 0 && dropped === 0 && !truncated) return

      // 快照还没回来：先攒着，等快照落地时再合并（见 mergePendingLines）
      if (!readyRef.current) {
        if (incoming.length > 0) pendingRef.current.push(...incoming)
        return
      }

      setBuffer((previous) => {
        const base = previous.taskId === taskId ? previous.lines : []
        return {
          taskId,
          lines: trimLines(base.length > 0 ? [...base, ...incoming] : incoming, maxLines),
          dropped: (previous.taskId === taskId ? previous.dropped : 0) + dropped,
          truncated: (previous.taskId === taskId ? previous.truncated : false) || truncated,
        }
      })
    })
  }, [taskId, maxLines])

  /* ── 3. 搜索防抖 ──────────────────────────────────────────────────── */

  useEffect(() => {
    const timer = setTimeout(() => setAppliedSearch(searchText), SEARCH_DEBOUNCE_MS)
    return () => clearTimeout(timer)
  }, [searchText])

  /* ── 4. 过滤（级别 + 搜索词）───────────────────────────────────────── */

  const visibleLines = useMemo<LogLine[]>(() => {
    if (isCleared) return []
    const query = appliedSearch.trim()
    const result: LogLine[] = []
    for (let index = 0; index < lines.length; index += 1) {
      const text = lines[index]
      if (level !== 'all') {
        const kind = classifyLine(text)
        if (level === 'errors' && kind !== 'error') continue
        if (level === 'warnings' && kind !== 'warning') continue
      }
      if (query.length > 0 && !text.toLowerCase().includes(query.toLowerCase())) continue
      result.push({ number: index + 1, text })
    }
    return result
  }, [lines, level, appliedSearch, isCleared])

  /* ── 5. 自动滚动：只在「贴底」时跟随；行数变化才动 DOM ─────────────── */

  useEffect(() => {
    if (!stickRef.current) return
    const element = contentRef.current
    if (!element) return
    element.scrollTop = element.scrollHeight
  }, [visibleLines.length, isCleared, taskId])

  const handleScroll = useCallback(() => {
    const element = contentRef.current
    if (!element) return
    const distance = element.scrollHeight - element.scrollTop - element.clientHeight
    const next = distance <= STICK_THRESHOLD_PX
    if (next !== stickRef.current) {
      stickRef.current = next
      setStick(next)
    }
  }, [])

  const scrollToBottom = useCallback(() => {
    const element = contentRef.current
    if (element) element.scrollTop = element.scrollHeight
    stickRef.current = true
    setStick(true)
  }, [])

  useEffect(() => {
    return () => {
      if (copieTimerRef.current !== null) clearTimeout(copieTimerRef.current)
    }
  }, [])

  /* ── 6. 操作 ──────────────────────────────────────────────────────── */

  const copyVisible = useCallback(async () => {
    const text = visibleLines.map((line) => line.text).join('\n')
    if (text.length === 0) {
      toast.warning('没有可复制的日志。')
      return
    }

    const result = await copyTextDetailed(text)
    if (!result.ok) {
      toast.error(
        result.message
          ? `复制失败：${result.message}。可手动选中文本复制。`
          : COPY_FAILURE_MESSAGES.log,
      )
      return
    }

    toast.success('日志已复制到剪贴板。')
    setCopied(true)
    if (copieTimerRef.current !== null) clearTimeout(copieTimerRef.current)
    copieTimerRef.current = setTimeout(() => setCopied(false), COPIED_FEEDBACK_MS)
  }, [visibleLines])

  const downloadLog = useCallback(() => {
    if (lines.length === 0) {
      toast.warning('暂无日志可下载。')
      return
    }

    const fileName = `ffmpeg-log-${formatFileStamp(new Date())}.txt`
    try {
      const blob = new Blob([lines.join('\r\n')], { type: 'text/plain;charset=utf-8' })
      const url = URL.createObjectURL(blob)
      const anchor = document.createElement('a')
      anchor.href = url
      anchor.download = fileName
      document.body.appendChild(anchor)
      anchor.click()
      anchor.remove()
      URL.revokeObjectURL(url)
      toast.success(`日志已保存为 ${fileName}`)
    } catch (error) {
      toast.error(`日志下载失败：${getErrorMessage(error)}`)
    }
  }, [lines])

  const clearShown = useCallback(() => {
    setCleared({ taskId, on: true })
    stickRef.current = true
    setStick(true)
  }, [taskId])

  const restoreShown = useCallback(() => {
    setCleared({ taskId, on: false })
  }, [taskId])

  const titleText = title && title.trim().length > 0 ? title : 'FFmpeg 日志'
  const appliedQuery = appliedSearch.trim()
  const filtered = appliedQuery.length > 0 || level !== 'all'

  return (
    <div
      className={className}
      style={{
        display: 'flex',
        flexDirection: 'column',
        border: '1px solid var(--border-color)',
        borderRadius: 'var(--radius-md)',
        overflow: 'hidden',
        background: 'var(--code-bg)',
        color: 'var(--text-primary)',
        fontSize: '0.85rem',
      }}
    >
      <div className="flex flex-wrap items-center justify-between gap-2 px-3 py-1.5" style={HEADER_STYLE}>
        <span id={titleId} style={{ fontWeight: 500 }}>
          <span aria-hidden="true">📜</span> {titleText}
        </span>

        <div className="flex flex-wrap items-center gap-1.5">
          <label className="visually-hidden" htmlFor={searchId}>
            搜索日志
          </label>
          <input
            id={searchId}
            type="search"
            placeholder="搜索..."
            value={searchText}
            onChange={(event) => setSearchText(event.target.value)}
            style={{ width: '150px', padding: '0.25rem 0.5rem', fontSize: '0.8rem' }}
          />

          <label className="visually-hidden" htmlFor={filterId}>
            日志级别筛选
          </label>
          <select
            id={filterId}
            aria-label="日志级别筛选"
            value={level}
            onChange={(event) => setLevel(event.target.value as LogLevelFilter)}
            style={{ width: 'auto', padding: '0.25rem 0.4rem', fontSize: '0.8rem' }}
          >
            <option value="all">全部</option>
            <option value="errors">仅错误</option>
            <option value="warnings">仅警告</option>
          </select>

          <IconButton ariaLabel="复制日志" onClick={() => void copyVisible()}>
            {copied ? '✓' : '📋'}
          </IconButton>
          <IconButton ariaLabel="下载日志" onClick={downloadLog}>
            ⬇️
          </IconButton>
          <IconButton ariaLabel="清空显示的日志" title="清空" onClick={clearShown}>
            🗑️
          </IconButton>

          <label className="flex items-center gap-1 whitespace-nowrap text-xs" style={{ color: 'var(--text-secondary)' }}>
            <input
              type="checkbox"
              checked={stick}
              onChange={(event) => {
                stickRef.current = event.target.checked
                setStick(event.target.checked)
                if (event.target.checked) scrollToBottom()
              }}
            />
            <span>自动滚动</span>
          </label>
        </div>
      </div>

      {loadError !== null ? (
        <div className="mx-3 mt-2">
          <div className="ffui-alert ffui-alert-warning flex-wrap items-center justify-between" role="status">
            <span>读取日志失败：{loadError}</span>
            <Button size="sm" onClick={() => setReloadToken((value) => value + 1)}>
              重试
            </Button>
          </div>
        </div>
      ) : null}

      <div style={{ position: 'relative' }}>
        <div
          ref={contentRef}
          onScroll={handleScroll}
          role="log"
          aria-live="off"
          aria-labelledby={titleId}
          className="overflow-y-auto py-2"
          style={{ background: 'var(--code-bg)', maxHeight, fontFamily: 'var(--font-mono)' }}
        >
          {loading && lines.length === 0 ? (
            <div className="ffui-loading-row">
              <span className="spinner" aria-hidden="true" /> 正在读取日志…
            </div>
          ) : isCleared ? (
            <div className="flex flex-wrap items-center justify-center gap-2.5 px-4 py-8 text-center" style={{ color: 'var(--text-muted)' }}>
              <span>日志已清空（新日志仍会继续显示）</span>
              <Button size="sm" onClick={restoreShown}>
                恢复显示
              </Button>
            </div>
          ) : visibleLines.length === 0 ? (
            <div className="flex flex-wrap items-center justify-center gap-2.5 px-4 py-8 text-center" style={{ color: 'var(--text-muted)' }}>
              {lines.length === 0 ? '暂无日志输出' : '没有符合当前搜索/筛选条件的日志行'}
            </div>
          ) : (
            visibleLines.map((line) => {
              const kind = classifyLine(line.text)
              // 行着色走内联样式（颜色全在 LINE_STYLE 里走设计令牌）
              return (
                <div key={line.number} className="flex px-3 py-0.5" style={{ lineHeight: 1.5, ...LINE_STYLE[kind] }}>
                  <span
                    aria-hidden="true"
                    className="mr-3 inline-block min-w-[44px] text-right select-none"
                    style={{ color: kind === '' || kind === 'progress' ? 'var(--text-muted)' : undefined }}
                  >
                    {line.number}
                  </span>
                  <span className="flex-1 break-all">
                    {highlightSegments(line.text, appliedQuery).map((segment, index) =>
                      segment.hit ? (
                        // 高亮的片段只来自同一行的字面量切片，不会跨行，用下标作 key 是稳定的
                        <mark key={index} style={MARK_STYLE}>
                          {segment.text}
                        </mark>
                      ) : (
                        segment.text
                      ),
                    )}
                  </span>
                </div>
              )
            })
          )}
        </div>

        {!stick && !isCleared && visibleLines.length > 0 ? (
          <div style={{ position: 'absolute', right: '0.9rem', bottom: '0.6rem' }}>
            <Button size="sm" onClick={scrollToBottom} title="回到底部">
              回到底部
            </Button>
          </div>
        ) : null}
      </div>

      <div className="flex flex-wrap gap-1.5 px-3 py-1" style={FOOTER_STYLE}>
        <span>共 {lines.length} 行</span>
        {!isCleared && visibleLines.length !== lines.length ? <span>· 显示 {visibleLines.length} 行</span> : null}
        {filtered ? <span>· 匹配 {visibleLines.length} 行</span> : null}
        {isCleared ? <span>· 已清空显示</span> : null}
        {buffer.dropped > 0 ? <span>· 服务端丢弃 {buffer.dropped} 行</span> : null}
        {buffer.truncated ? <span>· 日志已截断</span> : null}
      </div>
    </div>
  )
}
