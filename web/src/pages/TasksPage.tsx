/**
 * 任务列表页（对应 ui-spec §4.4 与参考实现 `Components/Pages/Tasks.razor`）。
 *
 * 三段式布局：🚀 进行中（实时卡片）/ ⏳ 等待中（行列表）/ 📜 历史记录（筛选 + 批量操作 + 表格）。
 *
 * ── 数据来源（重要）──────────────────────────────────────────────────────
 * - **进行中 / 等待中 + 实时进度**：全部来自共享的 WS 事件总线派发到 `store/queue.ts` 的状态
 *   （服务端连上就推快照，之后是 `task.progress` / `task.status` 增量）。本页**绝不自己开
 *   WebSocket**；只在「还没有拿到过快照」或「实时通道断开」时用 REST 兜底填充（见 syncLiveLists）。
 * - **历史记录**：REST `GET /api/tasks?status=&limit=&offset=`（分页）。WS 的 `task.status`
 *   事件会把密集变化合并成一次重新读库（200ms 合并延迟，与参考实现一致）。
 * - **日志**：展开某条任务时挂载 `components/tasks/LogViewer.tsx`，它自己拉
 *   `GET /api/tasks/{id}/log?tailLines=200` 并订阅 `task.log` 增量。
 *
 * 中文文案逐字对齐 ui-spec §4.4 / §4.9 / §12；状态中文名、徽标色调、左侧竖条色
 * 一律取自 `api/types.ts` 的全站状态映射（唯一真相）。
 */

import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { CSSProperties } from 'react'
import { Link } from 'react-router-dom'
import { ApiError, getErrorMessage } from '../api/client'
import { filesApi, historyApi, settingsApi, systemApi, tasksApi } from '../api/endpoints'
import type { Id, TaskDto, TaskStatus, TaskStatusEventDto, WsSnapshotDto } from '../api/types'
import {
  ALL_TASK_STATUSES,
  TASK_STATUS_ICON,
  TASK_STATUS_STRIPE_VAR,
  TASK_STATUS_TEXT,
  TASK_STATUS_TONE,
  isTerminalStatus,
  taskStatusText,
} from '../api/types'
import { onWsEvent, onWsStatus } from '../api/ws'
import { COPY_FAILURE_MESSAGES, copyTextDetailed } from '../lib/clipboard'
import {
  EMPTY_VALUE,
  formatDateTime,
  formatDateTimeShort,
  formatDuration,
  formatFileSize,
  formatFileName,
  formatProgressTime,
  formatSpeed,
} from '../lib/format'
import { PAGE_TITLES, useDocumentTitle } from '../lib/title'
import { selectProgress, useQueueStore } from '../store/queue'
import { toast } from '../store/toast'
import { Alert } from '../components/ui/Alert'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { ConfirmDialog } from '../components/ui/ConfirmDialog'
import { Field } from '../components/ui/Field'
import { ProgressBar } from '../components/ui/ProgressBar'
import { Select } from '../components/ui/Select'
import { LogViewer } from '../components/tasks/LogViewer'

/** 「重试全部失败任务」一次最多扫描的记录数（参考实现同名常量） */
const RETRY_SCAN_LIMIT = 1000
/** 显示条数选项（重新读库） */
const LIMIT_OPTIONS = [50, 100, 500]
/** 排序选项（仅内存排序） */
const SORT_OPTIONS = [
  { value: 'newest', label: '最新优先' },
  { value: 'oldest', label: '最早优先' },
]
/** 状态筛选选项（改变查询条件 → 重新读库） */
const STATUS_FILTER_OPTIONS = [
  { value: '', label: '全部' },
  ...ALL_TASK_STATUSES.map((status) => ({ value: status, label: TASK_STATUS_TEXT[status] })),
]
/** 每个展开任务的日志最多保留的行数（与 LogViewer 默认值一致） */
const LOG_MAX_LINES = 800
/** 状态变化合并刷新的等待时间（毫秒，参考实现同名常量） */
const STATUS_REFRESH_DELAY_MS = 200
/** 实时通道断开时的兜底轮询间隔 */
const OFFLINE_POLL_INTERVAL_MS = 5_000
/** 「已运行」计时器的刷新间隔 */
const TICK_INTERVAL_MS = 1_000

/** 批量操作的确认框种类 */
type ConfirmKind =
  | 'retryAllFailed'
  | 'clearFailedAndCancelled'
  | 'cleanupHistory'
  | 'clearHistory'
  | 'deleteByStatus'
  | 'deleteTask'

interface ConfirmRequest {
  kind: ConfirmKind
  /** `deleteTask` 用 */
  taskId?: Id
  /** `deleteByStatus` 用 */
  status?: TaskStatus
}

/** 表格单元格通用样式（颜色走设计令牌） */
const CELL_STYLE: CSSProperties = {
  padding: '0.6rem 0.7rem',
  textAlign: 'left',
  verticalAlign: 'middle',
  borderBottom: '1px solid var(--border-color)',
  fontSize: '0.9rem',
}

/** 表头单元格样式 */
const HEAD_CELL_STYLE: CSSProperties = {
  ...CELL_STYLE,
  background: 'var(--surface-3)',
  color: 'var(--text-muted)',
  fontSize: '0.82rem',
  fontWeight: 600,
}

/** 文件名：只显示文件名 + 省略号 + `title` 全文（≤768px 时收窄到 160px，见 ui-spec §4.4） */
const NAME_CELL_CLASS = 'inline-block max-w-[240px] truncate align-bottom max-md:max-w-[160px]'

/** 文件名（取不到时用「任务」，与参考实现一致） */
function taskNameOf(path: string | null | undefined): string {
  const name = formatFileName(path)
  return name === EMPTY_VALUE ? '任务' : name
}

