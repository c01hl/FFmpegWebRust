import type { ReactNode } from 'react'
import { cx } from '../../lib/cx'

export interface FieldProps {
  /** 中文标签；想过 `<label>` 用 htmlFor 关联控件 */
  label?: ReactNode
  /** 控件的 id（关联 label） */
  htmlFor?: string
  /** 帮助文本（灰色小字） */
  help?: ReactNode
  /** 校验错误（红色小字）；有值时同时给控件加 aria-invalid 由调用方负责 */
  error?: ReactNode
  /** 必填标记（视觉上的 `*`，并加 aria 说明） */
  required?: boolean
  className?: string
  /** 控件本身 */
  children?: ReactNode
}

/**
 * 表单行排版容器：标签 / 控件 / 帮助 / 错误四段式（.ffui-field*）。
 * 隐藏型标签用 `<span className="visually-hidden">` 自己写，不要用 Field。
 */
export function Field({
  label,
  htmlFor,
  help,
  error,
  required = false,
  className,
  children,
}: FieldProps) {
  return (
    <div className={cx('ffui-field', className)}>
      {label !== undefined && label !== null ? (
        <label className="ffui-field-label" htmlFor={htmlFor}>
          {label}
          {required ? (
            <>
              {' '}
              <span aria-hidden="true" style={{ color: 'var(--danger-color)' }}>
                *
              </span>
              <span className="visually-hidden">（必填）</span>
            </>
          ) : null}
        </label>
      ) : null}
      {children}
      {help ? <div className="ffui-field-help">{help}</div> : null}
      {error ? (
        <div className="ffui-field-error" role="alert">
          {error}
        </div>
      ) : null}
    </div>
  )
}
