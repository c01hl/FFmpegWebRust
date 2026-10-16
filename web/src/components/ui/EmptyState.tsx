import type { ReactNode } from 'react'
import { cx } from '../../lib/cx'

export interface EmptyStateProps {
  /** 前置图标（emoji / svg），可选 */
  icon?: ReactNode
  className?: string
  /** 中文文案；常用原文见 ui-spec §12，例如「还没有任何任务，去「转换」页开始第一个吧。」 */
  children?: ReactNode
}

/** 空状态（.ffui-empty-state）。文案逐字来自 ui-spec，不要自己改写。 */
export function EmptyState({ icon, className, children }: EmptyStateProps) {
  return (
    <div className={cx('ffui-empty-state', className)}>
      {icon ? (
        <div aria-hidden="true" className="mb-1 text-2xl">
          {icon}
        </div>
      ) : null}
      {children}
    </div>
  )
}
