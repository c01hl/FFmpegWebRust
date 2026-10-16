/**
 * WebSocket 客户端：**全局单例连接**（对应参考实现里由 MainLayout 统一持有的长连接）。
 *
 * 契约（docs/api-contract.md §2）：端点 `GET /api/ws`，单连接、广播式，服务端不区分会话。
 *
 * ── 事件流 ────────────────────────────────────────────────────────────────
 * 连接建立后服务端**立即推快照** `snapshot{queue, runningTasks, pendingTasks}`（避免首屏竞态）。
 * 之后是统一信封 `{type, ts, data}` 的增量事件：
 *   task.progress / task.status / task.log / queue.changed / batch.status /
 *   encoders.detected / settings.changed / templates.changed / pong / server.shutdown
 *
 * 本模块把事件做两件事：
 *   1. 派发到 zustand store（`store/queue.ts` 拿队列计数、运行中/等待中任务、进度、日志）；
 *      另外把 `settings.changed` / `encoders.detected` / `templates.changed` 的最新一份
 *      缓存在本模块（`latestSettings()` / `latestHardwareReport()` / `templatesChangedAt()`），
 *      页面要读最新设置或硬件报告时不用自己再订阅一遍。
 *   2. 广播到类型安全的事件总线：`onWsEvent('task.progress', handler)`，无需接触 socket。
 *
 * ── 断线重连与「重新同步」─────────────────────────────────────────────────
 * 指数退避（1s → 2s → 4s …，上限 30s，带 ±20% 抖动）。重连成功后服务端会重新推快照，
 * 所以**队列/进度/日志类状态自动恢复**；但页面自己通过 REST 拉取的数据（任务历史列表、
 * 批次列表、模板列表、设置表单…）不会自动刷新 —— 页面应当在下面这个钩子里重新拉一次：
 *
 *   useEffect(() => onWsStatus((status) => {
 *     if (status === 'open') void reload()
 *   }), [reload])
 *
 * 日志补齐：重连后按契约用 `GET /api/tasks/{id}/log?tailLines=200` 拉已错过的行
 * （见 api/endpoints.ts 的 tasksApi.log）。
 *
 * 心跳：客户端 20s 一次 `{action:'ping'}`；服务端 90s 未收到任何消息则关闭。
 * 另外本地看门狗：60s 没收到任何消息（含 pong）就主动断开重连。
 */

import { ApiError } from './client'
import type {
  HardwareReportDto,
  SettingsDto,
  TaskDto,
  TemplatesChangedEventDto,
  WsClientMessage,
  WsEventOf,
  WsEventType,
  WsServerEvent,
  WsTopic,
} from './types'
import { WS_PING_INTERVAL_MS } from './types'
import { useQueueStore, type WsConnectionState } from '../store/queue'
import { toast } from '../store/toast'

/** WebSocket 路径（同源） */
export const WS_PATH = '/api/ws'
/** 首次重连延迟 */
export const RECONNECT_BASE_DELAY_MS = 1_000
/** 重连延迟上限 */
export const RECONNECT_MAX_DELAY_MS = 30_000
/** 看门狗：这么久没收到任何消息（含 pong）就视为死连接 */
export const WS_WATCHDOG_MS = 60_000

type AnyEventHandler = (event: WsServerEvent) => void
type StatusHandler = (status: WsConnectionState) => void
type ErrorHandler = (error: Error) => void

const eventHandlers = new Map<WsEventType, Set<AnyEventHandler>>()
const statusHandlers = new Set<StatusHandler>()
const errorHandlers = new Set<ErrorHandler>()

/* ── 服务端推送的「最新一份」缓存 ──────────────────────────────────────── */

let lastSettings: SettingsDto | null = null
let lastHardwareReport: HardwareReportDto | null = null
let lastTemplatesChanged: (TemplatesChangedEventDto & { at: number }) | null = null

/** 最近一次 `settings.changed` 推来的设置（可能为 null = 还没收到过） */
export function latestSettings(): SettingsDto | null {
  return lastSettings
}

/** 最近一次 `encoders.detected` 推来的硬件报告（可能为 null） */
export function latestHardwareReport(): HardwareReportDto | null {
  return lastHardwareReport
}

/** 最近一次 `templates.changed`（模板 CRUD/导入/同步后页面据此失效缓存） */
export function templatesChangedAt(): (TemplatesChangedEventDto & { at: number }) | null {
  return lastTemplatesChanged
}

/* ── 事件总线 ──────────────────────────────────────────────────────────── */

/**
 * 订阅某一类 WS 事件。返回取消订阅函数（放进 useEffect 的清理里）。
 *
 *   useEffect(() => onWsEvent('task.log', (event) => setLines((prev) => [...prev, ...event.data.lines])), [])
 */
export function onWsEvent<TType extends WsEventType>(
  type: TType,
  handler: (event: WsEventOf<TType>) => void,
): () => void {
  const set = eventHandlers.get(type) ?? new Set<AnyEventHandler>()
  const wrapped = handler as unknown as AnyEventHandler
  set.add(wrapped)
  eventHandlers.set(type, set)
  return () => {
    set.delete(wrapped)
  }
}

