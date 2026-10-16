import type { HTMLAttributes, ReactNode } from 'react'
import { cx } from '../../lib/cx'

/** 提示条色调（对应 .ffui-alert 的修饰类） */
export type AlertTone = 'info' | 'success' | 'warning' | 'danger'

export interface AlertProps extends Omit<HTMLAttributes<HTMLDivElement>, 'className' | 'role'> {
  tone?: AlertTone
  /** 覆盖默认 role；默认 info/success 用 `status`，warning/danger 用 `alert` */
  role?: 'status' | 'alert' | 'note' | 'presentation' | 'none'
  /** 前置图标；不传则按 tone 给默认 emoji（作为 aria-hidden 渲染） */
  icon?: ReactNode
  className?: string
  children?: ReactNode
}

const TONE_CLASS: Record<AlertTone, string> = {
  info: 'ffui-alert-info',
  success: 'ffui-alert-success',
  warning: 'ffui-alert-warning',
  danger: 'ffui-alert-danger',
}

const DEFAULT_ICON: Record<AlertTone, string> = {
  info: 'ℹ️',
  success: '✅',
  warning: '⚠️',
  danger: '❌',
}

/** 内联提示条（.ffui-alert）。异步失败路径必须有可见反馈，优先用它或 Toast。 */
export function Alert({ tone = 'info', role, icon, className, children, ...rest }: AlertProps) {
  const resolvedRole = role ?? (tone === 'danger' || tone === 'warning' ? 'alert' : 'status')
  return (
    <div className={cx('ffui-alert', TONE_CLASS[tone], className)} role={resolvedRole} {...rest}>
      {icon === null ? null : (
        <span aria-hidden="true">{icon ?? DEFAULT_ICON[tone]}</span>
      )}
      <div>{children}</div>
    </div>
  )
}