/** 已运行秒数（`startedAt` 缺失或非法 → null，展示层会给出 `-`） */
function elapsedSeconds(task: TaskDto, now: number): number | null {
  if (!task.startedAt) return null
  const started = Date.parse(task.startedAt)
  if (!Number.isFinite(started)) return null
  return Math.max(0, (now - started) / 1000)
}

/** `HH:mm:ss`（任务卡片的「开始于」；时间缺失时用 `-`） */
function clockTimeOf(value: string | null | undefined): string {
  if (!value) return '-'
  const text = formatDateTime(value)
  return text === EMPTY_VALUE ? '-' : text.slice(-8)
}

/**
 * 详情行的「标签 + 值」两段式（参考实现 `.detail-item`）。
 * `mono` 用于路径，`danger` 用于错误信息。
 */
function DetailItem({
  label,
  value,
  mono = false,
  danger = false,
}: {
  label: string
  value: string
  mono?: boolean
  danger?: boolean
}) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <span className="text-xs" style={{ color: 'var(--text-muted)' }}>
        {label}
      </span>
      <span
        className={mono ? 'mono break-all text-sm' : 'break-all text-sm'}
        style={danger ? { color: 'var(--danger-color)' } : undefined}
      >
        {value}
      </span>
    </div>
  )
}

/** 是否是「已结束」的任务可以重试 / 打开输出目录 */
function canRetry(status: TaskStatus): boolean {
  return isTerminalStatus(status)
}

/** HTTP 409：任务已终态 / 状态已变化 */
function isConflict(error: unknown): boolean {
  return error instanceof ApiError && error.status === 409
}

/** 从 store 里找一个任务的输入路径（`task.status` 事件没带 task 时的兜底） */
function findLiveInputPath(taskId: Id): string | undefined {
  const state = useQueueStore.getState()
  const found =
    state.runningTasks.find((task) => task.id === taskId) ??
    state.pendingTasks.find((task) => task.id === taskId)
  return found?.inputPath
}

/**
 * 任务状态变化提示（逐字来自 ui-spec §4.4「状态变化 Toast」）。
 * 等待中/运行中不打扰用户。
 */
function notifyStatusChange(event: TaskStatusEventDto): void {
  const name = taskNameOf(event.task?.inputPath ?? findLiveInputPath(event.taskId))
  switch (event.newStatus) {
    case 'Completed':
      toast.success(`✅ 转换完成：${name}`)
      break
    case 'Failed':
      toast.error(
        event.errorMessage
          ? `❌ 转换失败：${name} — ${event.errorMessage}`
          : `❌ 转换失败：${name}`,
      )
      break
    case 'Cancelled':
      toast.warning(`已取消：${name}`)
      break
    case 'Skipped':
      toast.warning(`已跳过：${name}（输出文件已存在）`)
      break
    default:
      break
  }
}

/* ── 进行中任务卡片（自己订阅该任务的实时进度）──────────────────────────── */

interface RunningTaskCardProps {
  task: TaskDto
  /** 每秒更新一次的时间戳，用于「已运行」 */
  now: number
  expanded: boolean
  busy: boolean
  onToggleLog: (task: TaskDto) => void
  onCancel: (task: TaskDto, graceful: boolean) => void
}

function RunningTaskCard({
  task,
  now,
  expanded,
  busy,
  onToggleLog,
  onCancel,
}: RunningTaskCardProps) {
  // 实时进度来自共享 WS 事件总线（store），事件还没到就回退到 TaskDto 里的值
  const live = useQueueStore(selectProgress(task.id))
  const elapsed = elapsedSeconds(task, now)

  return (
    <article
      className="mb-3 rounded-md p-4 last:mb-0"
      style={{
        background: 'var(--surface-3)',
        border: '1px solid var(--border-color)',
        borderRadius: 'var(--radius-md)',
      }}
    >
      <div className="mb-2.5 flex flex-wrap items-center gap-2">
        <span className={NAME_CELL_CLASS} style={{ fontWeight: 600, maxWidth: '320px' }} title={task.inputPath}>
          {formatFileName(task.inputPath)}
        </span>
        <span aria-hidden="true" style={{ color: 'var(--text-muted)' }}>
          ➜
        </span>
        <span
          className={NAME_CELL_CLASS}
          style={{ color: 'var(--text-secondary)', maxWidth: '320px' }}
          title={task.outputPath}
        >
          {formatFileName(task.outputPath)}
        </span>
        <Badge tone={TASK_STATUS_TONE.Running} icon={TASK_STATUS_ICON.Running}>
          {taskStatusText(task.status)}
        </Badge>
        {task.templateName.trim().length > 0 ? (
          <span style={{ color: 'var(--text-muted)' }}>{task.templateName}</span>
        ) : null}
      </div>

      <ProgressBar
        label="转换进度"
        progress={live?.progress ?? task.progress}
        speed={live?.speed ?? task.processingSpeed}
        eta={live?.eta ?? task.estimatedTimeRemaining}
        currentTime={live?.currentTime ?? task.currentTime}
        totalDuration={live?.totalDuration ?? task.totalDuration}
        status={task.status}
      />

      <div className="mt-2.5 flex flex-wrap gap-x-4 gap-y-1 text-sm" style={{ color: 'var(--text-muted)' }}>
        <span>开始于 {clockTimeOf(task.startedAt)}</span>
        <span>已运行 {formatDuration(elapsed)}</span>
        <span>输入 {formatFileSize(task.inputFileSize)}</span>
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-1.5">
        <Button
          size="sm"
          disabled={busy}
          title="发送 q 让 FFmpeg 写完文件尾再退出，已产出的文件仍然可用"
          onClick={() => onCancel(task, true)}
        >
          ⏏️ 优雅停止
        </Button>
        <Button
          size="sm"
          variant="danger"
          disabled={busy}
          title="立即结束 FFmpeg 进程，未完成的输出文件会被清理"
          onClick={() => onCancel(task, false)}
        >
          ❌ 强制终止
        </Button>
        <Button size="sm" disabled={busy} onClick={() => onToggleLog(task)}>
          {expanded ? '▲ 收起日志' : '▼ 查看日志'}
        </Button>
      </div>

      {expanded ? (
        <div className="mt-3">
          <LogViewer taskId={task.id} maxLines={LOG_MAX_LINES} maxHeight="320px" />
        </div>
      ) : null}
    </article>
  )
}