/** 订阅连接状态变化（`'open'` 时应当重新拉取页面自己维护的数据） */
export function onWsStatus(handler: StatusHandler): () => void {
  statusHandlers.add(handler)
  return () => {
    statusHandlers.delete(handler)
  }
}

/** 订阅协议/网络错误（用于把「实时通道异常」展示给用户） */
export function onWsError(handler: ErrorHandler): () => void {
  errorHandlers.add(handler)
  return () => {
    errorHandlers.delete(handler)
  }
}

function emitEvent(event: WsServerEvent): void {
  const set = eventHandlers.get(event.type)
  if (!set) return
  for (const handler of set) {
    try {
      handler(event)
    } catch (error) {
      // 单个订阅者抛异常不能影响其它订阅者
      console.error('[ws] 事件处理器异常：', error)
    }
  }
}

function emitStatus(status: WsConnectionState): void {
  useQueueStore.getState().setConnection(status)
  for (const handler of statusHandlers) {
    try {
      handler(status)
    } catch (error) {
      console.error('[ws] 状态处理器异常：', error)
    }
  }
}

function emitError(error: Error): void {
  for (const handler of errorHandlers) {
    try {
      handler(error)
    } catch (thrown) {
      console.error('[ws] 错误处理器异常：', thrown)
    }
  }
}

/** 把事件派发到 zustand store（队列/进度/日志/批次） */
function dispatchToStores(event: WsServerEvent): void {
  const queue = useQueueStore.getState()
  switch (event.type) {
    case 'snapshot':
      queue.applySnapshot(event.data)
      break
    case 'task.progress':
      queue.applyProgress(event.data)
      break
    case 'task.status':
      queue.applyStatus(event.data)
      break
    case 'task.log':
      queue.appendLog(event.data)
      break
    case 'queue.changed':
      queue.applyQueueChanged(event.data)
      break
    case 'batch.status':
      queue.applyBatchStatus(event.data)
      break
    case 'encoders.detected':
      lastHardwareReport = event.data
      break
    case 'settings.changed':
      lastSettings = event.data
      break
    case 'templates.changed':
      lastTemplatesChanged = { ...event.data, at: Date.now() }
      break
    case 'server.shutdown':
      toast.warning(`服务端正在关闭：${event.data.reason || '未知原因'}`)
      break
    case 'pong':
      break
    default:
      // 服务端新增事件类型时前端不应崩：只记录
      console.debug('[ws] 未知事件类型：', event)
      break
  }
}

/** 解析一条服务端消息（解析失败返回 null 并报错） */
function parseMessage(raw: string): WsServerEvent | null {
  try {
    const parsed = JSON.parse(raw) as { type?: unknown }
    if (!parsed || typeof parsed !== 'object' || typeof parsed.type !== 'string') {
      emitError(new ApiError('收到无法识别的 WebSocket 消息', 'WS_INVALID_MESSAGE'))
      return null
    }
    return parsed as WsServerEvent
  } catch (error) {
    emitError(new ApiError(`WebSocket 消息不是合法 JSON：${(error as Error).message}`, 'WS_INVALID_MESSAGE'))
    return null
  }
}

/* ── 单例连接 ──────────────────────────────────────────────────────────── */

export interface WsClient {
  /** 建立连接（幂等；已连接/连接中时什么都不做） */
  connect: () => void
  /** 主动断开并停止重连（AppShell 卸载时调用） */
  disconnect: () => void
  /** 当前连接状态 */
  connectionState: () => WsConnectionState
  isConnected: () => boolean
  /** 发一条原始消息（未连接时返回 false，不抛异常） */
  send: (message: WsClientMessage) => boolean
  /** 发送订阅请求（缺省订阅全部；服务端默认也是全部） */
  subscribe: (topics?: WsTopic[]) => boolean
  /** 心跳（一般由内部定时器调用） */
  ping: () => boolean
  /** 便捷取消通道：不经过 REST 直接取消任务 */
  cancelTask: (taskId: string, graceful?: boolean) => boolean
}

let socket: WebSocket | null = null
let reconnectTimer: ReturnType<typeof setTimeout> | null = null
let pingTimer: ReturnType<typeof setInterval> | null = null
let reconnectAttempts = 0
/** 用户主动断开后不再自动重连 */
let manuallyClosed = false
/** 曾经连上过（用于区分首次连接与重连的状态文案） */
let hasConnectedOnce = false
let lastMessageAt = 0
/** 页面要订阅的主题（默认全部） */
let topics: WsTopic[] = ['tasks', 'logs', 'queue']

function wsUrl(): string {
  if (typeof window === 'undefined') return ''
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${protocol}//${window.location.host}${WS_PATH}`
}

function clearTimers(): void {
  if (reconnectTimer !== null) {
    clearTimeout(reconnectTimer)
    reconnectTimer = null
  }
  if (pingTimer !== null) {
    clearInterval(pingTimer)
    pingTimer = null
  }
}

