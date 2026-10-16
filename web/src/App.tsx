import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom'
import { AppShell } from './components/layout/AppShell'
import { ErrorPlaceholder, NotFoundPlaceholder } from './pages/_placeholder'
import { HomePage } from './pages/HomePage'
import { ConvertPage } from './pages/ConvertPage'
import { BatchPage } from './pages/BatchPage'
import { TemplatesAdminPage } from './pages/TemplatesAdminPage'
import { TemplatesPage } from './pages/TemplatesPage'
import { TasksPage } from './pages/TasksPage'
import { SettingsPage } from './pages/SettingsPage'

/**
 * 路由表（ui-spec §1，路径与参考实现完全一致）。
 *
 * | 路由                                   | 页面           | 中文页名         |
 * | -------------------------------------- | -------------- | ---------------- |
 * | `/`                                    | Home           | 首页             |
 * | `/convert`                             | Convert        | 转换             |
 * | `/batch`                               | BatchProcess   | 批量处理         |
 * | `/templates`                           | Templates      | 模板管理         |
 * | `/templates-admin`、`/templates/admin` | TemplatesAdmin | 预设模板管理     |
 * | `/tasks`                               | Tasks          | 任务列表         |
 * | `/settings`                            | Settings       | 设置             |
 * | `/Error`                               | Error          | 出错了           |
 * | `/not-found`                           | NotFound       | 404 — 页面不存在 |
 * | 其它                                   | → `/not-found` | 404 兜底         |
 *
 * 查询参数：`/convert?template=<id>`（模板页「选择」按钮跳转时预选模板）。
 * 页面标题由各页面自己用 `lib/title.ts` 的 `useDocumentTitle(PAGE_TITLES.xxx)` 设置。
 *
 * ⚠️ 现在的页面元素全是 `ComingSoon` 占位；后续任务把它们替换成真页面即可，
 * 路径与外壳（AppShell）不要改。
 */
export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<HomePage />} />
          <Route path="convert" element={<ConvertPage />} />
          <Route path="batch" element={<BatchPage />} />
          <Route path="templates" element={<TemplatesPage />} />
          {/* 预设模板管理有两个入口路径，且不进入主导航 */}
          <Route path="templates-admin" element={<TemplatesAdminPage />} />
          <Route path="templates/admin" element={<TemplatesAdminPage />} />
          <Route path="tasks" element={<TasksPage />} />
          <Route path="settings" element={<SettingsPage />} />
          <Route path="Error" element={<ErrorPlaceholder />} />
          <Route path="not-found" element={<NotFoundPlaceholder />} />
          <Route path="*" element={<Navigate to="/not-found" replace />} />
        </Route>
      </Routes>
    </BrowserRouter>
  )
}

/**
 * `/templates-admin` 与 `/templates/admin` 共用 `TemplatesAdminPage`（ui-spec §4.6）。
 */
