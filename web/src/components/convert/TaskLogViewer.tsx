/**
 * 任务日志查看器（ui-spec §3.2 `LogViewer` 的页内嵌入版，Convert / Batch 共用）。
 *
 * 关键点（对应规格与 §10.4 的性能要求）：
 * - 日志行来自全局 store 的**有界缓冲**：每任务最多 `MAX_LOG_LINES_PER_TASK`（800）行，
 *   服务端已按 100–200ms 合并推送，组件只订阅自己那一份切片，避免整页重渲染；
 * - 首屏/重连后若缓冲为空，用 `GET /api/tasks/{id}/log?tailLines=200` 补齐；
 * - 渲染窗口再二次截断（最多 `RENDER_WINDOW` 行），保证长日志不会拖慢页面；
 * - 自动滚动贴底阈值 32px；用户手动上滚后不再强制拉回底部；
 * - 搜索 250ms 防抖、字面量匹配（不区分大小写）并高亮，级别筛选 全部/仅错误/仅警告。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { getErrorMessage } from '../../api/client'
import { tasksApi } from '../../api/endpoints'
import { COPY_FAILURE_MESSAGES, copyAndToast } from '../../lib/clipboard'
import { selectTaskLog, useQueueStore } from '../../store/queue'
import { toast } from '../../store/toast'
import { Button } from '../ui/Button'
import { IconButton } from '../ui/IconButton'

/** 一次最多渲染的行数（内存里仍保留 800 行，只是不全部进 DOM） */
export const RENDER_WINDOW = 300
/** 搜索防抖（毫秒） */
export const LOG_SEARCH_DEBOUNCE_MS = 250
/** 贴底判定阈值（像素） */
const STICK_THRESHOLD = 32

const ERROR_PATTERN = /\b(errors?|failed|failure|invalid|fatal|unable|cannot|not found|no such file)\b/i
const WARNING_PATTERN = /\b(warnings?|warn|deprecated)\b/i
const PROGRESS_PATTERN = /time=|speed=/i

export type LogLevelFilter = 'all' | 'error' | 'warning'

export interface TaskLogViewerProps {
  taskId: string
  /** 区块标题（默认 `FFmpeg 日志`） */
  title?: string
  /** 日志区最大高度（默认 260px） */
  maxHeight?: string
}