function backoffDelay(attempt: number): number {
  const base = Math.min(RECONNECT_BASE_DELAY_MS * 2 ** Math.max(0, attempt - 1), RECONNECT_MAX_DELAY_MS)
  // ±20% 抖动，避免多标签页同时重连
  const jitter = base * 0.2 * (Math.random() * 2 - 1)
  return Math.max(200, Math.round(base + jitter))
}

function scheduleReconnect(): void {
  if (manuallyClosed) return
  if (reconnectTimer !== null) return
  reconnectAttempts += 1
  const delay = backoffDelay(reconnectAttempts)
  emitStatus('reconnecting')
  console.warn(`[ws] ${delay}ms 后重连（第 ${reconnectAttempts} 次）`)
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null
    open()
  }, delay)
}

function startHeartbeat(): void {
  if (pingTimer !== null) clearInterval(pingTimer)
  pingTimer = setInterval(() => {
    // 看门狗：长时间没有任何消息（含 pong）说明连接已死，直接断开触发重连
    if (Date.now() - lastMessageAt > WS_WATCHDOG_MS) {
      console.warn('[ws] 长时间未收到消息，主动断开重连')
      socket?.close()
      return
    }
    client.ping()
  }, WS_PING_INTERVAL_MS)
}

function open(): void {
  if (typeof WebSocket === 'undefined') return
  if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) return

  manuallyClosed = false
  emitStatus(hasConnectedOnce ? 'reconnecting' : 'connecting')

  let next: WebSocket
  try {
    next = new WebSocket(wsUrl())
  } catch (error) {
    emitError(new ApiError(`无法创建 WebSocket 连接：${(error as Error).message}`, 'WS_CREATE_FAILED'))
    scheduleReconnect()
    return
  }
  socket = next
  lastMessageAt = Date.now()

  next.onopen = () => {
    hasConnectedOnce = true
    reconnectAttempts = 0
    lastMessageAt = Date.now()
    emitStatus('open')
    // 服务端默认订阅全部；只有显式设过主题时才需要发 subscribe
    if (topics.length > 0 && topics.length < 3) {
      client.subscribe(topics)
    }
    startHeartbeat()
  }

  next.onmessage = (messageEvent: MessageEvent<string>) => {
    lastMessageAt = Date.now()
    const event = parseMessage(messageEvent.data)
    if (!event) return
    dispatchToStores(event)
    emitEvent(event)
  }

  next.onerror = () => {
    // onerror 之后一定会收到 onclose；这里只记录，避免重复重连
    console.warn('[ws] 连接出错')
  }

  next.onclose = (event: CloseEvent) => {
    if (socket === next) socket = null
    if (pingTimer !== null) {
      clearInterval(pingTimer)
      pingTimer = null
    }
    if (manuallyClosed) {
      emitStatus('closed')
      return
    }
    console.warn(`[ws] 连接关闭（code=${event.code}），准备重连`)
    scheduleReconnect()
  }
}

export const client: WsClient = {
  connect: () => {
    manuallyClosed = false
    if (socket) {
      if (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING) return
    }
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    open()
  },

  disconnect: () => {
    manuallyClosed = true
    reconnectAttempts = 0
    clearTimers()
    const current = socket
    socket = null
    if (current) {
      current.onopen = null
      current.onmessage = null
      current.onerror = null
      current.onclose = null
      try {
        current.close()
      } catch {
        // 关闭失败不影响后续
      }
    }
    emitStatus('closed')
  },

  connectionState: () => useQueueStore.getState().connection,

  isConnected: () => socket?.readyState === WebSocket.OPEN,

  send: (message) => {
    if (!socket || socket.readyState !== WebSocket.OPEN) return false
    try {
      socket.send(JSON.stringify(message))
      return true
    } catch (error) {
      emitError(new ApiError(`发送 WebSocket 消息失败：${(error as Error).message}`, 'WS_SEND_FAILED'))
      return false
    }
  },

  subscribe: (nextTopics) => {
    if (nextTopics && nextTopics.length > 0) topics = nextTopics
    return client.send({ action: 'subscribe', topics })
  },

  ping: () => client.send({ action: 'ping' }),

  cancelTask: (taskId, graceful = true) => client.send({ action: 'task.cancel', taskId, graceful }),
}

/**
 * 页面要改订阅主题时调用（例如只关心日志的页面订阅 `['tasks','logs']`）。
 * 保存下来，重连后会自动重新订阅。
 */
export function setWsTopics(nextTopics: WsTopic[]): void {
  topics = nextTopics.length > 0 ? nextTopics : ['tasks', 'logs', 'queue']
  if (client.isConnected()) client.subscribe(topics)
}

/** 浏览器恢复联网时立即重连，不等退避计时器 */
if (typeof window !== 'undefined') {
  window.addEventListener('online', () => {
    if (!manuallyClosed && !client.isConnected()) {
      clearTimers()
      reconnectAttempts = 0
      open()
    }
  })
}

/** 便捷：取 store 里最新快照的运行中任务（ws.ts 内部与测试用） */
export function currentRunningTasks(): TaskDto[] {
  return useQueueStore.getState().runningTasks
}
