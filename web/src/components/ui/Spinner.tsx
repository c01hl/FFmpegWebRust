import { cx } from '../../lib/cx'

export interface SpinnerProps {
  /** `sm` = 14×14（默认，对应 .spinner）；`lg` = 28×28（对应 .spinner-lg） */
  size?: 'sm' | 'lg'
  /** 无障碍文本；给了就输出 .visually-hidden 供屏幕阅读器朗读 */
  label?: string
  className?: string
}

/**
 * 加载指示器。**全局只有一份 @keyframes**（.spinner 定义在 index.css），
 * 组件内不要再写 `<style>` 定义动画。
 */
export function Spinner({ size = 'sm', label, className }: SpinnerProps) {
  return (
    <>
      <span className={cx('spinner', size === 'lg' && 'spinner-lg', className)} aria-hidden="true" />
      {label ? <span className="visually-hidden">{label}</span> : null}
    </>
  )
}