/** 页面主体 */
export function TasksPage() {
  useDocumentTitle(PAGE_TITLES.tasks)

  /* ── 实时（共享 WS → store）────────────────────────────────────────── */
  const runningTasks = useQueueStore((state) => state.runningTasks)
  const pendingTasks = useQueueStore((state) => state.pendingTasks)
  const activeCount = useQueueStore((state) => state.active)
  const queuedCount = useQueueStore((state) => state.queued)

  /* ── 历史记录（本页自己维护）───────────────────────────────────────── */
  const [statusFilter, setStatusFilter] = useState<'' | TaskStatus>('')
  const [filenameFilter, setFilenameFilter] = useState('')
  const [sortOrder, setSortOrder] = useState<'newest' | 'oldest'>('newest')
  const [limit, setLimit] = useState(50)
  const [offset, setOffset] = useState(0)
  const [history, setHistory] = useState<TaskDto[]>([])
  const [total, setTotal] = useState(0)
  const [isRefreshing, setIsRefreshing] = useState(false)
  const [listError, setListError] = useState<string | null>(null)

  /* ── 展开 / 忙碌 / 确认框 ──────────────────────────────────────────── */
  const [expandedTaskId, setExpandedTaskId] = useState<Id | null>(null)
  const [busyTaskId, setBusyTaskId] = useState<Id | null>(null)
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null)
  const [bulkBusy, setBulkBusy] = useState(false)
  /** 批量删除选中的状态（`DELETE /api/tasks?status=`） */
  const [bulkDeleteStatus, setBulkDeleteStatus] = useState<TaskStatus>('Failed')
  /** 「任务历史保留天数」（「清理历史」用），读不到就是 null */
  const [retentionDays, setRetentionDays] = useState<number | null>(null)
  /** 「已运行」用的时间戳，只在有运行中任务时每秒走一次 */
  const [now, setNow] = useState(() => Date.now())

  const refreshingRef = useRef(false)
  const reloadTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  /* ── 历史记录：读取 ───────────────────────────────────────────────── */

  const loadHistory = useCallback(
    async (showBusy = false) => {
      // 参考实现的行为：上一次刷新还在跑时直接放弃这一次（状态事件会再触发一次）
      if (refreshingRef.current) return
      refreshingRef.current = true
      if (showBusy) setIsRefreshing(true)
      try {
        const page = await tasksApi.list({
          status: statusFilter === '' ? undefined : statusFilter,
          limit,
          offset,
        })
        setHistory(page?.items ?? [])
        setTotal(page?.total ?? 0)
        setListError(null)
      } catch (error) {
        setListError(getErrorMessage(error))
        toast.error(`读取任务列表失败：${getErrorMessage(error)}`)
      } finally {
        refreshingRef.current = false
        if (showBusy) setIsRefreshing(false)
      }
    },
    [statusFilter, limit, offset],
  )

  /** 「清理历史」需要的保留天数（读不到时按钮会给提示） */
  const loadRetentionDays = useCallback(async () => {
    try {
      const settings = await settingsApi.get()
      setRetentionDays(settings.taskHistoryRetentionDays)
    } catch {
      // 只影响「清理历史」按钮，静默降级（按钮会提示无法读取）
      setRetentionDays(null)
    }
  }, [])

  /* ── 实时列表的兜底填充（没有快照 / 实时通道断开时）────────────────── */

  const syncLiveLists = useCallback(async (options?: { onlyIfEmpty?: boolean }) => {
    const state = useQueueStore.getState()
    if (options?.onlyIfEmpty && state.lastSnapshotAt !== null) return

    try {
      const [running, pending, queue] = await Promise.all([
        tasksApi.running(),
        tasksApi.pending(),
        systemApi.queue(),
      ])
      const snapshot: WsSnapshotDto = {
        queue: queue ?? {
          queued: pending?.length ?? 0,
          active: running?.length ?? 0,
          maxConcurrentTasks: useQueueStore.getState().maxConcurrentTasks,
        },
        runningTasks: running ?? [],
        pendingTasks: pending ?? [],
      }
      useQueueStore.getState().applySnapshot(snapshot)
    } catch (error) {
      // 兜底失败不算致命：实时通道恢复后会推快照
      console.warn('[tasks] 读取进行中/等待中任务失败：', getErrorMessage(error))
    }
  }, [])

  /* ── 状态变化 → 合并延迟后重新读库 ─────────────────────────────────── */

  const scheduleHistoryReload = useCallback(() => {
    if (reloadTimerRef.current !== null) return
    reloadTimerRef.current = setTimeout(() => {
      reloadTimerRef.current = null
      void loadHistory(false)
    }, STATUS_REFRESH_DELAY_MS)
  }, [loadHistory])

  useEffect(() => {
    // 读取历史列表属于「与外部系统同步」：setState 只发生在 await 之后，
    // 不会在 effect 同步阶段触发级联渲染，故显式关闭该启发式规则。
    // oxlint-disable-next-line react/set-state-in-effect
    void loadHistory(false)
  }, [loadHistory])

  useEffect(() => {
    void syncLiveLists({ onlyIfEmpty: true })
    // 同上：保留天数也是异步读取
    // oxlint-disable-next-line react/set-state-in-effect
    void loadRetentionDays()
  }, [syncLiveLists, loadRetentionDays])

  // 实时通道恢复：快照会自动回来，但本页自己拉的历史列表要重新读一次
  useEffect(() => {
    return onWsStatus((status) => {
      if (status !== 'open') return
      void syncLiveLists()
      void loadHistory(false)
      void loadRetentionDays()
    })
  }, [syncLiveLists, loadHistory, loadRetentionDays])

  useEffect(() => {
    return onWsEvent('task.status', (event) => {
      notifyStatusChange(event.data)
      scheduleHistoryReload()
    })
  }, [scheduleHistoryReload])

  // 兜底轮询：只在实时通道不是 open 时跑（正常情况完全依赖 WS，不查库）
  useEffect(() => {
    const timer = setInterval(() => {
      if (useQueueStore.getState().connection === 'open') return
      void syncLiveLists()
    }, OFFLINE_POLL_INTERVAL_MS)
    return () => clearInterval(timer)
  }, [syncLiveLists])

  // 「已运行」每秒走一格
  useEffect(() => {
    if (runningTasks.length === 0) return
    const timer = setInterval(() => setNow(Date.now()), TICK_INTERVAL_MS)
    return () => clearInterval(timer)
  }, [runningTasks.length])

  // 卸载时清掉合并延迟的计时器
  useEffect(() => {
    return () => {
      if (reloadTimerRef.current !== null) {
        clearTimeout(reloadTimerRef.current)
        reloadTimerRef.current = null
      }
    }
  }, [])

  /* ── 单任务操作 ───────────────────────────────────────────────────── */

  const runTaskAction = useCallback(
    async (task: TaskDto, action: () => Promise<void>) => {
      setBusyTaskId(task.id)
      try {
        await action()
      } finally {
        setBusyTaskId(null)
        void loadHistory(false)
      }
    },
    [loadHistory],
  )

  const startTask = useCallback(
    (task: TaskDto) =>
      runTaskAction(task, async () => {
        try {
          await tasksApi.start(task.id)
          toast.success(`已加入执行队列：${taskNameOf(task.inputPath)}`)
        } catch (error) {
          if (isConflict(error)) {
            toast.warning('这个任务已经结束或无法再次开始，列表可能不是最新的，已为你刷新。')
          } else {
            toast.error(`启动任务失败：${getErrorMessage(error)}`)
          }
        }
      }),
    [runTaskAction],
  )

  const cancelTask = useCallback(
    (task: TaskDto, graceful: boolean, successText: string) =>
      runTaskAction(task, async () => {
        try {
          await tasksApi.cancel(task.id, graceful)
          toast.success(successText)
        } catch (error) {
          if (isConflict(error)) {
            toast.warning('这个任务已经结束或无法取消，列表可能不是最新的，已为你刷新。')
          } else {
            toast.error(`停止任务失败：${getErrorMessage(error)}`)
          }
        }
      }),
    [runTaskAction],
  )

  const retryTask = useCallback(
    (task: TaskDto) =>
      runTaskAction(task, async () => {
        try {
          const result = await tasksApi.retry(task.id)
          toast.success(`已重新加入队列：${taskNameOf(result?.task?.inputPath ?? task.inputPath)}`)
        } catch (error) {
          if (error instanceof ApiError && error.status === 404) {
            toast.error('找不到原任务记录，无法重试。')
          } else {
            toast.error(`重试失败：${getErrorMessage(error)}`)
          }
        }
      }),
    [runTaskAction],
  )

  const copyCommand = useCallback(async (task: TaskDto) => {
    const command = (task.actualCommand ?? '').trim()
    if (command.length === 0) {
      toast.warning('这个任务还没有生成实际命令（可能尚未开始执行），暂时没有可复制的内容。')
      return
    }

    const result = await copyTextDetailed(`ffmpeg ${command}`)
    if (result.ok) toast.success('命令已复制到剪贴板')
    else if (result.message) toast.error(`复制命令失败：${result.message}`)
    else toast.error(COPY_FAILURE_MESSAGES.taskCommand)
  }, [])

  const copyError = useCallback(async (task: TaskDto) => {
    const text = (task.errorMessage ?? '').trim()
    if (text.length === 0) {
      toast.warning('这个任务没有记录错误信息。')
      return
    }

    const result = await copyTextDetailed(text)
    if (result.ok) toast.success('错误信息已复制到剪贴板')
    else if (result.message) toast.error(`复制错误信息失败：${result.message}`)
    else toast.error(COPY_FAILURE_MESSAGES.taskCommand)
  }, [])

  const revealOutput = useCallback(async (task: TaskDto) => {
    if ((task.outputPath ?? '').trim().length === 0) {
      toast.warning('输出目录不存在，可能已被移动或删除。')
      return
    }

    try {
      // 「打开输出目录」由服务端执行（ui-spec §4.9）：取 OutputPath 的目录名并调用系统文件管理器
      const result = await filesApi.reveal(task.outputPath)
      if (result?.ok === false) {
        toast.warning(result.reason?.trim() ? result.reason : '输出目录不存在，可能已被移动或删除。')
        return
      }
      toast.success('已在文件管理器中打开输出目录。')
    } catch (error) {
      toast.error(`无法打开输出目录：${getErrorMessage(error)}`)
    }
  }, [])

  const toggleLog = useCallback((task: TaskDto) => {
    setExpandedTaskId((current) => (current === task.id ? null : task.id))
  }, [])

  /* ── 批量操作 ─────────────────────────────────────────────────────── */

  const retryAllFailed = useCallback(async () => {
    let failed: TaskDto[]
    try {
      const page = await tasksApi.list({ status: 'Failed', limit: RETRY_SCAN_LIMIT, offset: 0 })
      failed = page?.items ?? []
    } catch (error) {
      toast.error(`读取失败任务失败：${getErrorMessage(error)}`)
      return
    }

    if (failed.length === 0) {
      toast.warning('没有失败的任务需要重试。')
      return
    }

    let queued = 0
    let skipped = 0
    let errors = 0
    let firstError: string | null = null

    for (const task of failed) {
      try {
        const result = await tasksApi.retry(task.id)
        if (result?.task) queued += 1
        else skipped += 1
      } catch (error) {
        errors += 1
        if (firstError === null) firstError = getErrorMessage(error)
      }
    }

    let text = `失败任务重试完成：已重新排队 ${queued} 个`
    if (skipped > 0) text += `，跳过 ${skipped} 个`
    if (errors > 0) text += `，${errors} 个出错（${firstError}）`

    if (errors > 0) toast.warning(text)
    else toast.success(text)
  }, [])

  /** 确认框里的动作（与参考实现的 ConfirmBulkAsync 一一对应） */
  const runConfirmedAction = useCallback(async () => {
    const request = confirm
    if (!request) return

    setBulkBusy(true)
    try {
      switch (request.kind) {
        case 'clearFailedAndCancelled': {
          const failed = await tasksApi.deleteByStatus('Failed')
          const cancelled = await tasksApi.deleteByStatus('Cancelled')
          const failedCount = failed?.deleted ?? 0
          const cancelledCount = cancelled?.deleted ?? 0
          setExpandedTaskId(null)
          toast.success(
            `已清理 ${failedCount + cancelledCount} 条记录（失败 ${failedCount} 条、已取消 ${cancelledCount} 条）。`,
          )
          break
        }

        case 'cleanupHistory': {
          if (retentionDays === null) {
            toast.warning('无法读取「任务历史保留天数」，请先打开「设置」页确认后再试。')
            return
          }
          if (retentionDays <= 0) {
            toast.warning('保留天数为 0，没有可清理的时间范围；请先在「设置」里填写天数，或使用「清空历史」。')
            return
          }
          const result = await historyApi.cleanup(retentionDays)
          setExpandedTaskId(null)
          toast.success(`已清理 ${result?.deletedTasks ?? 0} 条历史记录（保留最近 ${retentionDays} 天）。`)
          break
        }

        case 'clearHistory': {
          const result = await historyApi.clear()
          setExpandedTaskId(null)
          toast.success(`已清空 ${result?.deletedTasks ?? 0} 条历史记录（运行中与等待中的任务保留）。`)
          break
        }

        case 'retryAllFailed':
          await retryAllFailed()
          break

        case 'deleteByStatus': {
          const status = request.status ?? 'Failed'
          const result = await tasksApi.deleteByStatus(status)
          setExpandedTaskId(null)
          toast.success(`已删除 ${result?.deleted ?? 0} 条「${taskStatusText(status)}」任务记录。`)
          break
        }

        case 'deleteTask': {
          if (!request.taskId) break
          const result = await tasksApi.removeMany([request.taskId])
          setExpandedTaskId(null)
          toast.success(`已删除 ${result?.deleted ?? 0} 条任务记录。`)
          if ((result?.skippedRunning ?? 0) > 0) {
            toast.warning(`有 ${result?.skippedRunning} 个运行中的任务被跳过，未删除。`)
          }
          break
        }

        default:
          toast.warning('没有选择任何批量操作。')
          break
      }
    } catch (error) {
      toast.error(`批量操作失败：${getErrorMessage(error)}`)
    } finally {
      setBulkBusy(false)
      setConfirm(null)
      void loadHistory(false)
    }
  }, [confirm, retentionDays, retryAllFailed, loadHistory])

  /** 确认框文案（逐字来自 ui-spec §4.4 的批量操作表） */
  const confirmMeta = useMemo(() => {
    if (!confirm) return null
    switch (confirm.kind) {
      case 'retryAllFailed':
        return {
          title: '重试全部失败任务',
          message: `将扫描最近 ${RETRY_SCAN_LIMIT} 条失败的任务，逐个重新加入队列执行。`,
          detail: '会复用原来的输入文件、模板与参数。',
          confirmText: '重新排队',
          danger: false,
        }
      case 'clearFailedAndCancelled':
        return {
          title: '清理失败与取消的任务',
          message:
            '将删除全部「失败」和「已取消」的任务记录，运行中与等待中的任务不受影响。此操作不可撤销。',
          detail: '只删除记录，不会删除任何输出文件。',
          confirmText: '确认',
          danger: true,
        }
      case 'cleanupHistory':
        return {
          title: '清理历史',
          message:
            '将按「任务历史保留天数」删除更早的历史记录，运行中与等待中的任务保留。此操作不可撤销。',
          detail: '只删除任务记录，磁盘上的输出文件不会被删除。',
          confirmText: '确认',
          danger: true,
        }
      case 'clearHistory':
        return {
          title: '清空历史记录',
          message:
            '将删除全部已完成 / 失败 / 已取消 / 已跳过的历史记录，运行中与等待中的任务保留。此操作不可撤销。',
          detail: '只删除任务记录，磁盘上的输出文件不会被删除。',
          confirmText: '确认',
          danger: true,
        }
      case 'deleteByStatus':
        return {
          title: '按状态批量删除',
          message: `将删除全部「${taskStatusText(confirm.status ?? 'Failed')}」的任务记录。此操作不可撤销。`,
          detail: '只删除记录，不会删除任何输出文件；运行中的任务不会被删除。',
          confirmText: '确认',
          danger: true,
        }
      case 'deleteTask':
        return {
          title: '删除任务记录',
          message: '确定删除这条任务记录吗？',
          detail: '只删除记录，不会删除任何输出文件。此操作不可撤销。',
          confirmText: '删除',
          danger: true,
        }
    }
  }, [confirm])

  /* ── 历史表格：内存过滤 / 排序 / 去重 ──────────────────────────────── */

  const visibleHistory = useMemo(() => {
    const keyword = filenameFilter.trim().toLowerCase()
    const filtered =
      keyword.length === 0
        ? history
        : history.filter((task) => {
            const input = formatFileName(task.inputPath).toLowerCase()
            const output = formatFileName(task.outputPath).toLowerCase()
            return input.includes(keyword) || output.includes(keyword)
          })

    return [...filtered].sort((left, right) => {
      const leftAt = Date.parse(left.createdAt)
      const rightAt = Date.parse(right.createdAt)
      return sortOrder === 'oldest' ? leftAt - rightAt : rightAt - leftAt
    })
  }, [history, filenameFilter, sortOrder])

  // 运行中/等待中的任务已经在上面的分区里显示，历史表格不再重复（参考实现同样处理）
  const liveTaskIds = useMemo(
    () => new Set([...runningTasks, ...pendingTasks].map((task) => task.id)),
    [runningTasks, pendingTasks],
  )
  const historyRows = useMemo(
    () => visibleHistory.filter((task) => !liveTaskIds.has(task.id)),
    [visibleHistory, liveTaskIds],
  )

  const pageCount = Math.max(1, Math.ceil(total / limit))
  const pageIndex = Math.min(pageCount, Math.floor(offset / limit) + 1)
  const historyCountText =
    historyRows.length === total
      ? `${historyRows.length} 条记录`
      : `${historyRows.length} / ${total} 条记录`
  const hasAnyHistory = history.length > 0

  return (
    <div className="ffui-container flex flex-col gap-5">
      <header className="flex flex-wrap items-center gap-2.5">
        <h1 className="m-0">🗂️ 任务列表</h1>
        <Badge>
          运行中 {activeCount} · 排队 {queuedCount}
        </Badge>
        <span className="flex-1" />
        <Button
          size="sm"
          icon="🔄"
          loading={isRefreshing}
          onClick={() => void loadHistory(true)}
        >
          刷新
        </Button>
      </header>

      {/* ── 区块 1 · 🚀 进行中 ─────────────────────────────────────── */}
      <section className="ffui-panel">
        <h2 className="ffui-section-title">
          <span>🚀 进行中</span>
          <Badge>{runningTasks.length}</Badge>
        </h2>

        {listError !== null ? (
          <Alert tone="danger" className="mb-3">
            {`读取任务列表失败：${listError}`}
          </Alert>
        ) : null}

        {runningTasks.length === 0 ? (
          <div className="ffui-empty-state">
            当前没有正在执行的转换。
            <div className="mt-2.5 flex justify-center gap-2">
              <Link className="ffui-btn ffui-btn-primary ffui-btn-sm" to="/convert">
                开始转换
              </Link>
            </div>
          </div>
        ) : (
          runningTasks.map((task) => (
            <RunningTaskCard
              key={task.id}
              task={task}
              now={now}
              expanded={expandedTaskId === task.id}
              busy={busyTaskId === task.id}
              onToggleLog={toggleLog}
              onCancel={(target, graceful) =>
                void cancelTask(
                  target,
                  graceful,
                  graceful
                    ? '已发送优雅停止（q），FFmpeg 会写完文件尾再退出。'
                    : '已强制终止任务，未完成的输出文件会被清理。',
                )
              }
            />
          ))
        )}
      </section>

      {/* ── 区块 2 · ⏳ 等待中 ─────────────────────────────────────── */}
      <section className="ffui-panel">
        <h2 className="ffui-section-title">
          <span>⏳ 等待中</span>
          <Badge>{pendingTasks.length}</Badge>
        </h2>

        {pendingTasks.length === 0 ? (
          <div className="ffui-empty-state">队列里没有等待中的任务。</div>
        ) : (
          <ul className="m-0 flex list-none flex-col gap-2 p-0">
            {pendingTasks.map((task) => (
              <li
                key={task.id}
                className="flex flex-wrap items-center justify-between gap-3 rounded-sm px-3 py-2"
                style={{
                  background: 'var(--surface-3)',
                  border: '1px solid var(--border-color)',
                  borderRadius: 'var(--radius-sm)',
                }}
              >
                <div className="flex min-w-0 flex-col gap-0.5">
                  <span className={NAME_CELL_CLASS} title={task.inputPath}>
                    {formatFileName(task.inputPath)}
                  </span>
                  <span className="text-xs" style={{ color: 'var(--text-muted)' }}>
                    创建于 {formatDateTimeShort(task.createdAt)}
                  </span>
                </div>
                <div className="flex flex-wrap items-center gap-1.5">
                  <Badge tone={TASK_STATUS_TONE.Pending} icon={TASK_STATUS_ICON.Pending}>
                    {taskStatusText(task.status)}
                  </Badge>
                  <Button
                    size="sm"
                    disabled={busyTaskId === task.id}
                    onClick={() => void startTask(task)}
                  >
                    开始
                  </Button>
                  <Button
                    size="sm"
                    disabled={busyTaskId === task.id}
                    onClick={() => void cancelTask(task, false, '已取消排队，这个任务不会再开始执行。')}
                  >
                    取消排队
                  </Button>
                  <Button
                    size="sm"
                    disabled={busyTaskId === task.id}
                    onClick={() => setConfirm({ kind: 'deleteTask', taskId: task.id })}
                  >
                    删除
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      {/* ── 区块 3 · 📜 历史记录 ──────────────────────────────────── */}
      <section className="ffui-panel">
        <h2 className="ffui-section-title">
          <span>📜 历史记录</span>
          <Badge ariaLabel={historyCountText}>
            <span aria-live="polite">{historyCountText}</span>
          </Badge>
        </h2>

        <div className="mb-3.5 flex flex-wrap items-end gap-3">
          <div className="min-w-[140px]">
            <Select
              label="状态"
              value={statusFilter}
              options={STATUS_FILTER_OPTIONS}
              onChange={(event) => {
                setStatusFilter(event.target.value as '' | TaskStatus)
                setOffset(0)
              }}
            />
          </div>

          <div className="min-w-[140px] flex-1 basis-[220px]">
            <Field label="文件名" htmlFor="tasks-filename-filter">
              <input
                id="tasks-filename-filter"
                type="search"
                placeholder="按文件名筛选"
                value={filenameFilter}
                onChange={(event) => setFilenameFilter(event.target.value)}
              />
            </Field>
          </div>

          <div className="min-w-[140px]">
            <Select
              label="显示条数"
              value={String(limit)}
              options={LIMIT_OPTIONS.map((value) => ({ value: String(value), label: String(value) }))}
              onChange={(event) => {
                setLimit(Number(event.target.value))
                setOffset(0)
              }}
            />
          </div>

          <div className="min-w-[140px]">
            <Select
              label="排序"
              value={sortOrder}
              options={SORT_OPTIONS}
              onChange={(event) => setSortOrder(event.target.value as 'newest' | 'oldest')}
            />
          </div>

          <Button size="sm" icon="🔄" loading={isRefreshing} onClick={() => void loadHistory(true)}>
            刷新
          </Button>
        </div>

        <div
          className="mb-4 flex flex-wrap items-center gap-2 rounded-sm px-3 py-2"
          style={{
            background: 'var(--surface-3)',
            border: '1px solid var(--border-color)',
            borderRadius: 'var(--radius-sm)',
          }}
        >
          <span className="text-sm" style={{ color: 'var(--text-muted)' }}>
            批量操作
          </span>
          <Button
            size="sm"
            disabled={bulkBusy}
            onClick={() => setConfirm({ kind: 'retryAllFailed' })}
          >
            ♻️ 重试全部失败任务
          </Button>
          <Button
            size="sm"
            variant="danger"
            disabled={bulkBusy}
            onClick={() => setConfirm({ kind: 'clearFailedAndCancelled' })}
          >
            🧹 清理失败与取消的任务
          </Button>
          <Button
            size="sm"
            variant="danger"
            disabled={bulkBusy}
            onClick={() => setConfirm({ kind: 'cleanupHistory' })}
          >
            🧹 清理历史
          </Button>
          <Button
            size="sm"
            variant="danger"
            disabled={bulkBusy}
            onClick={() => setConfirm({ kind: 'clearHistory' })}
          >
            🗑️ 清空历史
          </Button>

          <span className="mx-1 flex-1" />

          <label className="text-sm" htmlFor="tasks-bulk-status" style={{ color: 'var(--text-muted)' }}>
            按状态批量删除
          </label>
          <select
            id="tasks-bulk-status"
            className="w-auto"
            value={bulkDeleteStatus}
            onChange={(event) => setBulkDeleteStatus(event.target.value as TaskStatus)}
          >
            {ALL_TASK_STATUSES.map((status) => (
              <option key={status} value={status}>
                {TASK_STATUS_TEXT[status]}
              </option>
            ))}
          </select>
          <Button
            size="sm"
            variant="danger"
            disabled={bulkBusy}
            onClick={() => setConfirm({ kind: 'deleteByStatus', status: bulkDeleteStatus })}
          >
            删除该状态的任务
          </Button>
        </div>

        {historyRows.length === 0 ? (
          <div className="ffui-empty-state">
            {hasAnyHistory
              ? '没有符合当前筛选条件的任务，试试清空筛选条件或点「刷新」。'
              : '还没有任何任务，去「转换」页开始第一个吧。'}
            {offset > 0 ? (
              <div className="mt-2.5 flex justify-center gap-2">
                <Button size="sm" onClick={() => setOffset(0)}>
                  返回第一页
                </Button>
              </div>
            ) : null}
            {hasAnyHistory ? null : (
              <div className="mt-2.5 flex justify-center gap-2">
                <Link className="ffui-btn ffui-btn-primary ffui-btn-sm" to="/convert">
                  去「转换」页开始第一个
                </Link>
                <Link className="ffui-btn ffui-btn-sm" to="/batch">
                  批量处理
                </Link>
              </div>
            )}
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full border-collapse">
              <thead>
                <tr>
                  <th scope="col" style={HEAD_CELL_STYLE}>
                    输入文件
                  </th>
                  <th scope="col" style={HEAD_CELL_STYLE}>
                    输出文件
                  </th>
                  <th scope="col" style={HEAD_CELL_STYLE}>
                    状态
                  </th>
                  <th scope="col" style={HEAD_CELL_STYLE}>
                    耗时
                  </th>
                  <th scope="col" style={HEAD_CELL_STYLE}>
                    创建时间
                  </th>
                  <th scope="col" style={HEAD_CELL_STYLE}>
                    操作
                  </th>
                </tr>
              </thead>
              <tbody>
                {historyRows.map((task) => {
                  const expanded = expandedTaskId === task.id
                  const elapsed = elapsedSeconds(task, now)
                  const stripe =
                    task.status === 'Failed' || task.status === 'Cancelled' || task.status === 'Skipped'
                      ? { boxShadow: `inset 3px 0 0 var(${TASK_STATUS_STRIPE_VAR[task.status]})` }
                      : undefined
                  const outputSize =
                    task.outputFileSize === null || task.outputFileSize === undefined
                      ? EMPTY_VALUE
                      : formatFileSize(task.outputFileSize)

                  return (
                    <Fragment key={task.id}>
                      <tr>
                        <td style={{ ...CELL_STYLE, ...stripe }}>
                          <span className={NAME_CELL_CLASS} title={task.inputPath}>
                            {formatFileName(task.inputPath)}
                          </span>
                        </td>
                        <td style={CELL_STYLE}>
                          <span className={NAME_CELL_CLASS} title={task.outputPath}>
                            {formatFileName(task.outputPath)}
                          </span>
                        </td>
                        <td style={CELL_STYLE}>
                          <Badge
                            tone={TASK_STATUS_TONE[task.status]}
                            icon={TASK_STATUS_ICON[task.status]}
                          >
                            {taskStatusText(task.status)}
                          </Badge>
                          {task.status === 'Failed' && task.errorMessage ? (
                            // 服务端的错误信息**逐字**展示，不做任何加工
                            <div
                              className="mt-1 max-w-[280px] break-words text-xs"
                              style={{ color: 'var(--danger-color)' }}
                              title={task.errorMessage}
                            >
                              {task.errorMessage}
                            </div>
                          ) : null}
                        </td>
                        <td style={CELL_STYLE}>
                          {task.status === 'Running' ? (
                            <>
                              <div>{formatDuration(elapsed)}</div>
                              <div className="text-xs" style={{ color: 'var(--text-muted)' }}>
                                {`速度 ${formatSpeed(task.processingSpeed)} · 剩余 ${
                                  task.estimatedTimeRemaining === null ||
                                  task.estimatedTimeRemaining === undefined
                                    ? EMPTY_VALUE
                                    : formatProgressTime(task.estimatedTimeRemaining)
                                }`}
                              </div>
                            </>
                          ) : (
                            <span>{formatDuration(task.durationSeconds ?? null)}</span>
                          )}
                        </td>
                        <td style={CELL_STYLE}>
                          <span title={formatDateTime(task.createdAt)}>
                            {formatDateTimeShort(task.createdAt)}
                          </span>
                        </td>
                        <td style={{ ...CELL_STYLE, whiteSpace: 'nowrap' }}>
                          <div className="flex flex-wrap items-center gap-1">
                            <Button
                              size="sm"
                              onClick={() => toggleLog(task)}
                              aria-expanded={expanded}
                            >
                              {expanded ? '收起日志' : '查看完整日志'}
                            </Button>
                            {canRetry(task.status) ? (
                              <>
                                <Button
                                  size="sm"
                                  disabled={busyTaskId === task.id}
                                  onClick={() => void retryTask(task)}
                                >
                                  重试
                                </Button>
                                <Button size="sm" onClick={() => void revealOutput(task)}>
                                  打开输出目录
                                </Button>
                              </>
                            ) : null}
                            <Button size="sm" onClick={() => void copyCommand(task)}>
                              复制命令
                            </Button>
                            {task.status === 'Failed' ? (
                              <Button size="sm" onClick={() => void copyError(task)}>
                                复制错误信息
                              </Button>
                            ) : null}
                            <Button
                              size="sm"
                              variant="danger"
                              disabled={busyTaskId === task.id}
                              onClick={() => setConfirm({ kind: 'deleteTask', taskId: task.id })}
                            >
                              删除
                            </Button>
                          </div>
                        </td>
                      </tr>

                      {expanded ? (
                        <tr>
                          <td colSpan={6} style={{ ...CELL_STYLE, background: 'var(--surface-3)' }}>
                            <div className="flex flex-col gap-3">
                              <div
                                style={{
                                  display: 'grid',
                                  gridTemplateColumns: 'repeat(auto-fit, minmax(280px, 1fr))',
                                  gap: '0.5rem 1rem',
                                }}
                              >
                                <DetailItem label="输入路径" value={task.inputPath || EMPTY_VALUE} mono />
                                <DetailItem label="输出路径" value={task.outputPath || EMPTY_VALUE} mono />
                                <DetailItem label="模板" value={task.templateName || EMPTY_VALUE} />
                                <DetailItem
                                  label="输入 / 输出大小"
                                  value={`${formatFileSize(task.inputFileSize)} → ${outputSize}`}
                                />
                                <DetailItem
                                  label="创建 / 完成时间"
                                  value={
                                    task.completedAt
                                      ? `${formatDateTime(task.createdAt)} → ${formatDateTime(task.completedAt)}`
                                      : formatDateTime(task.createdAt)
                                  }
                                />
                                {task.status === 'Skipped' ? (
                                  <DetailItem
                                    label="说明"
                                    value="输出文件已存在，按「文件已存在时的处理方式」的设置跳过。"
                                  />
                                ) : null}
                                {task.errorMessage ? (
                                  <DetailItem label="错误信息" value={task.errorMessage} danger />
                                ) : null}
                              </div>

                              <LogViewer
                                taskId={task.id}
                                maxLines={LOG_MAX_LINES}
                                maxHeight="320px"
                              />
                            </div>
                          </td>
                        </tr>
                      ) : null}
                    </Fragment>
                  )
                })}
              </tbody>
            </table>
          </div>
        )}

        {historyRows.length > 0 ? (
          <div className="mt-3 flex flex-wrap items-center gap-2">
            <span className="text-sm" style={{ color: 'var(--text-muted)' }}>
              第 {pageIndex} / {pageCount} 页（共 {total} 条）
            </span>
            <Button
              size="sm"
              disabled={offset <= 0 || isRefreshing}
              onClick={() => setOffset((value) => Math.max(0, value - limit))}
            >
              上一页
            </Button>
            <Button
              size="sm"
              disabled={offset + limit >= total || isRefreshing}
              onClick={() => setOffset((value) => value + limit)}
            >
              下一页
            </Button>
          </div>
        ) : null}
      </section>

      <ConfirmDialog
        open={confirm !== null}
        onOpenChange={(open) => {
          if (!open) setConfirm(null)
        }}
        title={confirmMeta?.title ?? '确认操作'}
        message={confirmMeta?.message ?? ''}
        detail={confirmMeta?.detail}
        confirmText={confirmMeta?.confirmText ?? '确认'}
        danger={confirmMeta?.danger ?? false}
        busy={bulkBusy}
        onConfirm={() => void runConfirmedAction()}
        onCancel={() => setConfirm(null)}
      />
    </div>
  )
}
