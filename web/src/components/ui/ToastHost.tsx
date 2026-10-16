import { TOAST_CLASS, selectToastMessages, useToastStore } from '../../store/toast'
import { cx } from '../../lib/cx'
import { IconButton } from './IconButton'

/**
 * 全局 Toast 容器（对应 ToastHost.razor）。**整个应用只能挂一个**（AppShell 里挂）。
 *
 * - `.ffui-toast-stack` 固定右下角，`z-index: 2000`；
 * - 容器 `role="status" aria-live="polite"`（提示会朗读，但不打断用户）；
 * - 自动消失由 store 里的定时器负责（Info/Success 4s、Warning 5s、Danger 8s，`durationMs<=0` 不消失）；
 * - 每条右侧有 `aria-label="关闭提示"` 的 ✕ 手动关闭；
 * - 最多同时 5 条，超出丢弃最旧的（store 里保证）。
 */
export function ToastHost() {
  const messages = useToastStore(selectToastMessages)
  const dismiss = useToastStore((state) => state.dismiss)

  if (messages.length === 0) return null

  return (
    <div className="ffui-toast-stack" role="status" aria-live="polite">
      {messages.map((message) => (
        <div key={message.id} className={cx('ffui-toast', TOAST_CLASS[message.level])}>
          <span aria-hidden="true">{message.icon}</span>
          <span className="ffui-toast-text">{message.text}</span>
          <IconButton
            ariaLabel="关闭提示"
            className="ffui-toast-close"
            onClick={() => dismiss(message.id)}
          >
            ✕
          </IconButton>
        </div>
      ))}
    </div>
  )
}
