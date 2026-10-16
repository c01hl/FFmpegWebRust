/**
 * 队列 / 任务实时状态 store（zustand）。
 *
 * 由 WebSocket 驱动（见 `api/ws.ts`）：
 * - 连接建立后服务端立刻推 `snapshot`（queue + runningTasks + pendingTasks），
 *   因此**首屏不会有竞态**，顶栏计数与进行中列表都能立即拿到值；
 * - `task.progress` / `task.status` / `queue.changed` / `task.log` 增量更新这里；
 * - 断线重连后服务端会重新推快照，页面自己的列表数据（历史、批次等）应
 *   在 `onWsStatus` 回到 `'open'` 时**重新拉取一次**（见 ws.ts 顶部注释）。
 *
 * 页面用法：
 *   const active = useQueueStore(selectActiveCount)
 *   const running = useQueueStore(selectRunningTasks)
 */

import { create } from 'zustand'
import type {
  BatchStatusEventDto,
  QueueChangedEventDto,
  QueueStatusDto,
  TaskDto,
  TaskLogEventDto,
  TaskProgressEventDto,
  TaskStatusEventDto,
  WsSnapshotDto,
} from '../api/types'

/** WebSocket 连接状态 */
export type WsConnectionState = 'idle' | 'connecting' | 'open' | 'reconnecting' | 'closed'

/** 单任务日志缓冲上限（ui-spec §11：每任务 800 行） */
export const MAX_LOG_LINES_PER_TASK = 800

/** 一条任务的实时进度（来自 `task.progress`） */
export interface TaskLiveProgress {
  taskId: string
  /** 0..100 */
  progress: number
  speed: number | null
  eta: number | null
  currentTime: number
  totalDuration: number
  /** 本地收到时间（ms），用于判断数据是否新鲜 */
  updatedAt: number
}

/** 单任务日志缓冲 */
export interface TaskLogBuffer {
  lines: string[]
  /** 服务端因合并/上限丢弃的行数 */
  dropped: number
  /** 服务端是否截断过 */
  truncated: boolean
  updatedAt: number
}

export interface QueueState {
  /** 排队中的任务数 */
  queued: number
  /** 正在执行的任务数 */
  active: number
  /** 并发上限（来自快照/系统设置） */
  maxConcurrentTasks: number
  /** 运行中的任务（快照 + task.status 维护） */
  runningTasks: TaskDto[]
  /** 等待中的任务 */
  pendingTasks: TaskDto[]
  /** taskId → 进度快照 */
  progress: Record<string, TaskLiveProgress>
  /** taskId → 日志缓冲 */
  logs: Record<string, TaskLogBuffer>
  /** batchId → 最近一次 batch.status 事件 */
  batches: Record<string, BatchStatusEventDto>
  /** WebSocket 连接状态（StatusBar 用） */
  connection: WsConnectionState
  /** 最近一次快照时间（ms） */
  lastSnapshotAt: number | null
  /** 最近一次事件时间（ms） */
  lastEventAt: number | null

  /** 连接成功后的整份快照（**唯一权威来源**，会覆盖 running/pending） */
  applySnapshot: (snapshot: WsSnapshotDto) => void
  /** 只更新计数（REST `/api/system/queue` 复用同一入口） */
  applyQueueStatus: (queue: QueueStatusDto) => void
  applyQueueChanged: (event: QueueChangedEventDto) => void
  applyProgress: (event: TaskProgressEventDto) => void
  applyStatus: (event: TaskStatusEventDto) => void
  applyBatchStatus: (event: BatchStatusEventDto) => void
  appendLog: (event: TaskLogEventDto) => void
  /** 清空某任务日志缓冲（页面「清空日志」用；服务端日志不受影响） */
  clearTaskLog: (taskId: string) => void
  setConnection: (connection: WsConnectionState) => void
  /** 断开或彻底登出时复位（保留 connection） */
  reset: () => void
}

const emptyLog = (): TaskLogBuffer => ({ lines: [], dropped: 0, truncated: false, updatedAt: Date.now() })

function statusBucket(task: TaskDto): 'running' | 'pending' | 'terminal' {
  if (task.status === 'Running') return 'running'
  if (task.status === 'Pending') return 'pending'
  return 'terminal'
}

/** 从两个列表里移除指定 id */
function withoutTask(list: TaskDto[], taskId: string): TaskDto[] {
  return list.filter((task) => task.id !== taskId)
}

