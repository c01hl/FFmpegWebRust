import { NavLink } from 'react-router-dom'
import { APP_VERSION } from '../../lib/title'
import { NAV_ITEMS, type NavItem } from '../../lib/nav'

export interface NavMenuProps {
  /** 主导航项；默认用 NavMenu.razor 的六项 */
  items?: readonly NavItem[]
  /** 页脚第一行左侧的平台显示名（`macOS` / `Windows` / `Linux`）；未知时传 undefined */
  platform?: string
  /** 页脚第一行右侧的运行时名（Rust 版固定 `Rust`，对应参考实现的 `.NET`） */
  runtime?: string
  /** 页脚第二行的版本号 */
  version?: string
}

/**
 * 侧栏导航（对应 NavMenu.razor）。
 *
 * - 品牌区 `🎬 FFmpeg WebUI`；
 * - `<nav aria-label="主导航">`，顺序即 §2.2 表格顺序：首页 / 转换 / 批量处理 / 模板 / 任务列表 / 设置；
 * - 首页用**精确匹配**（`end`），其余用前缀匹配；`/templates-admin` 会因前缀匹配点亮「模板」（与参考实现一致）；
 * - 页脚：`{平台} · Rust` + `v2.0.0`（等宽），`margin-top:auto` 顶到侧栏底部；
 * - 窄屏（≤900px）由 CSS 变成整宽横条、隐藏页脚（.nav-menu / .nav-footer 的媒体查询）。
 */
export function NavMenu({
  items = NAV_ITEMS,
  platform,
  runtime = 'Rust',
  version = APP_VERSION,
}: NavMenuProps) {
  return (
    <>
      <div className="brand">
        <span className="brand-icon" aria-hidden="true">
          🎬
        </span>
        <span className="brand-text">FFmpeg WebUI</span>
      </div>

      <nav className="nav-menu" aria-label="主导航">
        {items.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.exact}
            className={({ isActive }) => (isActive ? 'nav-link active' : 'nav-link')}
          >
            <span className="nav-icon" aria-hidden="true">
              {item.icon}
            </span>
            <span className="nav-text">{item.label}</span>
          </NavLink>
        ))}
      </nav>

      <div className="nav-footer">
        <div className="nav-footer-line">
          {platform ?? '未知平台'} · {runtime}
        </div>
        <div className="nav-footer-line mono">v{version}</div>
      </div>
    </>
  )
}
