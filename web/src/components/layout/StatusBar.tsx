import { Link } from 'react-router-dom'
import type { AppStatusDto } from '../../api/types'
import { APP_VERSION } from '../../lib/title'
import { Badge } from '../ui/Badge'
import { Spinner } from '../ui/Spinner'
import { useQueueStore, selectActiveCount, selectConnection, selectQueuedCount } from '../../store/queue'
import { cx } from '../../lib/cx'

export interface StatusBarProps {
  /** 应用版本（默认 2.0.0），渲染成 `v2.0.0` 徽标 */
  version?: string
  /** `GET /api/system/status` 的结果；`null` = 还没拿到（不显示可用性结论） */
  status?: AppStatusDto | null
  /** 正在拉取系统状态 */
  statusLoading?: boolean
}

/** 实时通道状态 → 中文 + 圆点类名 */
function connectionMeta(connection: ReturnType<typeof selectConnection>): {
  text: string
  dot: string
} {
  switch (connection) {
    case 'open':
      return { text: '实时已连接', dot: 'top-dot online' }
    case 'connecting':
    case 'reconnecting':
      return { text: '实时连接中…', dot: 'top-dot pending' }
    default:
      return { text: '实时未连接', dot: 'top-dot offline' }
  }
}

/**
 * 顶栏（对应 MainLayout.razor:16-27 的 `.top-row`）。
 *
 * 内容自左到右：
 * 1. `🎬 FFmpeg WebUI` + `v2.0.0` 徽标（等宽字体）；
 * 2. 弹性空隙；
 * 3. **FFmpeg 可用性**：可用 → 绿色徽标 `FFmpeg {版本}`；不可用 → 红色徽标 `FFmpeg 不可用`；未检测 → 灰色徽标；
 * 4. **实时通道状态**（WebSocket 是否连上，圆点 + 文案）；
 * 5. 运行指示胶囊（**仅当运行中或排队 > 0**）：`.spinner` + `{N} 个转换进行中（{M} 个排队）`，
 *    整颗胶囊是指向 `/tasks` 的链接，`title="查看任务列表"`。
 *
 * 第 5 项的数据完全由 WebSocket 事件驱动（`store/queue.ts` 的 active/queued），
 * 所以「2 个转换进行中」会随着任务状态实时变化。
 */
export function StatusBar({ version = APP_VERSION, status = null, statusLoading = false }: StatusBarProps) {
  const activeCount = useQueueStore(selectActiveCount)
  const queuedCount = useQueueStore(selectQueuedCount)
  const connection = useQueueStore(selectConnection)
  const { text: connectionText, dot } = connectionMeta(connection)

  const ffmpegBadge = (() => {
    if (statusLoading && !status) {
      return (
        <span className="top-meta">
          <Spinner />
          正在检测 FFmpeg…
        </span>
      )
    }
    if (!status) {
      return (
        <Badge tone="default" title="尚未获取到 FFmpeg 状态">
          FFmpeg 状态未知
        </Badge>
      )
    }
    if (status.ffmpegAvailable) {
      return (
        <Badge tone="success" title={status.ffmpegPath || undefined}>
          FFmpeg {status.ffmpegVersion || '可用'}
        </Badge>
      )
    }
    return (
      <Badge tone="danger" title={status.notices.length > 0 ? status.notices.join('；') : undefined}>
        FFmpeg 不可用
      </Badge>
    )
  })()

  return (
    <div className="top-row">
      <span className="app-title">🎬 FFmpeg WebUI</span>
      <span className="app-version ffui-badge">v{version}</span>

      <span className="top-spacer" />

      {ffmpegBadge}

      <span className="top-meta" title={connectionText}>
        <span className={cx(dot)} aria-hidden="true" />
        {connectionText}
      </span>

      {activeCount > 0 || queuedCount > 0 ? (
        <Link className="top-running" to="/tasks" title="查看任务列表">
          <span className="spinner" aria-hidden="true" />
          <span>
            {activeCount} 个转换进行中
            {queuedCount > 0 ? `（${queuedCount} 个排队）` : ''}
          </span>
        </Link>
      ) : null}
    </div>
  )
}
