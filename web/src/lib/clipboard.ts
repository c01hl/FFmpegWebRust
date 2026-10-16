/**
 * 复制到剪贴板（对应 ui-spec §5.5「统一实现约定」与 app.js:43-71）。
 *
 * 约定：
 * 1. 入参 null/undefined → 直接返回 false；
 * 2. 优先 `navigator.clipboard.writeText`，且要求 `window.isSecureContext`；
 * 3. 失败或不可用 → 回退到隐藏 `<textarea readonly>` + `select()` + `execCommand('copy')`；
 * 4. 全过程用布尔值表达成功/失败，**永不抛异常**；
 * 5. 上层按布尔值给「已复制」或「复制失败」Toast，且每条失败文案都要提示用户可手动选中文本复制。
 *
 * 五处失败文案是 ui-spec 里逐字指定的（不同入口用词不同，不要统一成一句）。
 */

import { toast } from '../store/toast'

/** 失败文案：与 ui-spec §5.1 / §5.3 / §5.4 / §3.2 逐字对应 */
export const COPY_FAILURE_MESSAGES = {
  /** §5.1 转换页「完整命令」 */
  command: '复制失败：浏览器不允许访问剪贴板。请手动选中命令文本后复制。',
  /** §5.3 模板编辑器命令预览 */
  templateEditor: '复制失败：浏览器不允许访问剪贴板，请手动选中命令文本后复制。',
  /** §5.4 模板卡片 / 模板详情的「复制命令参数」 */
  commandArgs: '复制失败：浏览器拒绝了剪贴板访问，请手动选择命令文本',
  /** §5.4 任务列表的「复制命令」 */
  taskCommand: '复制失败：浏览器不允许访问剪贴板，请在详情里手动选中文本。',
  /** §3.2 LogViewer 的「复制日志」 */
  log: '复制失败，可能是浏览器不允许访问剪贴板（非安全来源）。可手动选中文本复制。',
} as const

export type CopyFailureMessageKey = keyof typeof COPY_FAILURE_MESSAGES

/** 失败原因（仅用于日志/埋点，用户可见文案来自 COPY_FAILURE_MESSAGES 或调用方传入） */
export type CopyFailureKind = 'empty' | 'insecure-context' | 'api-unavailable' | 'denied' | 'exception' | 'exec-command-failed'

/** 上一次复制的结果（便于调用方打点或调试；不参与渲染） */
export interface CopyResult {
  ok: boolean
  /** 仅在 ok=false 时有值 */
  failure?: CopyFailureKind
  /** 仅在 ok=false 时有值：异常消息（中文提示里可拼接） */
  message?: string
}

/**
 * 复制文本。返回布尔值，不抛异常。
 * `null` / `undefined` / `''`（去空白后为空）都视为「没有内容」→ false。
 */
export async function copyText(text: string | null | undefined): Promise<boolean> {
  return (await copyTextDetailed(text)).ok
}

/** 与 `copyText` 相同，但额外返回失败原因，便于上层拼更细的文案 */
export async function copyTextDetailed(text: string | null | undefined): Promise<CopyResult> {
  if (text === null || text === undefined) return { ok: false, failure: 'empty' }
  const value = String(text)
  if (value.trim() === '') return { ok: false, failure: 'empty' }

  const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
  const secure = typeof window !== 'undefined' && window.isSecureContext === true

  if (clipboard && typeof clipboard.writeText === 'function') {
    if (!secure) {
      // 非安全来源下 clipboard API 通常直接抛 NotAllowedError，先走回退更稳
      const fallback = legacyCopy(value)
      if (fallback.ok) return fallback
      return { ok: false, failure: 'insecure-context' }
    }
    try {
      await clipboard.writeText(value)
      return { ok: true }
    } catch (error) {
      const fallback = legacyCopy(value)
      if (fallback.ok) return fallback
      return { ok: false, failure: 'denied', message: describe(error) }
    }
  }

  return legacyCopy(value)
}

/** 隐藏 textarea + execCommand('copy') 回退实现 */
function legacyCopy(value: string): CopyResult {
  if (typeof document === 'undefined') return { ok: false, failure: 'api-unavailable' }
  let textarea: HTMLTextAreaElement | null = null
  try {
    textarea = document.createElement('textarea')
    textarea.value = value
    textarea.setAttribute('readonly', 'readonly')
    // 放到视口外，避免页面跳动；仍然可见（display:none 在部分浏览器里无法选中）
    textarea.style.position = 'fixed'
    textarea.style.top = '0'
    textarea.style.left = '-9999px'
    textarea.style.opacity = '0'
    document.body.appendChild(textarea)
    textarea.focus()
    textarea.select()
    textarea.setSelectionRange(0, value.length)
    const ok = typeof document.execCommand === 'function' && document.execCommand('copy')
    return ok ? { ok: true } : { ok: false, failure: 'exec-command-failed' }
  } catch (error) {
    return { ok: false, failure: 'exception', message: describe(error) }
  } finally {
    if (textarea && textarea.parentNode) textarea.parentNode.removeChild(textarea)
  }
}

function describe(error: unknown): string {
  if (error instanceof Error && error.message) return error.message
  return String(error)
}

/** `copyAndToast` 的选项：所有文案都可覆盖，默认用 §5.1 的文案 */
export interface CopyAndToastOptions {
  /** 成功文案，默认 `已复制到剪贴板` */
  successText?: string
  /** 内容为空时的警告文案，默认 `没有可复制的内容。` */
  emptyText?: string
  /** 失败文案，默认 `COPY_FAILURE_MESSAGES.command`；也可传 `COPY_FAILURE_MESSAGES.log` 等 */
  failureText?: string
  /** 失败文案里是否拼接异常消息（形如 `复制失败：{异常消息}`） */
  includeErrorMessage?: boolean
}

/**
 * 复制并弹 Toast，返回是否成功。
 * 复制内容为空 → Warning；失败 → Danger；成功 → Success。
 */
export async function copyAndToast(
  text: string | null | undefined,
  options: CopyAndToastOptions = {},
): Promise<boolean> {
  const {
    successText = '已复制到剪贴板',
    emptyText = '没有可复制的内容。',
    failureText = COPY_FAILURE_MESSAGES.command,
    includeErrorMessage = false,
  } = options

  const result = await copyTextDetailed(text)
  if (result.ok) {
    toast.success(successText)
    return true
  }

  if (result.failure === 'empty') {
    toast.warning(emptyText)
    return false
  }

  toast.error(
    includeErrorMessage && result.message ? `复制失败：${result.message}` : failureText,
  )
  return false
}