export function TaskLogViewer({ taskId, title = 'FFmpeg 日志', maxHeight = '260px' }: TaskLogViewerProps) {
  const buffer = useQueueStore(selectTaskLog(taskId))
  const [fetched, setFetched] = useState<string[]>([])
  const [searchInput, setSearchInput] = useState('')
  const [searchText, setSearchText] = useState('')
  const [level, setLevel] = useState<LogLevelFilter>('all')
  const [autoScroll, setAutoScroll] = useState(true)
  const [clearedIndex, setClearedIndex] = useState(0)

  const contentRef = useRef<HTMLDivElement | null>(null)
  const stickRef = useRef(true)
  const searchGeneration = useRef(0)
  const mounted = useRef(true)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      searchGeneration.current += 1
    }
  }, [])

  // 缓冲为空时用 REST 补齐（重连后同样走这里）
  useEffect(() => {
    if (buffer && buffer.lines.length > 0) return
    let cancelled = false
    void (async () => {
      try {
        const result = await tasksApi.log(taskId, 200)
        if (cancelled || !mounted.current) return
        setFetched((result.logOutput ?? '').split('\n').map((line) => line.replace(/\r$/, '')))
      } catch (error) {
        // 拉取失败不弹错（页面其它区域已有错误反馈），只记录
        console.warn(`[log] 读取任务日志失败：${getErrorMessage(error)}`)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [buffer, taskId])

  const rawLines = buffer && buffer.lines.length > 0 ? buffer.lines : fetched
  const droppedLines = buffer?.dropped ?? 0

  const visibleLines = useMemo(() => {
    // 只保留最后 RENDER_WINDOW 行；「清空显示」只隐藏已显示过的部分
    const kept = rawLines.slice(Math.max(clearedIndex, rawLines.length - 800))
    const tail = kept.length > RENDER_WINDOW ? kept.slice(kept.length - RENDER_WINDOW) : kept

    const search = searchText.trim().toLowerCase()
    const filtered = tail.filter((line) => {
      if (level === 'error' && !ERROR_PATTERN.test(line)) return false
      if (level === 'warning' && !WARNING_PATTERN.test(line)) return false
      if (search !== '' && !line.toLowerCase().includes(search)) return false
      return true
    })
    return filtered
  }, [clearedIndex, level, rawLines, searchText])

  const isCleared = clearedIndex > 0 && visibleLines.length === 0 && rawLines.length <= clearedIndex
  const filtering = searchText.trim() !== '' || level !== 'all'

  // 自动滚动：只在内容变化且用户仍贴底时滚到底
  useEffect(() => {
    const element = contentRef.current
    if (!element) return
    if (autoScroll && stickRef.current) element.scrollTop = element.scrollHeight
  }, [autoScroll, visibleLines])

  const handleScroll = useCallback(() => {
    const element = contentRef.current
    if (!element) return
    const distance = element.scrollHeight - element.scrollTop - element.clientHeight
    stickRef.current = distance <= STICK_THRESHOLD
  }, [])

  const handleSearchInput = (raw: string) => {
    setSearchInput(raw)
    const generation = ++searchGeneration.current
    setTimeout(() => {
      if (!mounted.current || generation !== searchGeneration.current) return
      setSearchText(raw)
    }, LOG_SEARCH_DEBOUNCE_MS)
  }

  const handleCopy = async () => {
    await copyAndToast(visibleLines.join('\n'), {
      successText: '日志已复制到剪贴板。',
      emptyText: '没有可复制的日志。',
      failureText: COPY_FAILURE_MESSAGES.log,
    })
  }

  const handleClear = () => {
    setClearedIndex(rawLines.length)
    toast.show('日志已清空（新日志仍会继续显示）')
  }

  const levelOptions: { value: LogLevelFilter; label: string }[] = [
    { value: 'all', label: '全部' },
    { value: 'error', label: '仅错误' },
    { value: 'warning', label: '仅警告' },
  ]

  return (
    <div className="rounded-md border border-line overflow-hidden" style={{ background: 'var(--code-bg)' }}>
      <div className="flex flex-wrap items-center gap-2 px-3 py-2 border-b border-line bg-surface-3">
        <span className="font-medium" style={{ fontSize: '0.85rem' }}>📜 {title}</span>

        <label className="visually-hidden" htmlFor={`log-search-${taskId}`}>
          搜索日志
        </label>
        <input
          id={`log-search-${taskId}`}
          type="search"
          placeholder="搜索..."
          style={{ width: '150px' }}
          value={searchInput}
          onChange={(event) => handleSearchInput(event.target.value)}
          onBlur={(event) => {
            searchGeneration.current += 1
            setSearchText(event.target.value)
          }}
        />

        <select
          aria-label="日志级别筛选"
          value={level}
          style={{ width: 'auto' }}
          onChange={(event) => setLevel(event.target.value as LogLevelFilter)}
        >
          {levelOptions.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>

        <IconButton ariaLabel="复制日志" title="复制日志" onClick={() => void handleCopy()}>
          📋
        </IconButton>
        <IconButton ariaLabel="清空显示的日志" title="清空显示的日志" onClick={handleClear}>
          🗑️
        </IconButton>

        <label className="inline-flex items-center gap-1.5 text-muted-text" style={{ fontSize: '0.8rem' }}>
          <input
            type="checkbox"
            checked={autoScroll}
            onChange={(event) => {
              const next = event.target.checked
              setAutoScroll(next)
              stickRef.current = next
            }}
          />
          自动滚动
        </label>

        {isCleared ? (
          <>
            <span className="text-muted-text" style={{ fontSize: '0.8rem' }}>
              日志已清空（新日志仍会继续显示）
            </span>
            <Button size="sm" onClick={() => setClearedIndex(0)}>
              恢复显示
            </Button>
          </>
        ) : null}
      </div>

      <div
        ref={contentRef}
        className="mono"
        style={{ maxHeight, overflowY: 'auto', fontSize: '0.85rem', lineHeight: 1.6 }}
        onScroll={handleScroll}
      >
        {visibleLines.length === 0 ? (
          <div className="px-3 py-3 text-muted-text">
            {rawLines.length === 0
              ? '暂无日志输出'
              : filtering
                ? '没有符合当前搜索/筛选条件的日志行'
                : '日志已清空（新日志仍会继续显示）'}
          </div>
        ) : (
          visibleLines.map((line, index) => {
            const isError = ERROR_PATTERN.test(line)
            const isWarning = !isError && WARNING_PATTERN.test(line)
            const isProgress = !isError && !isWarning && PROGRESS_PATTERN.test(line)
            return (
              <div
                key={`${index}-${line.slice(0, 24)}`}
                className="flex gap-2 px-3 hover:bg-surface-3"
                style={{
                  background: isError ? 'var(--danger-bg)' : isWarning ? 'var(--warning-bg)' : undefined,
                  color: isError
                    ? 'var(--danger-text)'
                    : isWarning
                      ? 'var(--warning-text)'
                      : isProgress
                        ? 'var(--info-text)'
                        : undefined,
                }}
              >
                <span
                  className="shrink-0 text-right text-muted-text"
                  style={{ minWidth: '44px', userSelect: 'none' }}
                >
                  {index + 1}
                </span>
                <span className="break-all">{highlight(line, searchText)}</span>
              </div>
            )
          })
        )}
      </div>

      <div className="flex flex-wrap gap-2 px-3 py-1.5 bg-surface-3 text-muted-text" style={{ fontSize: '0.78rem' }}>
        <span>共 {rawLines.length} 行</span>
        {visibleLines.length !== rawLines.length ? <span>· 显示 {visibleLines.length} 行</span> : null}
        {filtering ? <span>· 匹配 {visibleLines.length} 行</span> : null}
        {clearedIndex > 0 ? <span>· 已清空显示</span> : null}
        {droppedLines > 0 ? <span>· 已丢弃 {droppedLines} 行（超出上限）</span> : null}
      </div>
    </div>
  )
}

/** 字面量高亮（不区分大小写），只包一层 `<mark>`，避免正则注入 */
function highlight(line: string, search: string): ReactNode {
  const needle = search.trim()
  if (needle === '') return line
  const haystack = line.toLowerCase()
  const target = needle.toLowerCase()
  const parts: ReactNode[] = []
  let cursor = 0
  let index = haystack.indexOf(target, cursor)
  let key = 0
  while (index >= 0) {
    if (index > cursor) parts.push(line.slice(cursor, index))
    parts.push(
      <mark
        key={key++}
        style={{ background: 'var(--warning-bg)', color: 'var(--warning-text)' }}
      >
        {line.slice(index, index + needle.length)}
      </mark>,
    )
    cursor = index + needle.length
    index = haystack.indexOf(target, cursor)
  }
  if (parts.length === 0) return line
  parts.push(line.slice(cursor))
  return <>{parts}</>
}
