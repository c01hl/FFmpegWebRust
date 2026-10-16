import { useId } from 'react'
import type { ReactNode, SelectHTMLAttributes } from 'react'
import { cx } from '../../lib/cx'

/** 下拉选项 */
export interface SelectOption {
  value: string
  label: string
  /** 选项说明（作为 option 的 title） */
  description?: string
  disabled?: boolean
}

export interface SelectProps extends Omit<SelectHTMLAttributes<HTMLSelectElement>, 'className' | 'children'> {
  /** 可选项；也可以直接传 `<option>` 作为 children（两种都支持，二选一） */
  options?: SelectOption[]
  /** 无障碍名称；给了 label 时可省略 */
  label?: ReactNode
  /** 视觉隐藏的 label 文本（有 ariaLabel 时用） */
  ariaLabel?: string
  help?: ReactNode
  error?: ReactNode
  className?: string
  children?: ReactNode
}

/**
 * 下拉选择。`<select>` 视觉来自 index.css 的全局表单规则（--input-bg / --border-color / --focus-ring）。
 * 用法：
 *   <Select label="界面主题" value={mode} onChange={(e) => setMode(e.target.value)} options={THEME_OPTIONS} />
 */
export function Select({
  options,
  label,
  ariaLabel,
  help,
  error,
  className,
  children,
  id,
  ...rest
}: SelectProps) {
  const generatedId = useId()
  const selectId = id ?? generatedId
  const helpId = help ? `${selectId}-help` : undefined
  const errorId = error ? `${selectId}-error` : undefined

  return (
    <div className={cx('ffui-field', className)}>
      {label !== undefined && label !== null ? (
        <label className="ffui-field-label" htmlFor={selectId}>
          {label}
        </label>
      ) : null}
      <select
        id={selectId}
        aria-label={label === undefined || label === null ? ariaLabel : undefined}
        aria-describedby={[helpId, errorId].filter(Boolean).join(' ') || undefined}
        aria-invalid={error ? true : undefined}
        {...rest}
      >
        {children ??
          (options ?? []).map((option) => (
            <option key={option.value} value={option.value} title={option.description} disabled={option.disabled}>
              {option.label}
            </option>
          ))}
      </select>
      {help ? (
        <div className="ffui-field-help" id={helpId}>
          {help}
        </div>
      ) : null}
      {error ? (
        <div className="ffui-field-error" id={errorId} role="alert">
          {error}
        </div>
      ) : null}
    </div>
  )
}