export const useQueueStore = create<QueueState>((set) => ({
  queued: 0,
  active: 0,
  maxConcurrentTasks: 1,
  runningTasks: [],
  pendingTasks: [],
  progress: {},
  logs: {},
  batches: {},
  connection: 'idle',
  lastSnapshotAt: null,
  lastEventAt: null,

  applySnapshot: (snapshot) =>
    set((state) => ({
      queued: snapshot.queue?.queued ?? 0,
      active: snapshot.queue?.active ?? 0,
      maxConcurrentTasks: snapshot.queue?.maxConcurrentTasks ?? state.maxConcurrentTasks,
      runningTasks: snapshot.runningTasks ?? [],
      pendingTasks: snapshot.pendingTasks ?? [],
      lastSnapshotAt: Date.now(),
      lastEventAt: Date.now(),
    })),

  applyQueueStatus: (queue) =>
    set({
      queued: queue.queued ?? 0,
      active: queue.active ?? 0,
      maxConcurrentTasks: queue.maxConcurrentTasks ?? 1,
    }),

  applyQueueChanged: (event) =>
    set({ queued: event.queued ?? 0, active: event.active ?? 0, lastEventAt: Date.now() }),

  applyProgress: (event) =>
    set((state) => ({
      progress: {
        ...state.progress,
        [event.taskId]: {
          taskId: event.taskId,
          progress: event.progress,
          speed: event.speed ?? null,
          eta: event.eta ?? null,
          currentTime: event.currentTime ?? 0,
          totalDuration: event.totalDuration ?? 0,
          updatedAt: Date.now(),
        },
      },
      lastEventAt: Date.now(),
    })),

  applyStatus: (event) =>
    set((state) => {
      const task = event.task
      // 没有完整任务对象时，就地把已有任务的状态改掉（服务端可能只推状态）
      const patch = (list: TaskDto[]): TaskDto[] =>
        list.map((item) =>
          item.id === event.taskId
            ? {
                ...item,
                status: event.newStatus,
                errorMessage: event.errorMessage ?? null,
                ...(task ?? {}),
              }
            : item,
        )

      let runningTasks = patch(state.runningTasks)
      let pendingTasks = patch(state.pendingTasks)

      // 位置调整：变成 Running → 进 running；回到 Pending → 进 pending；终态 → 两边都移除
      if (task) {
        const bucket = statusBucket(task)
        if (bucket === 'terminal') {
          runningTasks = withoutTask(runningTasks, task.id)
          pendingTasks = withoutTask(pendingTasks, task.id)
        } else {
          runningTasks = withoutTask(runningTasks, task.id)
          pendingTasks = withoutTask(pendingTasks, task.id)
          if (bucket === 'running') runningTasks = [...runningTasks, task]
          else pendingTasks = [...pendingTasks, task]
        }
      }

      return { runningTasks, pendingTasks, lastEventAt: Date.now() }
    }),

  applyBatchStatus: (event) =>
    set((state) => ({
      batches: { ...state.batches, [event.batchId]: event },
      lastEventAt: Date.now(),
    })),

  appendLog: (event) =>
    set((state) => {
      const previous = state.logs[event.taskId] ?? emptyLog()
      const merged = [...previous.lines, ...(event.lines ?? [])]
      // 只保留最后 MAX_LOG_LINES_PER_TASK 行（ui-spec §11 的内存上限）
      const overflow = Math.max(0, merged.length - MAX_LOG_LINES_PER_TASK)
      return {
        logs: {
          ...state.logs,
          [event.taskId]: {
            lines: overflow > 0 ? merged.slice(overflow) : merged,
            dropped: (previous.dropped ?? 0) + (event.dropped ?? 0) + overflow,
            truncated: previous.truncated || Boolean(event.truncated) || overflow > 0,
            updatedAt: Date.now(),
          },
        },
        lastEventAt: Date.now(),
      }
    }),

  clearTaskLog: (taskId) =>
    set((state) => ({
      logs: { ...state.logs, [taskId]: emptyLog() },
    })),

  setConnection: (connection) => set({ connection }),

  reset: () =>
    set({
      queued: 0,
      active: 0,
      maxConcurrentTasks: 1,
      runningTasks: [],
      pendingTasks: [],
      progress: {},
      logs: {},
      batches: {},
      lastSnapshotAt: null,
      lastEventAt: null,
    }),
}))

/* ── 选择器（组件里用它避免整店订阅）──────────────────────────────────── */

export const selectQueuedCount = (state: QueueState): number => state.queued
export const selectActiveCount = (state: QueueState): number => state.active
export const selectHasActivity = (state: QueueState): boolean => state.active > 0 || state.queued > 0
export const selectRunningTasks = (state: QueueState): TaskDto[] => state.runningTasks
export const selectPendingTasks = (state: QueueState): TaskDto[] => state.pendingTasks
export const selectConnection = (state: QueueState): WsConnectionState => state.connection

/** 取某任务的进度（没有实时事件时返回 undefined，页面回退到 TaskDto.progress） */
export const selectProgress =
  (taskId: string) =>
  (state: QueueState): TaskLiveProgress | undefined =>
    state.progress[taskId]

/** 取某任务的日志缓冲 */
export const selectTaskLog =
  (taskId: string) =>
  (state: QueueState): TaskLogBuffer | undefined =>
    state.logs[taskId]

/** 非组件环境读计数（例如 ws.ts 打日志） */
export function currentQueueCounts(): { queued: number; active: number } {
  const { queued, active } = useQueueStore.getState()
  return { queued, active }
}
