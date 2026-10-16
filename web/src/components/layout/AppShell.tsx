import { useCallback, useEffect, useRef, useState } from 'react'
import { Outlet, useLocation } from 'react-router-dom'
import { getErrorMessage } from '../../api/client'
import { systemApi } from '../../api/endpoints'
import type { AppStatusDto } from '../../api/types'
import { client as wsClient } from '../../api/ws'
import { NAV_ITEMS } from '../../lib/nav'
import { APP_VERSION } from '../../lib/title'
import { toast } from '../../store/toast'
import { useQueueStore } from '../../store/queue'
import { NavMenu } from './NavMenu'
import { StatusBar } from './StatusBar'
import { ToastHost } from '../ui/ToastHost'

/**
 * 本地 hook：拉一次系统状态（FFmpeg 可用性 + 平台 + 输出目录）。
 *
 * ⚠️ 注意第一个 `await` 之前**不要** setState：在 effect 的同步阶段 setState 会造成级联渲染
 * （oxlint 的 react/set-state-in-effect 规则会拦）。初始的 `loading=true` 已经覆盖首屏需要。
 */
function useSystemStatus(): { status: AppStatusDto | null; loading: boolean; reload: () => void } {
  const [status, setStatus] = useState<AppStatusDto | null>(null)
  const [loading, setLoading] = useState(true)
  const notifiedRef = useRef(false)

  const refresh = useCallback(async (): Promise<void> => {
    try {
      const result = await systemApi.status()
      setStatus(result)
      notifiedRef.current = false
    } catch (error) {
      // 任何失败都要有可见反馈，且不允许出现未捕获的 rejection
      setStatus(null)
      if (!notifiedRef.current) {
        notifiedRef.current = true
        toast.warning(`无法获取系统状态：${getErrorMessage(error)}`)
      }
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    // 拉取系统状态属于「与外部系统同步」：setState 只发生在 await 之后，
    // 不会在 effect 同步阶段触发级联渲染，故显式关闭该启发式规则。
    // oxlint-disable-next-line react/set-state-in-effect
    void refresh()
  }, [refresh])

  // 供页面/顶栏的「重试」按钮使用（事件回调里可以放心 setState）
  const reload = useCallback(() => {
    setLoading(true)
    void refresh()
  }, [refresh])

  return { status, loading, reload }
}

/**
 * 应用外壳（对应 MainLayout.razor）。
 *
 * ┌────────────┬───────────────────────────────────────────────┐
 * │ .sidebar   │ .top-row（StatusBar，sticky，z-index 20）      │
 * │  NavMenu   ├───────────────────────────────────────────────┤
 * │            │ .content（<Outlet />，页面内容）                │
 * └────────────┴───────────────────────────────────────────────┘
 *                ToastHost（右下角浮层，z-index 2000）
 *
 * 它同时负责三件全局事务：
 * 1. **WebSocket 单例连接**：挂载时连上，卸载时断开（严格模式下重复挂载是幂等的）；
 * 2. **系统状态**：拉一次 `/api/system/status`，用于顶栏 FFmpeg 徽标与页脚平台名，失败弹 Toast；
 * 3. **路由切换后把焦点移到页面 h1**（对应参考实现的 `FocusOnNavigate Selector="h1"`，
 *    见 ui-spec §2.6.4 第 2 条；页面只要保证首个标题是 `<h1>` 即可）。
 */
export function AppShell() {
  const { status, loading } = useSystemStatus()
  // 版本以服务端为准（Cargo.toml），取不到时用兜底常量
  const version = status?.version ?? APP_VERSION
  const location = useLocation()

  // 全局唯一的 WS 连接
  useEffect(() => {
    wsClient.connect()
    return () => {
      wsClient.disconnect()
      // 断开后计数归零，避免顶栏显示过期的「进行中」
      useQueueStore.getState().reset()
    }
  }, [])

  // 路由切换 → 焦点移到 h1（键盘/读屏用户不会停在旧位置）。
  // 首次渲染不抢焦点：参考实现的 FocusOnNavigate 只在「导航」时生效，
  // 首屏聚焦 h1 会让用户看到一个多余的焦点环。
  // ⚠️ 用「上一次的路径」判断而不是布尔开关：开发模式下 StrictMode 会把挂载 effect 跑两次，
  //    布尔开关会被第二次运行绕过去。
  const lastPathRef = useRef(location.pathname)
  useEffect(() => {
    if (lastPathRef.current === location.pathname) return
    lastPathRef.current = location.pathname
    const heading = document.querySelector<HTMLElement>('.content h1')
    if (!heading) return
    heading.setAttribute('tabindex', '-1')
    heading.focus({ preventScroll: false })
  }, [location.pathname])

  return (
    <div className="page">
      <aside className="sidebar">
        <NavMenu items={NAV_ITEMS} platform={status?.platform} version={version} />
      </aside>

      <main className="main-column">
        <StatusBar status={status} statusLoading={loading} version={version} />
        <article className="content">
          <Outlet />
        </article>
      </main>

      <ToastHost />
    </div>
  )
}
