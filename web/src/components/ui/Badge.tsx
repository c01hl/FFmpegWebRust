import type { ReactNode } from 'react'
import { cx } from '../../lib/cx'

/** 徽标色调（对应 .ffui-badge 的修饰类） */
export type BadgeTone = 'default' | 'primary' | 'success' | 'danger' | 'warning' | 'info'

export interface BadgeProps {
  tone?: BadgeTone
  /** 前置图标（emoji / svg） */
  icon?: ReactNode
  /** 无障碍说明，例如 `状态：可用` */
  ariaLabel?: string
  title?: string
  className?: string
  children?: ReactNode
}

const TONE_CLASS: Record<BadgeTone, string> = {
  default: '',
  primary: 'ffui-badge-primary',
  success: 'ffui-badge-success',
  danger: 'ffui-badge-danger',
  warning: 'ffui-badge-warning',
  info: 'ffui-badge-info',
}

/** 胶囊徽标（.ffui-badge）。 */
export function Badge({ tone = 'default', icon, ariaLabel, title, className, children }: BadgeProps) {
  return (
    <span
      className={cx('ffui-badge', TONE_CLASS[tone], className)}
      aria-label={ariaLabel}
      title={title}
    >
      {icon ? <span aria-hidden="true">{icon}</span> : null}
      {children}
    </span>
  )
}
