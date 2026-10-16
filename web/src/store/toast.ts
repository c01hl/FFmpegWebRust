/**
 * 全局 Toast（zustand）。
 *
 * 对应参考实现 `Services/ToastService.cs` + `Components/Shared/ToastHost.razor`：
 * - 全局唯一，任何页面/异步任务都能推消息；
 * - 最多同时 5 条，超出丢弃**最旧**的；
 * - 文本会 `trim()`，空白文本直接忽略；
 * - 默认时长：Info/Success 4000ms、Warning 5000ms、Danger 8000ms；`durationMs <= 0` 表示不自动消失；
 * - 级别 → 图标：Info `ℹ️`、Success `✅`、Warning `⚠️`、Danger `❌`。
 *
 * 在组件里：
 *   const messages = useToastStore((s) => s.messages)   // ToastHost 用
 * 在非组件环境（api/ws 客户端等）：
 *   import { toast } from '../store/toast'; toast.error('…')
 */

import { create } from 'zustand'

/** 提示级别（与 ToastService.cs 的 ToastLevel 对应） */
export type ToastLevel = 'info' | 'success' | 'warning' | 'danger'

/** 一条提示 */
export interface ToastMessage {
  id: string
  /** 已 trim 的展示文本 */
  text: string
  level: ToastLevel
  /** 级别图标（aria-hidden，旁边是文本） */
  icon: string
  /** 实际设置的时长（0 = 不自动消失） */
  durationMs: number
  createdAt: number
}

/** 各级别默认时长（ToastService.cs） */
export const TOAST_DURATIONS: Record<ToastLevel, number> = {
  info: 4000,
  success: 4000,
  warning: 5000,
  danger: 8000,
}

/** 级别图标 */
export const TOAST_ICONS: Record<ToastLevel, string> = {
  info: 'ℹ️',
  success: '✅',
  warning: '⚠️',
  danger: '❌',
}

/** 级别 → 边框修饰类 */
export const TOAST_CLASS: Record<ToastLevel, string> = {
  info: '',
  success: 'ffui-toast-success',
  warning: 'ffui-toast-warning',
  danger: 'ffui-toast-danger',
}

/** 同时显示的最大条数 */
export const MAX_TOASTS = 5

export interface ToastState {
  messages: ToastMessage[]
  /** 通用入口；`durationMs` 省略时用级别的默认时长 */
  show: (text: string, level?: ToastLevel, durationMs?: number) => string
  success: (text: string, durationMs?: number) => string
  warning: (text: string, durationMs?: number) => string
  /** 错误提示（Danger，默认停留更久） */
  error: (text: string, durationMs?: number) => string
  dismiss: (id: string) => void
  clear: () => void
}

let toastSeq = 0

/** 定时器表：dismiss / clear 时取消，避免内存泄漏 */
const timers = new Map<string, ReturnType<typeof setTimeout>>()

function cancelTimer(id: string): void {
  const timer = timers.get(id)
  if (timer !== undefined) {
    clearTimeout(timer)
    timers.delete(id)
  }
}

export const useToastStore = create<ToastState>((set, get) => ({
  messages: [],

  show: (text, level = 'info', durationMs) => {
    const value = typeof text === 'string' ? text.trim() : ''
    const id = `toast-${Date.now().toString(36)}-${(toastSeq += 1).toString(36)}`
    if (!value) return id // 空白文本直接忽略（ToastService.cs 的行为）

    const duration = durationMs ?? TOAST_DURATIONS[level]

    // 同文案 + 同级别的提示已经在屏幕上时：只「续期」，不再堆一条。
    // 例如后端未启动时，AppShell / 首页 / 转换页会各自报一次同样的失败，
    // 没有这条规则就会在右下角刷出一排一模一样的提示。
    const existing = get().messages.find((message) => message.text === value && message.level === level)
    if (existing) {
      cancelTimer(existing.id)
      set((state) => ({
        messages: state.messages.map((message) =>
          message.id === existing.id ? { ...message, durationMs: duration, createdAt: Date.now() } : message,
        ),
      }))
      if (duration > 0) {
        timers.set(
          existing.id,
          setTimeout(() => {
            get().dismiss(existing.id)
          }, duration),
        )
      }
      return existing.id
    }

    const message: ToastMessage = {
      id,
      text: value,
      level,
      icon: TOAST_ICONS[level],
      durationMs: duration,
      createdAt: Date.now(),
    }

    set((state) => {
      const next = [...state.messages, message]
      // 超出上限时丢弃最旧的，并清掉它的定时器
      while (next.length > MAX_TOASTS) {
        const dropped = next.shift()
        if (dropped) cancelTimer(dropped.id)
      }
      return { messages: next }
    })

    if (duration > 0) {
      timers.set(
        id,
        setTimeout(() => {
          get().dismiss(id)
        }, duration),
      )
    }
    return id
  },

  success: (text, durationMs) => get().show(text, 'success', durationMs),
  warning: (text, durationMs) => get().show(text, 'warning', durationMs),
  error: (text, durationMs) => get().show(text, 'danger', durationMs),

  dismiss: (id) => {
    cancelTimer(id)
    set((state) => ({ messages: state.messages.filter((message) => message.id !== id) }))
  },

  clear: () => {
    for (const id of timers.keys()) cancelTimer(id)
    set({ messages: [] })
  },
}))

/**
 * 非组件环境的调用入口（api/ws/lib 都从这里推消息）。
 * 组件里也可以用，但不参与订阅渲染；需要渲染请用 `useToastStore`。
 */
export const toast = {
  show: (text: string, level: ToastLevel = 'info', durationMs?: number) =>
    useToastStore.getState().show(text, level, durationMs),
  success: (text: string, durationMs?: number) => useToastStore.getState().success(text, durationMs),
  warning: (text: string, durationMs?: number) => useToastStore.getState().warning(text, durationMs),
  error: (text: string, durationMs?: number) => useToastStore.getState().error(text, durationMs),
  dismiss: (id: string) => useToastStore.getState().dismiss(id),
  clear: () => useToastStore.getState().clear(),
}

/** ToastHost 的订阅选择器（避免整个 store 变化都重渲染） */
export const selectToastMessages = (state: ToastState): ToastMessage[] => state.messages
