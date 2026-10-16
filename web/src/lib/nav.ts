/**
 * 侧栏导航配置（ui-spec §2.2）。
 * 单独放一个非组件模块，避免和组件文件混在一起（React Fast Refresh 只允许组件文件导出组件）。
 */

/** 主导航项 */
export interface NavItem {
  /** react-router 路径（`/` 或 `/convert`） */
  to: string
  label: string
  /** 图标（emoji），渲染在 aria-hidden 的 span 里 */
  icon: string
  /** true = 精确匹配（`NavLinkMatch.All`），只有首页用 */
  exact?: boolean
}

/** 六个主导航项（NavMenu.razor:27-35，顺序固定；`/templates-admin` 不进导航） */
export const NAV_ITEMS: readonly NavItem[] = [
  { to: '/', label: '首页', icon: '🏠', exact: true },
  { to: '/convert', label: '转换', icon: '🔄' },
  { to: '/batch', label: '批量处理', icon: '📁' },
  { to: '/templates', label: '模板', icon: '📋' },
  { to: '/tasks', label: '任务列表', icon: '🗂️' },
  { to: '/settings', label: '设置', icon: '⚙️' },
]
