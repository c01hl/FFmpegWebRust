/**
 * 批量处理进度面板（对应 ui-spec §3.11 `BatchProgressPanel.razor`）。
 *
 * - 计数**全部从 `tasks` 推导**（不额外传计数参数，避免出现负数或与实际不符）；
 * - 总进度 = `已结束任务数 / 总数`（已结束 = 完成 + 失败 + 取消 + 跳过）；
 * - 默认只显示「优先级任务」（运行中/等待中/失败/已取消）+ 最多补足 8 条按创建时间倒序的其它任务；
 * - 单任务操作为互斥按钮：运行中 → `终止`；等待中 → `取消`；其它 → `重试`；
 *   失败/已取消且有错误 → `复制错误`；已完成 → `打开目录`。
 */

import { useEffect, useState } from 'react'
import type { TaskDto, TaskStatus } from '../../api/types'
import { taskStatusText } from '../../api/types'
import { copyAndToast } from '../../lib/clipboard'
import { formatDuration, formatFileSize, toFixedTrim } from '../../lib/format'
import { Badge } from '../ui/Badge'
import { Button } from '../ui/Button'

/** 每批最多显示的补充任务数（与参考实现一致：补足到 8 条） */
const FILLER_TARGET = 8

export interface BatchProgressPanelProps {
  /** 本批次的全部任务 */
  tasks: TaskDto[]
  isProcessing: boolean
  onRetry: (taskId: string) => void
  onCancel: (taskId: string) => void
  /** 打开输出文件所在的目录 */
  onOpenOutputFolder: (outputPath: string) => void
}

const STATUS_ICON: Record<TaskStatus, string> = {
  Completed: '✅',
  Failed: '❌',
  Running: '⏳',
  Cancelled: '⏹️',
  Skipped: '⏭️',
  Pending: '⏸️',
}

const STATUS_TONE: Record<TaskStatus, 'default' | 'success' | 'danger' | 'warning' | 'info'> = {
  Completed: 'success',
  Failed: 'danger',
  Running: 'info',
  Cancelled: 'warning',
  Skipped: 'warning',
  Pending: 'default',
}

const STATUS_STRIPE: Record<TaskStatus, string> = {
  Running: 'var(--info-color)',
  Completed: 'var(--success-color)',
  Failed: 'var(--danger-color)',
  Cancelled: 'var(--warning-color)',
  Skipped: 'var(--warning-color)',
  Pending: 'var(--border-strong)',
}

/** 错误信息截断：最多 90 字符（超出加省略号） */
function shorten(text: string, max = 90): string {
  return text.length <= max ? text : `${text.slice(0, max)}…`
}

/** 文件名（兼容 Windows 分隔符） */
function fileNameOf(path: string): string {
  return path.replace(/\\/g, '/').split('/').pop() || path
}

function createdAt(task: TaskDto): number {
  const time = Date.parse(task.createdAt)
  return Number.isFinite(time) ? time : 0
}

