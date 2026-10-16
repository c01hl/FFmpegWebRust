import type { ButtonHTMLAttributes, ReactNode } from 'react'
import { cx } from '../../lib/cx'

export interface IconButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'className'> {
  /**
   * **必填**的无障碍名称（ui-spec §2.6.4：所有图标按钮必须有 aria-label）。
   * 文案示例：`关闭对话框`、`关闭提示`、`复制日志`、`下载日志`、`清空显示的日志`、
   * `移除 {文件名}`、`查看模板详情`、`复制命令参数`。
   */
  ariaLabel: string
  /** 悬浮提示，缺省时用 ariaLabel */
  title?: string
  /** 图标（emoji 或 svg），会作为 aria-hidden 处理 */
  children: ReactNode
  className?: string
}

/** 图标按钮（.ffui-icon-btn）。`ariaLabel` 必填，从类型层面杜绝「只有图标没有名称」。 */
export function IconButton({
  ariaLabel,
  title,
  children,
  className,
  type = 'button',
  ...rest
}: IconButtonProps) {
  return (
    <button
      type={type}
      className={cx('ffui-icon-btn', className)}
      aria-label={ariaLabel}
      title={title ?? ariaLabel}
      {...rest}
    >
      <span aria-hidden="true">{children}</span>
    </button>
  )
}
