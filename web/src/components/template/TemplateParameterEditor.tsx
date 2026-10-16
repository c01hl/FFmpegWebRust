/**
 * 模板参数控件自动生成器（对应 ui-spec §3.5 `TemplateParameterEditor.razor`）。
 *
 * 生成规则（逐字照抄规格）：
 * - 容器 `.param-grid`：`repeat(auto-fit, minmax(210px, 1fr))`，gap `.85rem`；
 * - 按 `template.parameters` 顺序渲染，跳过 Name 为空的项；
 * - `Number` 且 Min/Max 至少有一个时，标签后追加范围提示 `（{Min ?? '—'} ~ {Max ?? '∞'}）`；
 * - `Select` → `<select>`，选中项 Description 非空时显示在控件下方；
 * - `Number` → `<input type="number" step="any">`，带 min/max；
 * - `Boolean` → 复选框 + `已启用` / `已关闭`，勾选写 TrueValue、取消写 FalseValue；
 * - `Text`（默认）→ `<input type="text">`，placeholder = DefaultValue；
 * - 控件 id 规则：`param-{name}`（空格换成下划线）；
 * - 取值：`values[name]` 存在就用它，否则回退 `defaultValue`；
 * - **回写绝不修改传入的字典**，而是复制一份后回传。
 */

import type { TemplateParameter, CommandTemplate } from '../../api/types'
import { toFixedTrim } from '../../lib/format'

export interface TemplateParameterEditorProps {
  /** 可空；null 或没有可见参数时不渲染任何内容 */
  template: CommandTemplate | null | undefined
  /** 当前参数值（**只读**，回调里给新对象） */
  values: Record<string, string>
  onChange: (values: Record<string, string>) => void
  /** 禁用所有控件（启动中 / 批量处理进行中） */
  disabled?: boolean
}

/** 控件 id：`param-{name}`，空格换成下划线（与参考实现一致） */
function controlId(parameter: TemplateParameter): string {
  return `param-${parameter.name}`.replace(/ /g, '_')
}

/** 真值判定：兼容 TrueValue（忽略大小写）以及 1/true/True/yes/on */
function isBooleanTrue(parameter: TemplateParameter, value: string): boolean {
  if (!value) return false
  if (value.toLowerCase() === (parameter.trueValue ?? '').toLowerCase()) return true
  return value === '1' || value.toLowerCase() === 'true' || value.toLowerCase() === 'yes' || value.toLowerCase() === 'on'
}

/** 范围提示：Min/Max 用 `0.##` 格式，缺失用 `—` / `∞` */
function rangeText(parameter: TemplateParameter): string | null {
  if (parameter.kind !== 'Number') return null
  if (parameter.min === null && parameter.max === null) return null
  const min = parameter.min === null ? '—' : toFixedTrim(parameter.min, 2)
  const max = parameter.max === null ? '∞' : toFixedTrim(parameter.max, 2)
  return `（${min} ~ ${max}）`
}

export function TemplateParameterEditor({
  template,
  values,
  onChange,
  disabled = false,
}: TemplateParameterEditorProps) {
  const visible = (template?.parameters ?? []).filter((parameter) => (parameter.name ?? '').trim() !== '')
  if (!template || visible.length === 0) return null

  const getValue = (parameter: TemplateParameter): string => {
    const value = values[parameter.name]
    return value === undefined || value === null ? (parameter.defaultValue ?? '') : value
  }

  /** 回写规则：复制一份字典再改键，绝不就地修改（否则父组件的参数变更可能被忽略） */
  const setValue = (parameter: TemplateParameter, rawValue: string) => {
    onChange({ ...values, [parameter.name]: rawValue })
  }

  return (
    <div className="grid gap-3.5" style={{ gridTemplateColumns: 'repeat(auto-fit, minmax(210px, 1fr))' }}>
      {visible.map((parameter) => {
        const value = getValue(parameter)
        const range = rangeText(parameter)
        const id = controlId(parameter)
        const optionDescription =
          parameter.kind === 'Select'
            ? (parameter.options.find((option) => option.value === value)?.description ?? '')
            : ''

        return (
          <div key={parameter.name} className="flex flex-col gap-1 min-w-0">
            <label
              className="flex items-baseline gap-1.5 font-medium text-secondary-text"
              style={{ fontSize: '0.85rem' }}
              htmlFor={parameter.kind === 'Boolean' ? `${id}-switch` : id}
            >
              {parameter.label || parameter.name}
              {range ? (
                <span className="text-muted-text" style={{ fontSize: '0.75rem', fontWeight: 400 }}>
                  {range}
                </span>
              ) : null}
            </label>

            {parameter.kind === 'Select' ? (
              <select
                id={id}
               
                value={value}
                disabled={disabled}
                onChange={(event) => setValue(parameter, event.target.value)}
              >
                {parameter.options.map((option) => (
                  <option key={option.value} value={option.value} title={option.description}>
                    {option.label}
                  </option>
                ))}
              </select>
            ) : parameter.kind === 'Number' ? (
              <input
                id={id}
               
                type="number"
                step="any"
                value={value}
                disabled={disabled}
                min={parameter.min ?? undefined}
                max={parameter.max ?? undefined}
                onChange={(event) => setValue(parameter, event.target.value)}
              />
            ) : parameter.kind === 'Boolean' ? (
              <label className="inline-flex items-center gap-1.5 text-secondary-text" style={{ fontSize: '0.88rem' }}>
                <input
                  id={`${id}-switch`}
                  type="checkbox"
                  disabled={disabled}
                  checked={isBooleanTrue(parameter, value)}
                  onChange={(event) => setValue(parameter, event.target.checked ? parameter.trueValue : parameter.falseValue)}
                />
                <span>{isBooleanTrue(parameter, value) ? '已启用' : '已关闭'}</span>
              </label>
            ) : (
              <input
                id={id}
               
                type="text"
                value={value}
                disabled={disabled}
                placeholder={parameter.defaultValue}
                onChange={(event) => setValue(parameter, event.target.value)}
              />
            )}

            {parameter.kind === 'Select'
              ? optionDescription
                ? (
                    <span className="text-muted-text" style={{ fontSize: '0.75rem' }}>
                      {optionDescription}
                    </span>
                  )
                : null
              : parameter.description
                ? (
                    <span className="text-muted-text" style={{ fontSize: '0.75rem' }}>
                      {parameter.description}
                    </span>
                  )
                : null}
          </div>
        )
      })}
    </div>
  )
}