export function BatchProgressPanel({
  tasks,
  isProcessing,
  onRetry,
  onCancel,
  onOpenOutputFolder,
}: BatchProgressPanelProps) {
  const [showAll, setShowAll] = useState(false)
  /** 处理中的「当前时间」：由 1 秒定时器写入，避免在渲染期调用 Date.now()（不纯） */
  const [now, setNow] = useState(0)

  useEffect(() => {
    if (!isProcessing) return
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [isProcessing])

  const total = tasks.length
  const completed = tasks.filter((task) => task.status === 'Completed').length
  const failed = tasks.filter((task) => task.status === 'Failed').length
  const cancelled = tasks.filter((task) => task.status === 'Cancelled').length
  const skipped = tasks.filter((task) => task.status === 'Skipped').length
  const running = tasks.filter((task) => task.status === 'Running').length
  const pending = tasks.filter((task) => task.status === 'Pending').length
  const finished = completed + failed + cancelled + skipped
  const percent = total > 0 ? Math.max(0, Math.min(100, (finished * 100) / total)) : 0

  // 可见任务：优先级任务 + 补足项，最后按创建时间正序
  const priority = tasks.filter((task) =>
    task.status === 'Running' || task.status === 'Pending' || task.status === 'Failed' || task.status === 'Cancelled',
  )
  const priorityIds = new Set(priority.map((task) => task.id))
  const filler = tasks
    .filter((task) => !priorityIds.has(task.id))
    .sort((left, right) => createdAt(right) - createdAt(left))
    .slice(0, Math.max(0, FILLER_TARGET - priority.length))

  const visible = (showAll ? [...tasks] : [...priority, ...filler]).sort(
    (left, right) => createdAt(left) - createdAt(right),
  )
  const hidden = Math.max(0, total - visible.length)

  const elapsed = (() => {
    if (total === 0) return null
    const fallbackNow = Math.max(...tasks.map((task) => createdAt(task)))
    const referenceNow = now > 0 ? now : fallbackNow
    const end = Math.max(
      ...tasks.map((task) => (task.completedAt ? Date.parse(task.completedAt) : referenceNow)),
    )
    const start = Math.min(...tasks.map((task) => createdAt(task)))
    const seconds = (end - start) / 1000
    return Number.isFinite(seconds) && seconds > 0 ? seconds : null
  })()

  const etaSeconds = (() => {
    const estimates = tasks
      .filter((task) => task.status === 'Running' && (task.estimatedTimeRemaining ?? 0) > 0)
      .map((task) => task.estimatedTimeRemaining ?? 0)
    if (estimates.length === 0) return null
    const average = estimates.reduce((sum, value) => sum + value, 0) / estimates.length
    return Math.max(...estimates) + pending * average
  })()

  if (total === 0) {
    return (
      <div className="ffui-empty-state">
        还没有开始批量处理。添加文件、选好模板后点击「开始批量处理」。
      </div>
    )
  }

  return (
    <div>
      <div className="mb-3.5" aria-live="polite">
        <div className="flex flex-wrap items-center gap-2.5 mb-2">
          <span className="font-semibold" style={{ fontSize: '1.15rem' }}>
            {finished} / {total}
          </span>
          <span className="text-muted-text" style={{ fontSize: '0.83rem' }}>
            已处理
          </span>
          {running > 0 ? <Badge tone="info">{running} 个进行中</Badge> : null}
          {pending > 0 ? <Badge>{pending} 个等待中</Badge> : null}
          {!isProcessing && finished === total ? (
            <Badge tone={failed + cancelled === 0 ? 'success' : 'warning'}>
              {failed + cancelled === 0 ? '全部成功' : '已完成，含失败项'}
            </Badge>
          ) : null}
        </div>

        <div
          className="ffui-progress-track"
          role="progressbar"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(percent)}
          aria-label="批量处理总进度"
        >
          <div className="ffui-progress-fill" style={{ width: `${toFixedTrim(percent, 2)}%` }} />
        </div>

        <div className="flex flex-wrap gap-3.5 mt-2" style={{ fontSize: '0.84rem' }}>
          <span className="text-success">✅ 已完成 {completed}</span>
          {failed > 0 ? <span className="text-danger">❌ 失败 {failed}</span> : null}
          {cancelled > 0 ? <span className="text-warning">⏹️ 已取消 {cancelled}</span> : null}
          {skipped > 0 ? <span className="text-warning">⏭️ 已跳过 {skipped}</span> : null}
          {elapsed !== null ? <span className="text-muted-text">已用时 {formatDuration(elapsed)}</span> : null}
          {etaSeconds !== null ? (
            <span className="text-muted-text">预计还需 {formatDuration(etaSeconds)}</span>
          ) : null}
        </div>
      </div>

      <div className="flex flex-col gap-1.5" style={{ maxHeight: '460px', overflowY: 'auto' }} role="list">
        {visible.map((task) => {
          const errorText = task.errorMessage ?? ''
          return (
            <div
              key={task.id}
              role="listitem"
              className="flex items-center gap-2.5 rounded-sm border border-line bg-surface-3 px-2.5 py-2"
              style={{ borderLeftWidth: '3px', borderLeftColor: STATUS_STRIPE[task.status] }}
            >
              <span aria-hidden="true">{STATUS_ICON[task.status]}</span>

              <div className="flex-1 min-w-0">
                <div className="truncate" style={{ fontSize: '0.88rem' }} title={task.inputPath}>
                  {fileNameOf(task.inputPath)}
                </div>
                <div className="flex flex-wrap items-center gap-2 mt-0.5">
                  <Badge tone={STATUS_TONE[task.status]}>{taskStatusText(task.status, 'batch')}</Badge>
                  {task.status === 'Running' ? (
                    <>
                      <span className="text-muted-text" style={{ fontSize: '0.83rem' }}>
                        {toFixedTrim(task.progress, 1)}%
                      </span>
                      {(task.processingSpeed ?? 0) > 0 ? (
                        <span className="text-muted-text" style={{ fontSize: '0.83rem' }}>
                          {toFixedTrim(task.processingSpeed ?? 0, 2)}x
                        </span>
                      ) : null}
                    </>
                  ) : (task.outputFileSize ?? 0) > 0 ? (
                    <span className="text-muted-text" style={{ fontSize: '0.83rem' }}>
                      {formatFileSize(task.outputFileSize)}
                    </span>
                  ) : null}
                  {errorText !== '' ? (
                    <span className="text-danger" style={{ fontSize: '0.8rem' }} title={errorText}>
                      {shorten(errorText)}
                    </span>
                  ) : null}
                </div>
              </div>

              <div className="flex flex-wrap gap-1.5 justify-end shrink-0">
                {task.status === 'Running' ? (
                  <Button size="sm" variant="danger" onClick={() => onCancel(task.id)}>
                    终止
                  </Button>
                ) : task.status === 'Pending' ? (
                  <Button size="sm" onClick={() => onCancel(task.id)}>
                    取消
                  </Button>
                ) : (
                  <Button size="sm" onClick={() => onRetry(task.id)}>
                    重试
                  </Button>
                )}

                {(task.status === 'Failed' || task.status === 'Cancelled') && errorText !== '' ? (
                  <Button
                    size="sm"
                    onClick={() =>
                      void copyAndToast(errorText === '' ? '(没有错误信息)' : errorText, {
                        successText: '错误信息已复制',
                        emptyText: '(没有错误信息)',
                        failureText: '复制失败：浏览器不允许访问剪贴板，请手动选中文本复制。',
                        includeErrorMessage: false,
                      })
                    }
                  >
                    复制错误
                  </Button>
                ) : null}

                {task.status === 'Completed' ? (
                  <Button size="sm" onClick={() => onOpenOutputFolder(task.outputPath)}>
                    打开目录
                  </Button>
                ) : null}
              </div>
            </div>
          )
        })}
      </div>

      {hidden > 0 || showAll ? (
        <Button size="sm" className="mt-2.5" onClick={() => setShowAll((value) => !value)}>
          {showAll ? '只看进行中与失败的任务' : `还有 ${hidden} 个任务，展开全部`}
        </Button>
      ) : null}
    </div>
  )
}
