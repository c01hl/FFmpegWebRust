import type { ReactNode } from 'react'
import { Button } from './Button'
import { Modal } from './Modal'
import { Spinner } from './Spinner'

export interface ConfirmDialogProps {
  open: boolean
  /** 状态回传；取消/关闭（含 Esc、遮罩）都会触发它并调用 onCancel */
  onOpenChange: (open: boolean) => void
  /** 默认 `确认操作` */
  title?: ReactNode
  /** 正文（主文案） */
  message: ReactNode
  /** 补充说明，例如完整路径（灰色小字、可换行） */
  detail?: ReactNode
  /** 默认 `确定` */
  confirmText?: string
  /** 默认 `取消` */
  cancelText?: string
  /** 危险操作：图标变 ⚠️、确认按钮变红 */
  danger?: boolean
  /** 处理中：两个按钮都禁用，确认按钮内前置 .spinner，且遮罩/✕ 不可关闭 */
  busy?: boolean
  onConfirm: () => void
  /** 取消回调（Esc / 遮罩 / 关闭按钮 / 取消按钮都会触发） */
  onCancel?: () => void
}

/**
 * 确认框（对应 ConfirmDialog.razor）。
 * 内部已经包含 `<Modal size="sm">`，**调用方不要再套一层 Modal**。
 *
 * 行为细节（与参考实现一致）：
 * - `IsBusy` 时 `CloseOnBackdrop=false`、`DisableClose=true`；
 * - 关闭（含 Esc、点遮罩）后固定触发 `onCancel`；
 * - 默认文案：标题 `确认操作`、确认 `确定`、取消 `取消`；图标 Danger → `⚠️`，否则 `❓`。
 */
export function ConfirmDialog({
  open,
  onOpenChange,
  title = '确认操作',
  message,
  detail,
  confirmText = '确定',
  cancelText = '取消',
  danger = false,
  busy = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const handleOpenChange = (next: boolean) => {
    if (!next && busy) return // 处理中不允许关闭
    onOpenChange(next)
    if (!next) onCancel?.()
  }

  const handleCancel = () => {
    if (busy) return
    onOpenChange(false)
    onCancel?.()
  }

  return (
    <Modal
      open={open}
      onOpenChange={handleOpenChange}
      title={title}
      size="sm"
      closeOnBackdrop={!busy}
      disableClose={busy}
    >
      <div className="ffui-confirm-body">
        <div className="ffui-confirm-icon" aria-hidden="true">
          {danger ? '⚠️' : '❓'}
        </div>
        <div className="ffui-confirm-text">
          <p className="ffui-confirm-message">{message}</p>
          {detail ? <p className="ffui-confirm-detail">{detail}</p> : null}
        </div>
      </div>

      <div className="ffui-modal-footer">
        <Button onClick={handleCancel} disabled={busy}>
          {cancelText}
        </Button>
        <Button variant={danger ? 'danger' : 'primary'} onClick={onConfirm} disabled={busy}>
          {busy ? <Spinner /> : null}
          {confirmText}
        </Button>
      </div>
    </Modal>
  )
}
