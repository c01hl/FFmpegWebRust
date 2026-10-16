/**
 * REST 客户端：fetch 封装（JSON、错误体解析、超时、AbortSignal）。
 *
 * 契约要点（docs/api-contract.md §0）：
 * - 前缀 `/api`（同源，开发期由 Vite 代理到 127.0.0.1:5266）；
 * - 非 2xx 的响应体形如 `{"error":{"code":"TEMPLATE_NOT_FOUND","message":"模板不存在或已被删除"}}`，
 *   统一解析成抛出的 `ApiError`；
 * - 默认超时 30s；模板试运行 / 硬件检测等慢端点用 `LONG_TIMEOUT_MS`（120s），
 *   调用处通过 `timeoutMs` 覆盖。
 *
 * 失败语义：
 * - HTTP 非 2xx        → `ApiError(code, message, status)`
 * - 网络不可达 / DNS   → `ApiError('NETWORK_ERROR', '无法连接到服务…')`
 * - 超时               → `ApiError('TIMEOUT', '请求超时（{n} 秒）：{path}')`
 * - 调用方主动 abort   → `ApiError('ABORTED', '请求已取消')`
 * - 响应不是 JSON      → `ApiError('INVALID_RESPONSE', …)`
 *
 * 页面侧统一用 `getErrorMessage(error)`（或 `reportApiError(error, prefix)` 弹 Toast）
 * 处理失败，**不要**出现未捕获的 Promise rejection。
 */

import type { ApiErrorBody } from './types'
import { toast } from '../store/toast'

/** API 前缀（同源） */
export const API_BASE = '/api'
/** 默认超时 30 秒 */
export const DEFAULT_TIMEOUT_MS = 30_000
/** 慢端点超时 120 秒（模板试运行、硬件检测、扫描大目录） */
export const LONG_TIMEOUT_MS = 120_000

/** 统一的 API 错误 */
export class ApiError extends Error {
  /** HTTP 状态码；网络/超时类错误的 status 是 0 */
  readonly status: number
  /** 契约里的错误码，或本地错误码（NETWORK_ERROR / TIMEOUT / ABORTED / INVALID_RESPONSE） */
  readonly code: string
  /** 附加信息（超时毫秒数、原始响应文本等），便于排查 */
  readonly detail?: string

  constructor(message: string, code = 'UNKNOWN', status = 0, detail?: string) {
    super(message)
    this.name = 'ApiError'
    this.code = code
    this.status = status
    this.detail = detail
  }
}

/** 单次请求的选项 */
export interface RequestOptions {
  method?: 'GET' | 'POST' | 'PUT' | 'DELETE' | 'PATCH'
  /** 会被 JSON.stringify；不要传 FormData */
  body?: unknown
  /** 查询参数；undefined / null / '' 的键会被忽略 */
  query?: Record<string, string | number | boolean | undefined | null>
  /** 超时毫秒数，默认 DEFAULT_TIMEOUT_MS */
  timeoutMs?: number
  /** 外部取消信号；与内部超时信号会合并 */
  signal?: AbortSignal
  /** 额外请求头 */
  headers?: Record<string, string>
  /** 是否保持 `?` 查询里的数组展开（默认 `key=1&key=2`） */
  credentials?: RequestCredentials
}

type QueryValue = string | number | boolean | undefined | null

/** 拼查询串；跳过 undefined/null/''；数组与 `query` 的值为数组时展开成重复键 */
function buildQuery(query?: Record<string, QueryValue | QueryValue[]>): string {
  if (!query) return ''
  const parts: string[] = []
  for (const [key, raw] of Object.entries(query)) {
    const values = Array.isArray(raw) ? raw : [raw]
    for (const value of values) {
      if (value === undefined || value === null || value === '') continue
      parts.push(`${encodeURIComponent(key)}=${encodeURIComponent(String(value))}`)
    }
  }
  return parts.length > 0 ? `?${parts.join('&')}` : ''
}

/**
 * 给任意 Promise 加超时。
 * 超时会 reject 一个 `ApiError('TIMEOUT')`；原 Promise 的后续结果被忽略。
 */
export function withTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number,
  message = `请求超时（${Math.round(timeoutMs / 1000)} 秒）`,
): Promise<T> {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) return promise
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new ApiError(message, 'TIMEOUT'))
    }, timeoutMs)
    promise.then(
      (value) => {
        clearTimeout(timer)
        resolve(value)
      },
      (error) => {
        clearTimeout(timer)
        reject(error)
      },
    )
  })
}

