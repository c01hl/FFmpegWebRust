import { Link } from 'react-router-dom'
import { PAGE_TITLES, useDocumentTitle } from '../lib/title'

export interface ComingSoonProps {
  /** 中文页名（用于 h1 与文档标题） */
  title: string
  /** 该页的路由路径（展示给开发者看） */
  routePath: string
  /** 该页在 ui-spec 里的章节，例如 `ui-spec §4.1` */
  specSection?: string
  /** 页面简述（中文） */
  summary?: string
}

/**
 * 占位页：本任务只负责「前端基础设施」，Home/Convert/Batch/Templates/Tasks/Settings
 * 由后续任务实现。所有路由都已登记，今天就能跑起来。
 *
 * 注意 h1 必须存在且可聚焦：AppShell 在路由切换后会把焦点移到 `.content h1`。
 */
export function ComingSoon({ title, routePath, specSection, summary }: ComingSoonProps) {
  useDocumentTitle(`${title} - FFmpeg WebUI`)

  return (
    <div className="ffui-container">
      <h1>{title}</h1>

      <div className="ffui-panel">
        <div className="ffui-alert ffui-alert-info" role="status">
          <span aria-hidden="true">ℹ️</span>
          <div>
            <strong>该页面尚未实现。</strong>
            <div>
              前端基础设施（设计令牌、API 客户端、WebSocket 客户端、Store、共享组件）已就绪；
              页面本体将在后续实现。
            </div>
          </div>
        </div>

        {summary ? <p style={{ color: 'var(--text-secondary)' }}>{summary}</p> : null}

        <dl style={{ display: 'grid', gridTemplateColumns: 'auto 1fr', gap: '0.4rem 1rem', margin: 0 }}>
          <dt style={{ color: 'var(--text-muted)' }}>路由</dt>
          <dd className="mono" style={{ margin: 0 }}>
            {routePath}
          </dd>
          {specSection ? (
            <>
              <dt style={{ color: 'var(--text-muted)' }}>规格</dt>
              <dd className="mono" style={{ margin: 0 }}>
                {specSection}
              </dd>
            </>
          ) : null}
        </dl>

        <div style={{ display: 'flex', gap: '0.6rem', marginTop: '1.25rem' }}>
          <Link className="ffui-btn ffui-btn-primary" to="/">
            返回首页
          </Link>
          <Link className="ffui-btn" to="/settings">
            打开设置
          </Link>
        </div>
      </div>
    </div>
  )
}

/** 404 页占位（文案逐字来自 NotFound.razor，后续任务可用真页面替换） */
export function NotFoundPlaceholder() {
  useDocumentTitle(PAGE_TITLES.notFound)

  return (
    <div className="ffui-container">
      <div className="ffui-panel" style={{ textAlign: 'center', padding: '3rem 1.5rem' }}>
        <h1>404 — 页面不存在</h1>
        <p style={{ color: 'var(--text-secondary)' }}>
          你访问的地址没有对应的页面，可能是链接已失效或输入有误。
        </p>
        <div style={{ display: 'flex', gap: '0.6rem', justifyContent: 'center', marginTop: '1.25rem' }}>
          <Link className="ffui-btn ffui-btn-primary" to="/">
            返回首页
          </Link>
          <Link className="ffui-btn" to="/convert">
            开始转换
          </Link>
        </div>
      </div>
    </div>
  )
}

/** 错误页占位（文案逐字来自 Error.razor） */
export function ErrorPlaceholder() {
  useDocumentTitle(PAGE_TITLES.error)

  return (
    <div className="ffui-container">
      <div className="ffui-panel">
        <h1>😵 应用遇到了一个错误</h1>
        <p>
          这次请求没有正常完成。多数情况下重新加载页面即可恢复；如果反复出现，请把下面的信息反馈给开发者。
        </p>

        <details style={{ margin: '1rem 0', color: 'var(--text-secondary)', fontSize: '0.9rem' }}>
          <summary>排查建议</summary>
          <ul style={{ margin: '0.6rem 0 0', paddingLeft: '1.2rem' }}>
            <li>确认「设置」里的 FFmpeg 路径指向真实可执行文件（可用「测试 FFmpeg」按钮验证）。</li>
            <li>确认输出目录存在且可写。</li>
            <li>数据目录需要有读写权限；如果被占用或损坏，可删除其中的 data.db 重新初始化。</li>
            <li>查看运行此应用的终端窗口，那里有更详细的错误日志。</li>
          </ul>
        </details>

        <div style={{ display: 'flex', gap: '0.6rem' }}>
          <Link className="ffui-btn ffui-btn-primary" to="/">
            返回首页
          </Link>
          <Link className="ffui-btn" to="/settings">
            打开设置
          </Link>
        </div>
      </div>
    </div>
  )
}
