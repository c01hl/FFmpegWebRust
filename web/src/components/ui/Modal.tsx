import { useCallback, useEffect, useId, useRef } from 'react'
import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from 'react'
import { cx } from '../../lib/cx'
import { IconButton } from './IconButton'

/** 尺寸（对应 .ffui-modal-sm/md/lg/xl） */
export type ModalSize = 'sm' | 'md' | 'lg' | 'xl'

export interface ModalProps {
  /** 标题（渲染在 .ffui-modal-header 的 `<h2>` 里，同时作为 aria-labelledby 的目标） */
  title: ReactNode
  open: boolean
  /** 请求切换开合状态（Esc、遮罩点击、右上角 ✕ 都走它） */
  onOpenChange: (open: boolean) => void
  /** 默认 `md` */
  size?: ModalSize
  /** 点击遮罩是否关闭，默认 true */
  closeOnBackdrop?: boolean
  /** 禁用一切关闭路径（如正在保存），默认 false */
  disableClose?: boolean
  className?: string
  /** 内容；需要底部按钮栏时在最后放 `<div className="ffui-modal-footer">…</div>` */
  children?: ReactNode
}

const SIZE_CLASS: Record<ModalSize, string> = {
  sm: 'ffui-modal-sm',
  md: 'ffui-modal-md',
  lg: 'ffui-modal-lg',
  xl: 'ffui-modal-xl',
}

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',')

/**
 * 打开中的模态栈（**后开先处理**）。
 * 参考实现里 Esc 是「取 document.querySelectorAll('.ffui-modal') 的最后一个、点它的关闭按钮」，
 * 这里用显式的栈实现同样语义：只有最上层弹窗响应 Esc。
 */
const modalStack: HTMLElement[] = []

function focusableIn(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (element) => element.offsetParent !== null || element === document.activeElement,
  )
}

/**
 * 通用模态框（对应 app.css 的 .ffui-modal 家族）。
 *
 * 相对参考实现**补上了三件事**（ui-spec §2.6.1 末尾明确要求）：
 * 1. **焦点陷阱**：Tab / Shift+Tab 在弹窗内循环，不会跑到背后的页面；
 * 2. **关闭后归还焦点**：打开时记住 `document.activeElement`，关闭时还回去；
 * 3. **只有最上层弹窗响应 Esc**（打开时把焦点移进对话框，键盘用户不会落到背景）。
 *
 * 结构：backdrop(role=presentation) > dialog(role=dialog aria-modal=true aria-labelledby) >
 *       header(标题 + 关闭按钮) + body(.ffui-modal-body)
 */
export function Modal({
  title,
  open,
  onOpenChange,
  size = 'md',
  closeOnBackdrop = true,
  disableClose = false,
  className,
  children,
}: ModalProps) {
  const titleId = useId()
  const dialogRef = useRef<HTMLDivElement | null>(null)
  const previouslyFocused = useRef<HTMLElement | null>(null)

  const requestClose = useCallback(() => {
    if (disableClose) return
    onOpenChange(false)
  }, [disableClose, onOpenChange])

  // 打开：入栈、记住原焦点、把焦点移进对话框；关闭：出栈、归还焦点
  useEffect(() => {
    if (!open) return
    const dialog = dialogRef.current
    if (!dialog) return
    previouslyFocused.current = document.activeElement as HTMLElement | null
    modalStack.push(dialog)

    // 优先聚焦第一个可聚焦元素（例如关闭按钮），否则聚焦对话框本身
    const first = focusableIn(dialog)[0]
    if (first) first.focus()
    else dialog.focus()

    return () => {
      const index = modalStack.lastIndexOf(dialog)
      if (index >= 0) modalStack.splice(index, 1)
      const previous = previouslyFocused.current
      previouslyFocused.current = null
      if (previous && document.contains(previous) && typeof previous.focus === 'function') {
        previous.focus()
      }
    }
  }, [open])

  // Esc 关闭最上层；Tab 在弹窗内循环（焦点陷阱）
  useEffect(() => {
    if (!open) return
    const onKeyDown = (event: KeyboardEvent) => {
      const dialog = dialogRef.current
      if (!dialog) return
      // 只管最上层：本元素必须是栈顶
      if (modalStack[modalStack.length - 1] !== dialog) return

      if (event.key === 'Escape') {
        if (disableClose) return
        event.preventDefault()
        event.stopPropagation()
        onOpenChange(false)
        return
      }

      if (event.key !== 'Tab') return
      const focusables = focusableIn(dialog)
      if (focusables.length === 0) {
        event.preventDefault()
        dialog.focus()
        return
      }
      const first = focusables[0]
      const last = focusables[focusables.length - 1]
      const active = document.activeElement as HTMLElement | null
      if (event.shiftKey) {
        if (active === first || !dialog.contains(active)) {
          event.preventDefault()
          last.focus()
        }
      } else if (active === last || !dialog.contains(active)) {
        event.preventDefault()
        first.focus()
      }
    }

    document.addEventListener('keydown', onKeyDown, true)
    return () => document.removeEventListener('keydown', onKeyDown, true)
  }, [open, disableClose, onOpenChange])

  const onBackdropClick = () => {
    if (closeOnBackdrop && !disableClose) onOpenChange(false)
  }

  const onDialogKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    // 拦掉冒泡，避免弹窗内的按键被背景页面的全局监听处理
    event.stopPropagation()
  }

  if (!open) return null

  return (
    <div className="ffui-modal-backdrop" role="presentation" onClick={onBackdropClick}>
      <div
        ref={dialogRef}
        className={cx('ffui-modal', SIZE_CLASS[size], className)}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onClick={(event) => event.stopPropagation()}
        onKeyDown={onDialogKeyDown}
      >
        <div className="ffui-modal-header">
          <h2 id={titleId}>{title}</h2>
          <IconButton ariaLabel="关闭对话框" disabled={disableClose} onClick={requestClose}>
            ✕
          </IconButton>
        </div>
        <div className="ffui-modal-body">{children}</div>
      </div>
    </div>
  )
}
