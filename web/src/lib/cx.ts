/**
 * 极小的 className 拼接工具：`cx('ffui-btn', active && 'active', props.className)`。
 * 只处理 false/null/undefined（不处理对象/数组，页面里用三元表达式足够）。
 */
export type ClassValue = string | false | null | undefined

export function cx(...values: ClassValue[]): string {
  let out = ''
  for (const value of values) {
    if (!value) continue
    out = out ? `${out} ${value}` : value
  }
  return out
}
