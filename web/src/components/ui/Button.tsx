import type { ButtonHTMLAttributes, ReactNode } from 'react'
import { cx } from '../../lib/cx'
import { Spinner } from './Spinner'

/** 视觉变体（对应 .ffui-btn 的修饰类） */
export type ButtonVariant = 'default' | 'primary' | 'danger' | 'ghost'
/** 尺寸（md = 默认，无修饰类） */
export type ButtonSize = 'sm' | 'md' | 'lg'

export interface ButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'className'> {
  variant?: ButtonVariant
  size?: ButtonSize
  /** 加载中：禁用按钮并在文字前放一个 .spinner */
  loading?: boolean
  /** 前置图标（emoji 或 svg） */
  icon?: ReactNode
  className?: string
  children?: ReactNode
}

const VARIANT_CLASS: Record<ButtonVariant, string> = {
  default: '',
  primary: 'ffui-btn-primary',
  danger: 'ffui-btn-danger',
  ghost: 'ffui-btn-ghost',
}

const SIZE_CLASS: Record<ButtonSize, string> = {
  sm: 'ffui-btn-sm',
  md: '',
  lg: 'ffui-btn-lg',
}

/**
 * 通用按钮（.ffui-btn 家族）。
 * 默认 `type="button"`，避免在表单里意外触发提交。
 */
export function Button({
  variant = 'default',
  size = 'md',
  loading = false,
  icon,
  className,
  children,
  disabled,
  type = 'button',
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      className={cx('ffui-btn', VARIANT_CLASS[variant], SIZE_CLASS[size], className)}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      {...rest}
    >
      {loading ? <Spinner /> : icon}
      {children}
    </button>
  )
}