async function parseErrorBody(response: Response): Promise<ApiError> {
  const text = await response.text().catch(() => '')
  if (text) {
    try {
      const parsed = JSON.parse(text) as Partial<ApiErrorBody>
      const code = parsed?.error?.code
      const message = parsed?.error?.message
      if (typeof message === 'string' && message) {
        return new ApiError(message, typeof code === 'string' && code ? code : 'HTTP_ERROR', response.status)
      }
    } catch {
      // 非 JSON 的错误体：退回通用文案，把原文塞进 detail
    }
  }
  return new ApiError(`请求失败（HTTP ${response.status}）`, 'HTTP_ERROR', response.status, text || undefined)
}

/**
 * 发一个请求并解析 JSON。
 * - 204 / 空响应体 → 返回 `undefined as T`（调用方按 `void` 使用）
 * - 错误统一抛 `ApiError`
 */
export async function apiRequest<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const {
    method = 'GET',
    body,
    query,
    timeoutMs = DEFAULT_TIMEOUT_MS,
    signal,
    headers,
    credentials = 'same-origin',
  } = options

  const url = `${API_BASE}${path}${buildQuery(query)}`
  const controller = new AbortController()
  const abortByCaller = () => controller.abort()
  if (signal) {
    if (signal.aborted) controller.abort()
    else signal.addEventListener('abort', abortByCaller, { once: true })
  }

  // 超时同时中断底层 fetch（避免请求悬挂）；用 flag 区分「超时」与「调用方取消」
  let timedOut = false
  const timer =
    Number.isFinite(timeoutMs) && timeoutMs > 0
      ? setTimeout(() => {
          timedOut = true
          controller.abort()
        }, timeoutMs)
      : undefined

  const requestHeaders: Record<string, string> = { Accept: 'application/json', ...headers }
  if (body !== undefined) requestHeaders['Content-Type'] = 'application/json'

  try {
    let response: Response
    try {
      response = await fetch(url, {
        method,
        headers: requestHeaders,
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: controller.signal,
        credentials,
      })
    } catch (error) {
      if (timedOut) {
        throw new ApiError(`请求超时（${Math.round(timeoutMs / 1000)} 秒）：${path}`, 'TIMEOUT')
      }
      if (controller.signal.aborted) throw new ApiError('请求已取消', 'ABORTED')
      throw new ApiError(
        `无法连接到服务（请确认后端已启动）：${getErrorMessage(error)}`,
        'NETWORK_ERROR',
      )
    }

    if (!response.ok) throw await parseErrorBody(response)

    if (response.status === 204) return undefined as T
    const text = await response.text()
    if (!text) return undefined as T
    try {
      return JSON.parse(text) as T
    } catch {
      throw new ApiError(
        '服务返回的内容不是合法 JSON',
        'INVALID_RESPONSE',
        response.status,
        text.slice(0, 500),
      )
    }
  } finally {
    if (timer !== undefined) clearTimeout(timer)
    if (signal) signal.removeEventListener('abort', abortByCaller)
  }
}

/** 便捷方法（`http.get<T>('/settings')`） */
export const http = {
  get: <T>(path: string, options?: Omit<RequestOptions, 'method' | 'body'>) =>
    apiRequest<T>(path, { ...options, method: 'GET' }),
  post: <T>(path: string, body?: unknown, options?: Omit<RequestOptions, 'method' | 'body'>) =>
    apiRequest<T>(path, { ...options, method: 'POST', body }),
  put: <T>(path: string, body?: unknown, options?: Omit<RequestOptions, 'method' | 'body'>) =>
    apiRequest<T>(path, { ...options, method: 'PUT', body }),
  delete: <T>(path: string, options?: Omit<RequestOptions, 'method' | 'body'>) =>
    apiRequest<T>(path, { ...options, method: 'DELETE' }),
}

/** 从任意异常里取一句可展示的中文文案（永不抛异常、永不返回空串） */
export function getErrorMessage(error: unknown, fallback = '操作失败，请稍后重试。'): string {
  if (error instanceof ApiError) return error.message || fallback
  if (error instanceof Error) return error.message || fallback
  if (typeof error === 'string' && error) return error
  if (error && typeof error === 'object' && 'message' in error) {
    const message = (error as { message?: unknown }).message
    if (typeof message === 'string' && message) return message
  }
  return fallback
}

/**
 * 失败即弹 Toast 的兜底：`reportApiError(error)` → Error Toast（默认停留 8s）。
 * 返回同一句文案，便于调用方同时做内联展示。
 */
export function reportApiError(error: unknown, prefix?: string): string {
  const message = getErrorMessage(error)
  toast.error(prefix ? `${prefix}${message}` : message)
  return message
}
