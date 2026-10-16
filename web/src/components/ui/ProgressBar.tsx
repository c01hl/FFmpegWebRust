import type { TaskStatus } from '../../api/types'
import { taskStatusText } from '../../api/types'
import { clampPercent, formatDurationPair, formatPercent, formatProgressTime, formatSpeed } from '../../lib/format'
import { cx } from '../../lib/cx'

export interface ProgressBarProps {
  /** 默认 `转换进度` */
  label?: string
  /** 0..100（会被 clamp，NaN/Infinity → 0） */
  progress: number
  /** 处理倍速；> 0 才显示 */
  speed?: number | null
  /** 预计剩余秒数；> 0 才显示 */
  eta?: number | null
  /** 已处理秒数 */
  currentTime?: number
  /** 输入总时长（秒） */
  totalDuration?: number
  /**
   * 任务状态。`null`/`undefined` 表示调用方不提供状态 ——
   * 此时**即使 100% 也不会显示成「已完成」**（进度条自身不判断业务状态）。
   */
  status?: TaskStatus | null
  className?: string
}

/** 填充色修饰类（与 ProgressBar.razor 的 FillClass 逻辑一致） */
function fillClass(status: TaskStatus | null | undefined, safeProgress: number): string {
  switch (status) {
    case 'Completed':
      return 'completed'
    case 'Failed':
      return 'failed'
    case 'Cancelled':
      return 'cancelled'
    case 'Skipped':
      return 'skipped'
    case 'Pending':
      return 'pending'
    case 'Running':
      return 'running'
    default:
      return safeProgress > 0 ? 'running' : 'pending'
  }
}

/** 百分比文字颜色修饰类（只有这三种状态改色） */
function percentageClass(status: TaskStatus | null | undefined): string {
  switch (status) {
    case 'Completed':
      return 'completed'
    case 'Failed':
      return 'failed'
    case 'Cancelled':
      return 'cancelled'
    default:
      return ''
  }
}

/**
 * 转换进度条（对应 ProgressBar.razor）。
 *
 * 无障碍：轨道是 `role="progressbar"`，带 aria-valuemin/max/now/valuetext/aria-label
 * （ui-spec §2.6.4 第 5 条）。
 *
 * 状态徽标文案：已完成 / 失败 / 运行中 / 已取消 / 已跳过 / 等待中；
 * 徽标 `title` 是枚举原名（`Completed`…）。状态→视觉映射见 ui-spec §4.10。
 */
export function ProgressBar({
  label = '转换进度',
  progress,
  speed,
  eta,
  currentTime = 0,
  totalDuration = 0,
  status = null,
  className,
}: ProgressBarProps) {
  const safeProgress = clampPercent(progress)
  const percentageText = formatPercent(safeProgress) // 固定 1 位小数 + %，如 85.5%
  /** 不变文化的 1 位小数文本（aria-valuenow 用，不带 % 号，与 ProgressBar.razor 一致） */
  const percentageValue = safeProgress.toFixed(1)
  const accessibleLabel = label.trim() === '' ? '转换进度' : label
  const hasStatus = status !== null && status !== undefined
  const statusText = hasStatus ? taskStatusText(status) : ''
  const ariaValueText = hasStatus ? `${accessibleLabel} ${percentageText}，${statusText}` : `${accessibleLabel} ${percentageText}`

  const hasSpeed = typeof speed === 'number' && Number.isFinite(speed) && speed > 0
  const hasEta = typeof eta === 'number' && Number.isFinite(eta) && eta > 0
  const showPair = currentTime > 0 && totalDuration > 0
  const showUsedOnly = totalDuration <= 0 && currentTime > 0

  return (
    <div className={cx('ffui-progress-component', className)}>
      <div className="ffui-progress-header">
        <span className="ffui-progress-label">{label}</span>
        <span className={cx('ffui-progress-percentage', percentageClass(status))}>{percentageText}</span>
      </div>

      <div
        className="ffui-progress-track"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Number(percentageValue)}
        aria-valuetext={ariaValueText}
        aria-label={accessibleLabel}
      >
        {/* 宽度用不变文化的数字串，避免写成 `width: 85,5%` 被浏览器丢弃 */}
        <div
          className={cx('ffui-progress-fill', fillClass(status, safeProgress))}
          style={{ width: `${safeProgress.toFixed(1)}%` }}
        />
      </div>

      <div className="ffui-progress-stats">
        {showPair ? (
          <span className="ffui-stat-item">
            <span className="ffui-stat-label">已用/总时长</span>
            <span className="ffui-stat-value">{formatDurationPair(currentTime, totalDuration)}</span>
          </span>
        ) : null}
        {showUsedOnly ? (
          <span className="ffui-stat-item">
            <span className="ffui-stat-label">已用时长</span>
            <span className="ffui-stat-value">{formatProgressTime(currentTime)}</span>
          </span>
        ) : null}
        {hasSpeed ? (
          <span className="ffui-stat-item">
            <span className="ffui-stat-label">速度</span>
            <span className="ffui-stat-value">{formatSpeed(speed)}</span>
          </span>
        ) : null}
        {hasEta ? (
          <span className="ffui-stat-item">
            <span className="ffui-stat-label">剩余时间</span>
            <span className="ffui-stat-value">{formatProgressTime(eta ?? 0)}</span>
          </span>
        ) : null}
        {hasStatus ? (
          <span className="ffui-stat-item status">
            <span
              className={cx('ffui-status-badge', fillClass(status, safeProgress))}
              title={status ?? undefined}
            >
              {statusText}
            </span>
          </span>
        ) : null}
      </div>
    </div>
  )
}
