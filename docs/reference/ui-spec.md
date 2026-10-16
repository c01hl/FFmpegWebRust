# FFmpeg WebUI —— UI/UX 与交互规格说明书（React 重写依据）

> 参考实现：`/Users/c01h/code/github/FFmpegWebUI/`（C# / .NET Blazor Server）
> 本文目标：让一位 **只读本文档的 React 开发者**，能够 1:1 重建全部界面、交互、文案与实时行为。
>
> **阅读约定**
> - 所有“源码位置”均指参考项目仓库内的相对路径 + 行号，格式 `文件:起-止`。
> - 所有**加引号的紫色文字即界面上的中文原文（Chinese UI copy）**，必须逐字复用（含全角标点、`！`、`。`、`：`、`（…）`、`…`、`⚠️`/`✅` 之类的 emoji）。
> - 少数文案里的 `**加粗**` 是对应源码中的 `<strong>` / `<b>`，渲染时应保留加粗。
> - 少数引号里带前置 emoji（如 `⚠️ …` / `✅ …`）时，源码把这些 emoji 放在独立的 `aria-hidden` span 中与文字相邻；等价实现请在文字旁保留该 emoji，**不要重复**。
> - 形如 `{异常消息}`、`{名称}` 的花括号是占位符说明，不是界面字面量。
> - 截图位于 `FFmpegWebUI/readmefiles/`，本文以 `../../../FFmpegWebUI/readmefiles/<name>.png` 引用（相对本文档路径）。
> - 「应用名」= `FFmpeg WebUI`（`Services/AppInfo.cs:7`），「版本」= `2.0.0`（`Services/AppInfo.cs:10`）。界面出现 `@AppInfo.Name` / `v@(AppInfo.Version)` 的地方就是这两个值。

---

## 0. 技术形态与全局约束（决定了 React 版的架构）

来源：`Components/App.razor:1-46`、`Components/Routes.razor:1-24`、`docs/ui-conventions.md:1-219`

1. **交互模型**：整套 UI 是 **Blazor Server 交互式渲染**（`App.razor:24` `<Routes @rendermode="InteractiveServer" />`）。状态在服务端，每次交互都是 HTTP/WebSocket 往返后重渲染。**React 版应把长连接语义换成 HTTP 轮询 + SSE/WebSocket 事件流**，见 §15.5。
2. **任务在服务端后台队列执行**（`docs/ui-conventions.md:219`）。页面跳转、刷新、断线**都不会中断转换**。UI 只是观察者，这一条决定了所有「实时」区域都是**订阅式刷新**而不是本地计算。
3. **格式化硬约束**（`docs/ui-conventions.md:72-77`）：数字/百分比进入 CSS 或文本时必须用不变文化（否则中文区域会输出 `85,5` 这种非法 CSS 宽度）；文件大小统一走 `FormatFileSize`；时长统一走 `TimeSpan`/`DurationSeconds`。
4. **禁止在组件 `<style>` 里写死颜色**（`docs/ui-conventions.md:7`）。React 版对应做法：**只在全局 CSS 变量里定义颜色**，组件只用 `var(--token)`。
5. **首屏主题防闪白**：服务端在 `<html>` 上预置 `data-theme`（`App.razor:3`、`App.razor:41-45` 读 Cookie）；客户端脚本再校正一次（`wwwroot/js/app.js:159-161`）。
6. 页面级错误边界（`Routes.razor:3-18`）与底部全局错误条（`App.razor:26-30`）的文案见 §2.6。
7. **所有 `catch` 必须有可见反馈**（Toast 或提示条），禁止空 catch（`docs/ui-conventions.md:70`）。React 版同理：任何失败路径都必须映射到一条 Toast 或内联错误文案。

---

## 1. 路由表（Route Table）

来源：各页面顶部 `@page` 指令。React Router 建议路径与本节完全一致。

| 路由 | 页面组件 | 中文页名 | 页面目的 | 源码位置 |
| --- | --- | --- | --- | --- |
| `/` | `Home` | 首页 | 环境状态、硬件编码能力总览、最近 5 条任务、快速开始入口 | `Components/Pages/Home.razor:1` |
| `/convert` | `Convert` | 转换 | 单文件转换主流程：输入输出 → 选模板 → 填参数 → 确认命令 → 开始；并显示进行中/最近任务 | `Components/Pages/Convert.razor:1` |
| `/batch` | `BatchProcess` | 批量处理 | 多文件批量转换：加文件、选输出目录与模板、进度面板、重试全部失败 | `Components/Pages/BatchProcess.razor:1` |
| `/templates` | `Templates` | 模板管理 | 浏览/搜索/筛选/排序模板、详情查看、编辑、复制、删除、导入导出 | `Components/Pages/Templates.razor:1` |
| `/templates-admin` 与 `/templates/admin` | `TemplatesAdmin` | 预设模板管理 | 系统预设模板的完整增删改查、预设重置、运行诊断、历史清理（高级页面，**不进入主导航**） | `Components/Pages/TemplatesAdmin.razor:1-2` |
| `/tasks` | `Tasks` | 任务列表 | 进行中/等待中/历史三段式任务管理，筛选、排序、批量操作、实时日志 | `Components/Pages/Tasks.razor:1` |
| `/settings` | `Settings` | 设置 | 环境信息、硬件加速、路径、输出规则、性能与数据、界面与通知 | `Components/Pages/Settings.razor:1` |
| `/Error` | `Error` | 出错了 | 未处理异常时的错误页（含请求 ID 与排查建议） | `Components/Pages/Error.razor:1` |
| `/not-found` | `NotFound` | 404 — 页面不存在 | 路由未命中时的兜底页（同时被 `Router` 用作 `NotFoundPage`） | `Components/Pages/NotFound.razor:1`、`Routes.razor:1` |

**查询参数**
- `/convert?template=<LiteDB ObjectId>`：从模板页跳转时预选模板（`Convert.razor:587-589` 声明、`645-689` 消费）。预处理文案：
  - 成功：「已预选模板：{模板名}」
  - 非法 ID：「链接中的模板标识无效。」
  - 找不到：「链接中的模板不存在或已被删除。」
  - 读取异常：「读取预选模板失败：{异常消息}」
- 模板页「选择」按钮就是跳到 `/convert?template=<id>`（`Templates.razor:788-805`）。

**页面标题（`<PageTitle>` / `document.title`）**

| 页面 | 标题原文 |
| --- | --- |
| 全部页面（布局默认） | `FFmpeg WebUI`（`MainLayout.razor:8`） |
| 首页 | `首页 - FFmpeg WebUI` |
| 转换 | `转换 - FFmpeg WebUI` |
| 批量处理 | `批量处理 - FFmpeg WebUI` |
| 模板管理 | `模板管理 - FFmpeg WebUI` |
| 预设模板管理 | `预设模板管理 - FFmpeg WebUI` |
| 任务列表 | `任务列表 - FFmpeg WebUI` |
| 设置 | `设置 - FFmpeg WebUI`（注意：此页硬编码，不是 `@AppInfo.Name`，`Settings.razor:15`） |
| 错误页 | `出错了 - FFmpeg WebUI` |
| 404 | `页面不存在 - FFmpeg WebUI` |

---

## 2. 应用外壳、布局与导航

### 2.1 布局骨架

来源：`Components/Layout/MainLayout.razor:10-35`、`Components/Layout/MainLayout.razor.css:1-75`、`NavMenu.razor.css:1-69`

```
┌────────────┬───────────────────────────────────────────────┐
│ .sidebar   │ .top-row（sticky top, z-index 20）             │
│  232px     │   🎬 FFmpeg WebUI  [v2.0.0]   …   [进行中胶囊]  │
│ ┌────────┐ ├───────────────────────────────────────────────┤
│ │ .brand │ │ article.content（padding 1.5rem 1.75rem 3rem） │
│ ├────────┤ │   <页面内容，通常 .ffui-container 居中>          │
│ │ 导航项 │ │                                               │
│ │  ×6    │ │                                               │
│ ├────────┤ │                                               │
│ │.nav-ftr│ │                                               │
│ └────────┘ │                                               │
└────────────┴───────────────────────────────────────────────┘
                    ToastHost（右下角浮层，z-index 2000）
ReconnectModal（Blazor 断线 modal） / #blazor-error-ui（底部错误条，z-index 3000）
```

- 侧栏宽度固定 `232px`，右侧 1px `--border-color` 边框，背景 `--sidebar-bg`。
- `.top-row`：`display:flex; gap:.75rem; padding:.7rem 1.25rem; border-bottom:1px solid var(--border-color); background:var(--sidebar-bg); position:sticky; top:0; z-index:20`。
- `.content`：`flex:1; padding:1.5rem 1.75rem 3rem`。
- **响应式断点 `max-width:900px`**：`.page` 改为纵向排列；侧栏变成整宽横条（`border-right:none; border-bottom:1px solid`）；`.nav-menu` 变成横向换行（`flex-direction:row; flex-wrap:wrap`）；`.nav-footer` 隐藏；`.content` padding 变 `1rem .9rem 2.5rem`。
- 打印样式（`app.css:542-545`）：`.sidebar`、`.ffui-toast-stack`、`.top-row` 全部 `display:none !important`。

**顶栏内容（`MainLayout.razor:16-27`）**
1. `🎬 FFmpeg WebUI`（`.app-title`，font-weight 600）
2. `v2.0.0`（`.app-version.ffui-badge`，等宽字体 0.72rem）
3. 弹性空隙 `.top-spacer`
4. **仅当「运行中 > 0 或 排队 > 0」时出现**的运行指示胶囊 `.top-running`，是一个指向 `/tasks` 的链接（`title="查看任务列表"`），内部：
   - 一个 `.spinner` 旋转图标（`aria-hidden="true"`）
   - 文案：`{N} 个转换进行中`，若排队数 > 0 则追加 `（{M} 个排队）`
   - 例：`2 个转换进行中（3 个排队）`
   - 样式：圆角胶囊、`background:var(--info-bg); color:var(--info-text); border:1px solid var(--info-color)`、font-size .82rem

该计数由 `TaskService.TaskStatusChanged` 事件驱动（`MainLayout.razor:54-56, 63-66, 105-116`），即**实时**更新。

**浏览器通知（`MainLayout.razor:79-103`，由 MainLayout 统一负责，因此任何页面都能收到）**
- 触发条件：任务进入 `Completed / Failed / Cancelled / Skipped` 且设置项 `NotifyOnCompletion` 为真。
- 通知标题/正文（`MainLayout.razor:89-95`）：
  | 状态 | title | body |
  | --- | --- | --- |
  | 已完成 | `✅ 转换完成` | 文件名 |
  | 失败 | `❌ 转换失败` | `{文件名}\n{错误消息}` |
  | 已取消 | `已取消` | 文件名 |
  | 已跳过 | `已跳过` | 文件名 |
- 任务取不到时文件名回退为 `任务`。通知被浏览器拦截时**静默失败**（不影响其它功能）。

### 2.2 侧栏导航（NavMenu）

来源：`Components/Layout/NavMenu.razor:1-36`、`NavMenu.razor.css:1-69`

- 品牌区 `.brand`：`🎬`（`aria-hidden`） + 文本 `FFmpeg WebUI`。
- `<nav class="nav-menu" aria-label="主导航">`，**顺序即下表顺序**（`NavMenu.razor:27-35`）：

| # | 路由 | 图标 | 中文标签 | 高亮匹配规则 |
| --- | --- | --- | --- | --- |
| 1 | ``（即 `/`） | 🏠 | 首页 | `NavLinkMatch.All`（**精确匹配**） |
| 2 | `convert` | 🔄 | 转换 | 前缀匹配 |
| 3 | `batch` | 📁 | 批量处理 | 前缀匹配 |
| 4 | `templates` | 📋 | 模板 | 前缀匹配 |
| 5 | `tasks` | 🗂️ | 任务列表 | 前缀匹配 |
| 6 | `settings` | ⚙️ | 设置 | 前缀匹配 |

> ⚠️ 注意：`/templates-admin` 会因前缀匹配把「模板」项点亮（`NavLinkMatch.Prefix`）。React 版若想避免，需要显式排除。
- 导航项样式：`.nav-link` 为 flex 行，icon 固定宽 `1.2rem` 居中；hover → `background:var(--surface-3); color:var(--text-primary)`；active → `background:var(--primary-light); color:var(--primary-dark); font-weight:600`。
- 侧栏页脚 `.nav-footer`（`NavMenu.razor:19-22`，`margin-top:auto` 顶到底部）：
  - 第一行：`{平台显示名} · .NET {主版本号}`（例：`macOS · .NET 9`）
  - 第二行：`v{版本}`，等宽字体（class `mono`）

### 2.3 主题切换（浅色 / 深色 / 跟随系统）与持久化

来源：`wwwroot/js/app.js:8-41`、`App.razor:1-45`、`MainLayout.razor:41-52, 118-129`、`Settings.razor:587-595, 1413-1449`

- **三档取值**：`System`（跟随系统）、`Light`（浅色）、`Dark`（深色）。
- **唯一的切换入口是设置页**的下拉选择 `<select id="theme-select">`（`Settings.razor:589-593`），选项文案：`跟随系统` / `浅色` / `深色`。**顶栏与侧栏都没有主题切换按钮**。
- 帮助文案（`Settings.razor:594`）：`切换后立即生效并自动记住，下次打开时保持（不需要点「保存设置」，也不需要刷新页面）。`
- **生效机制**：
  1. `ffui.applyTheme(theme)` → 解析 `System`（用 `matchMedia('(prefers-color-scheme: dark)')`），把解析结果写到 `<html data-theme="Light|Dark">`，同时把原始三档值写到 `<html data-theme-mode="System|Light|Dark">`（`app.js:10-18`）。
  2. `ffui.setThemeCookie(theme)` → 写 Cookie `ffui-theme=<encodeURIComponent(theme)>;path=/;max-age=31536000;SameSite=Lax`（`app.js:22-30`）。
  3. 服务端渲染时读该 Cookie 预置 `<html data-theme-mode>` / `data-theme`，避免深色系统下白色闪屏（`App.razor:3, 41-45`）。
  4. `ffui.watchSystemTheme()` 注册 `prefers-color-scheme` 的 change 监听：**仅当 `data-theme-mode === 'System'`** 时才同步 `data-theme`（`app.js:32-41`）。
  5. 首屏脚本末尾立刻 `applyTheme(文档上的 data-theme-mode || 'System')` 并 `watchSystemTheme()`（`app.js:159-161`）。
- **持久化**：改主题时**只单独保存 Theme 字段**到数据库，不整份表单保存，避免把其它未保存改动一起写进去（`Settings.razor:1402-1433` 注释与实现）。同时同步「已保存」快照，避免一直显示「有未保存的更改」。
- 失败文案（`Settings.razor:1431`）：`主题已在本页生效，但没能记住（保存设置时会一起写入）：{异常消息}`；页面预览失败（`1435-1449`）：`主题已记录，但本页预览失败（浏览器连接不可用）：{异常消息}`。
- 数据库里的主题用的是枚举字符串 `System` / `Light` / `Dark`（`Settings.razor:841` 默认 `System`）。

### 2.4 设计令牌（Design Tokens）—— 完整清单与浅/深色取值

来源：`wwwroot/app.css:1-124`（**这是唯一允许写颜色字面量的地方**）

**浅色（默认，`:root`，`app.css:10-72`）**

| 令牌 | 浅色值 | 用途 |
| --- | --- | --- |
| `--primary-color` | `#2563eb` | 品牌主色（按钮、选中态、进度条、链接） |
| `--primary-dark` | `#1d4ed8` | 主色 hover / 徽标文字 |
| `--primary-light` | `#dbeafe` | 主色浅底（选中行、primary 徽标底） |
| `--primary-contrast` | `#ffffff` | 主色底上的文字 |
| `--success-color` | `#16a34a` | 成功前景 |
| `--success-bg` | `#dcfce7` | 成功底 |
| `--success-text` | `#14532d` | 成功底上的文字 |
| `--danger-color` | `#dc2626` | 危险/失败前景 |
| `--danger-bg` | `#fee2e2` | 危险底 |
| `--danger-text` | `#7f1d1d` | 危险底上的文字 |
| `--warning-color` | `#d97706` | 警告前景 |
| `--warning-bg` | `#fef3c7` | 警告底 |
| `--warning-text` | `#78350f` | 警告底上的文字 |
| `--info-color` | `#0891b2` | 信息/运行中前景 |
| `--info-bg` | `#cffafe` | 信息底 |
| `--info-text` | `#164e63` | 信息底上的文字 |
| `--body-bg` | `#f5f6f8` | body 背景 |
| `--page-bg` | `#f5f6f8` | `.page` 背景 |
| `--main-bg` | `#f5f6f8` | `main` 背景 |
| `--card-bg` | `#ffffff` | 卡片/面板/弹窗底 |
| `--surface-2` | `#ffffff` | 次级表面 |
| `--surface-3` | `#f1f3f5` | 三级表面（表头、页脚、hover、次级卡片） |
| `--sidebar-bg` | `#ffffff` | 侧栏与顶栏底 |
| `--input-bg` | `#ffffff` | 输入框底 |
| `--light-bg` | `#f1f3f5` | 浅底（遗留别名） |
| `--code-bg` | `#eff1f5` | 代码块/日志区底 |
| `--overlay-bg` | `rgba(15, 23, 42, 0.55)` | 模态遮罩 |
| `--border-color` | `#d7dbe0` | 常规边框 |
| `--border-strong` | `#b9c0c9` | 强边框 / 滚动条滑块 / 待办进度条 |
| `--text-primary` | `#1f2937` | 主文字 |
| `--text-secondary` | `#4b5563` | 次级文字 |
| `--text-muted` | `#6b7280` | 弱化文字 |
| `--text-inverse` | `#ffffff` | 反色文字 |
| `--shadow-sm` | `0 1px 2px rgba(15, 23, 42, 0.08)` | 轻阴影 |
| `--shadow-md` | `0 4px 14px rgba(15, 23, 42, 0.10)` | 中阴影（hover 卡片、Toast） |
| `--shadow-lg` | `0 12px 32px rgba(15, 23, 42, 0.18)` | 重阴影（模态） |
| `--radius-sm` | `6px` | 小圆角（按钮、输入、徽标容器） |
| `--radius-md` | `10px` | 中圆角（面板、卡片） |
| `--radius-lg` | `16px` | 大圆角（模态） |
| `--focus-ring` | `0 0 0 3px rgba(37, 99, 235, 0.35)` | 输入聚焦光晕 |
| `--font-mono` | `ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace` | 等宽 |
| `--spacing-1` | `0.25rem` | 间距梯度 |
| `--spacing-2` | `0.5rem` | |
| `--spacing-3` | `0.75rem` | |
| `--spacing-4` | `1rem` | |
| `--spacing-5` | `1.5rem` | |
| `--spacing-6` | `2rem` | |

**深色（`[data-theme="Dark"]`，`app.css:75-124`）；只有这些变量换值**

| 令牌 | 深色值 | | 令牌 | 深色值 |
| --- | --- | --- | --- | --- |
| `--primary-color` | `#4d8dff` | | `--overlay-bg` | `rgba(0, 0, 0, 0.7)` |
| `--primary-dark` | `#3b76e0` | | `--border-color` | `#333940` |
| `--primary-light` | `#1e2d4a` | | `--border-strong` | `#4a525c` |
| `--primary-contrast` | `#ffffff` | | `--text-primary` | `#e8eaed` |
| `--success-color` | `#4ade80` | | `--text-secondary` | `#b6bcc5` |
| `--success-bg` | `#14351f` | | `--text-muted` | `#8b929c` |
| `--success-text` | `#bbf7d0` | | `--text-inverse` | `#121417` |
| `--danger-color` | `#f87171` | | `--shadow-sm` | `0 1px 2px rgba(0,0,0,0.5)` |
| `--danger-bg` | `#3b1717` | | `--shadow-md` | `0 4px 14px rgba(0,0,0,0.55)` |
| `--danger-text` | `#fecaca` | | `--shadow-lg` | `0 12px 32px rgba(0,0,0,0.65)` |
| `--warning-color` | `#fbbf24` | | `--focus-ring` | `0 0 0 3px rgba(77, 141, 255, 0.45)` |
| `--warning-bg` | `#3a2c0c` | | `--body-bg` `--page-bg` `--main-bg` | `#121417` |
| `--warning-text` | `#fde68a` | | `--card-bg` `--surface-2` | `#1c1f24` |
| `--info-color` | `#38bdf8` | | `--surface-3` | `#262a30` |
| `--info-bg` | `#0d2f3d` | | `--sidebar-bg` | `#17191d` |
| `--info-text` | `#bae6fd` | | `--input-bg` `--light-bg` | `#22262c` |
| | | | `--code-bg` | `#17191d` |

深色块同时设置 `color-scheme: dark`（`app.css:76`），浅色块 `color-scheme: light`（`app.css:11`）。`<head>` 里还有 `<meta name="color-scheme" content="light dark">`（`App.razor:8`）。

**基础排版**（`app.css:126-182`）
- `html, body`：`font-family: "Helvetica Neue", Helvetica, Arial, "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif`；`font-size: 15px`；`line-height: 1.6`。
- `h1` `1.6rem/650`；`h2` `1.2rem/600`；`h3` `1.05rem/600`；标题行高 1.3。
- `a`：`color:var(--primary-color)`，hover `var(--primary-dark)`。
- `code, pre, .mono`：等宽 + `--code-bg` 底 + `--radius-sm`；`code` padding `0.1rem 0.35rem`、`font-size:0.85em`；`pre` padding `--spacing-3`、`overflow-x:auto`、1px 边框。
- **全局键盘焦点**：`:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; border-radius: var(--radius-sm); }`；并且 `h1:focus { outline:none }`、`h1:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 4px }`（因为路由切换会把焦点移到 `h1`）。
- `.visually-hidden`：绝对定位 1×1、`clip:rect(0,0,0,0)`、`white-space:nowrap`（无障碍专用文本）。

**滚动条**（`app.css:526-540`）：`scrollbar-width:thin`、`scrollbar-color:var(--border-strong) transparent`；WebKit：宽/高 10px，滑块 `--border-strong` 圆角，hover 变 `--text-muted`。

### 2.5 全局可复用 class 清单（React 版必须提供等价原子类或 CSS Modules）

来源：`app.css:184-483`、`docs/ui-conventions.md:29-40`

**表单控件**（`app.css:184-222`，直接作用于元素选择器）
- 覆盖 `input[type=text|number|search|url]`、`select`、`textarea`：`background:var(--input-bg); border:1px solid var(--border-color); border-radius:var(--radius-sm); padding:.5rem .7rem; font-size:.95rem; width:100%; box-sizing:border-box`，`transition: border-color .15s, box-shadow .15s`。
- `:focus` → `outline:none; border-color:var(--primary-color); box-shadow:var(--focus-ring)`。
- `:disabled` → `background:var(--surface-3); color:var(--text-muted); cursor:not-allowed`。
- `::placeholder` → `color:var(--text-muted)`。
- `input[type=checkbox|radio]` → `width:auto; accent-color:var(--primary-color)`。

**按钮族**

| class | 视觉 |
| --- | --- |
| `.ffui-btn` | inline-flex + 居中 + `gap:.4rem`，`padding:.5rem 1rem`，`background:var(--surface-3)`，`color:var(--text-primary)`，`border:1px solid transparent`，`border-radius:var(--radius-sm)`，`font-size:.92rem`，`cursor:pointer`；hover（非 disabled）→ `background:var(--border-color)`；`:disabled` → `opacity:.55; cursor:not-allowed` |
| `.ffui-btn-primary` | 底/边 `--primary-color`，文字 `--primary-contrast`；hover → `--primary-dark` |
| `.ffui-btn-danger` | 底/边 `--danger-color`，文字 `#fff`；hover → `filter:brightness(0.92)` |
| `.ffui-btn-ghost` | 透明底 + `border-color:var(--border-color)`；hover → `--surface-3` |
| `.ffui-btn-sm` | `padding:.3rem .65rem; font-size:.85rem` |
| `.ffui-btn-lg` | `padding:.75rem 1.6rem; font-size:1rem` |
| `.ffui-icon-btn` | 透明、无边框、`color:var(--text-secondary)`、`line-height:1`；hover → `background:var(--surface-3); color:var(--text-primary)`。**规范要求必须带 `aria-label`** |

**面板 / 徽标 / 提示条 / 空状态 / 容器**
- `.ffui-panel`：`background:var(--card-bg); border:1px solid var(--border-color); border-radius:var(--radius-md); padding:var(--spacing-4)`。
- `.ffui-panel-title`：`margin:0 0 var(--spacing-3)`，`font-size:1rem; font-weight:600`。
- `.ffui-badge`：inline-flex、`gap:.25rem`、`padding:.12rem .5rem`、`border-radius:999px`、`font-size:.76rem`、`font-weight:500`、默认 `background:var(--surface-3); color:var(--text-secondary)`、`white-space:nowrap`；修饰类 `-primary`（`--primary-light`/`--primary-dark`）、`-success`、`-danger`、`-warning`、`-info`（各自 `-bg`/`-text` 组合）。
- `.ffui-alert`：flex + `gap:.6rem` + `align-items:flex-start`，`padding:.7rem .9rem`，`border-radius:var(--radius-sm)`，1px 透明边框；`-info|-success|-warning|-danger` 分别用对应三元组 + 该语义色描边。
- `.ffui-empty-state`：`padding:var(--spacing-6) var(--spacing-4); text-align:center; color:var(--text-muted); font-size:.92rem`。
- `.ffui-container`：`max-width:1320px; margin:0 auto; padding:var(--spacing-5) var(--spacing-4) var(--spacing-6)`。
- `.ffui-section-title`：flex + `gap:var(--spacing-2)`，`margin:0 0 var(--spacing-3)`，`font-size:1.1rem; font-weight:600`。
- `.ffui-progress-track`：高 10px，圆角 999px，`background:var(--surface-3)`，`overflow:hidden`；`.ffui-progress-fill`：满高，`background:var(--primary-color)`，`transition: width .25s ease`。
- 加载：`.spinner`（14×14，2px 边框，`border-top-color:var(--primary-color)`，`ffui-spin .8s linear infinite`）、`.spinner-lg`（28×28，3px）、`.ffui-loading-row`（flex + gap + `color:var(--text-muted)` + `padding:var(--spacing-4)`）。**规范明确禁止组件内重复定义 `@keyframes spin`。**

### 2.6 模态、Toast、确认框、键盘无障碍（全局约定）

#### 2.6.1 Modal（`Components/Shared/Modal.razor:1-102`）

**结构与行为**
- 参数：`Title`、`IsOpen`、`IsOpenChanged`、`Size`（`sm|md|lg|xl`，默认 `md`）、`CloseOnBackdrop`（默认 true）、`DisableClose`、`ChildContent`（**唯一渲染片段，没有 FooterContent**）。
- 渲染：`IsOpen` 为真时输出 `.ffui-modal-backdrop`（`onclick` 关外），内部 `.ffui-modal @SizeClass`，属性 `role="dialog" aria-modal="true" aria-labelledby="{生成的唯一标题id}" tabindex="-1"`，并且 `@onclick:stopPropagation`。
- 头部 `.ffui-modal-header`：`<h2 id=...>` 标题 + 关闭按钮 `<button class="ffui-icon-btn" aria-label="关闭对话框">✕</button>`，`disabled="@DisableClose"`。
- 尺寸映射（`Modal.razor:30-38` 内联样式）：`.ffui-modal-sm` `min(420px,100%)`、`.ffui-modal-md` `min(720px,100%)`、`.ffui-modal-lg` `min(1040px,100%)`、`.ffui-modal-xl` `min(1320px,100%)`；基础 `.ffui-modal` 为 `min(720px,100%)`、`max-height:min(88vh,900px)`、`display:flex; flex-direction:column; overflow:hidden`、`border-radius:var(--radius-lg)`、`box-shadow:var(--shadow-lg)`。
- 遮罩：`position:fixed; inset:0; background:var(--overlay-bg); display:flex; align-items:center; justify-content:center; padding:var(--spacing-4); z-index:1000`。
- 主体 `.ffui-modal-body`：`padding:var(--spacing-5); overflow-y:auto; flex:1`。
- 底部按钮栏 `.ffui-modal-footer`（定义在 `app.css:381-394`，**由调用方作为内容的最后一个元素写出**）：`justify-content:flex-end; gap:var(--spacing-2); margin:var(--spacing-5) calc(-1*var(--spacing-5))`；`padding:.75rem 1.5rem; border-top:1px solid var(--border-color); background:var(--surface-3); position:sticky; bottom:calc(-1*var(--spacing-5)); flex-wrap:wrap`。
- **焦点**：打开时（`IsOpen` 由假变真且渲染完成）调用 `ffui.focusElement(对话框元素)` 把焦点移进对话框，键盘用户不会落到背后页面（`Modal.razor:74-86`）。**没有实现焦点陷阱（focus trap），也没有关闭后归还焦点**——React 版强烈建议补上。
- **Esc 关闭**：由全局 `keydown` 捕获实现（`app.js:140-150`）：取 `document.querySelectorAll('.ffui-modal')` 的**最后一个**（最上层），找它的 `.ffui-modal-header .ffui-icon-btn`（即关闭按钮），若未 disabled 则 `preventDefault()` 并 `.click()`——即 Esc 只是「点一下关闭按钮」，状态仍由组件处理。捕获阶段 `true`。
- 关闭路径统一走 `CloseAsync()`：若 `DisableClose` 直接返回；否则内部 `IsOpen=false` 再 `await IsOpenChanged.InvokeAsync(false)`。
- **文本输入型弹窗的关闭拦截**：模板编辑器会在有未保存修改或正在保存/试运行时，收到 `false` 后**立刻回推 `true`**，改用 ConfirmDialog 询问（`TemplateEditor.razor:1000-1021`）。

#### 2.6.2 ConfirmDialog（`Components/Shared/ConfirmDialog.razor:1-89`）

- 参数：`IsOpen`、`IsOpenChanged`、`Title`（默认 `确认操作`）、`Message`、`Detail`（可空，用于完整路径等补充）、`ConfirmText`（默认 `确定`）、`CancelText`（默认 `取消`）、`Danger`、`OnConfirm`、`OnCancel`、`IsBusy`。
- 内部已包含 `<Modal>`（`Size="sm"`、`CloseOnBackdrop="@(!IsBusy)"`、`DisableClose="@IsBusy"`），**调用方不要再套一层 Modal**。
- 正文布局：左侧图标 `⚠️`（Danger=true）或 `❓`（Danger=false），font-size 1.6rem；右侧 `Message`（`--text-primary`）与可选的 `Detail`（`--text-muted`，0.86rem，`word-break:break-all` 以容纳长路径）。
- 底部按钮：`取消`（`.ffui-btn`）+ 确认按钮（`.ffui-btn` + 若 Danger 则 `.ffui-btn-danger`，否则 `.ffui-btn-primary`）；`IsBusy` 时两键都 disabled，确认按钮内前置 `.spinner`。
- 取消/关闭（含 Esc、遮罩）都会触发 `OnCancel`；`OnOpenChanged(false)` 也会触发 `OnCancel`（`ConfirmDialog.razor:70-75`）。

#### 2.6.3 Toast（`Components/Shared/ToastHost.razor:1-50`、`Services/ToastService.cs`）

- 挂载在 MainLayout，唯一实例；通过 `IToastService.Show/Success/Warning/Error` 调用。规范禁止页面自己写横幅（`docs/ui-conventions.md:55`）。
- 渲染：`.ffui-toast-stack`（`position:fixed; right:1rem; bottom:1rem; flex column; gap:.5rem; z-index:2000; max-width:min(420px, calc(100vw - 2rem))`），容器带 `role="status" aria-live="polite"`。
- 单条 `.ffui-toast`：flex、`gap:.5rem`、`padding:.65rem .9rem`、`border-left:4px solid var(--primary-color)`、`background:var(--card-bg)`、`box-shadow:var(--shadow-md)`、`font-size:.9rem`、入场动画 `ffui-toast-in .18s ease-out`（从 `opacity:0; translateY(6px)` 到正常）。
- 级别 → 左边框色 / 图标 / 类名：
  | 级别 | 边框色 | 图标 | CSS 类 | 默认时长 |
  | --- | --- | --- | --- | --- |
  | Info | `--primary-color` | `ℹ️` | （无） | 4000 ms |
  | Success | `--success-color` | `✅` | `ffui-toast-success` | 4000 ms |
  | Warning | `--warning-color` | `⚠️` | `ffui-toast-warning` | 5000 ms |
  | Danger | `--danger-color` | `❌` | `ffui-toast-danger` | 8000 ms |
- 每条右侧有 `<button class="ffui-icon-btn ffui-toast-close" aria-label="关闭提示">✕</button>`；文本区 `.ffui-toast-text` 为 `flex:1; word-break:break-word`。
- **最多同时 5 条**（超出丢弃最旧的）；`durationMs > 0` 时到时自动消失；空白文本直接忽略；文本会 `Trim()`。

#### 2.6.4 键盘与无障碍规则汇总

1. 全局 `:focus-visible` 使用 2px 主色描边 + 2px 偏移（`app.css:166-170`）。
2. 路由跳转后焦点移到 `h1`（`Routes.razor:6` `FocusOnNavigate Selector="h1"`），且 `h1` 的 focus 样式被单独放宽到 4px 偏移。
3. 所有图标按钮必须有 `aria-label`（`.ffui-icon-btn` 规则，`ui-conventions.md:32`）。实际文案示例：`关闭对话框`、`关闭提示`、`复制日志`、`下载日志`、`清空显示的日志`、`移除 {文件名}`、`查看模板详情`、`复制命令参数`、`上移参数 {名}`、`下移参数 {名}`、`删除参数 {名}`、`上移选项`、`下移选项`、`删除选项`、`移除输入格式 {fmt}`、`移除标签 {tag}`、`浏览并选择{Label}（文件|文件夹）`、`搜索日志`、`日志级别筛选`。
4. 表单控件用 `<label for>` 关联，隐藏型标签用 `.visually-hidden`。
5. 进度条：`role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="{百分比}" aria-valuetext="{标签} {百分比}%，{状态}" aria-label="{标签}"`（`ProgressBar.razor:11-15, 101-109`）。
6. 日志区：`role="log" aria-live="off" aria-labelledby="{标题id}"`（避免刷屏朗读，`LogViewer.razor:57-58`）。
7. 快捷键：**只有 Esc**（关闭最上层模态）。模板列表/卡片等处用真 `<button>` + `aria-pressed` 表达选中，因此回车/空格可用（`TemplateSelector.razor:8`）。
8. `@key` 用于所有循环渲染（Blazor 要求）；React 版对应 `key` 用 `template.Id` / `task.Id` / 文件名等稳定标识。
9. 设置页表单操作区是 `role="group" aria-label="设置操作"`（`Settings.razor:621`）。
10. 表格排序按钮：`<th aria-sort="none|ascending|descending">` + 内部 `<button class="th-sort">`（`TemplatesAdmin.razor:106-140, 763-771`），指示符 `↕` / `▲` / `▼`。

#### 2.6.5 断线与致命错误提示（React 版可忽略「断线」，但要保留错误条文案）

- Blazor 重连 modal（`Components/Layout/ReconnectModal.razor:8-38`），`<dialog id="components-reconnect-modal">`，各状态文案：
  - `正在重新连接服务…`
  - `重新连接失败，将在 {N} 秒后重试。`
  - `无法重新连接到服务。<br />请确认本应用仍在运行，然后重试或重新加载页面。` + 按钮 `重试`
  - `会话已被服务端暂停。` + 按钮 `恢复会话`
  - `恢复会话失败。<br />请重新加载页面。`
  - 固定提示：`提示：转换任务在服务端后台队列中执行，断线或刷新页面都不会中断正在进行的转换。`
- 底部致命错误条（`App.razor:26-30`）：`页面发生了未处理的错误。` + 链接 `重新加载` + `🗙` 关闭（`role="button" tabindex="0"`）。
- 渲染失败边界（`Routes.razor:9-17`）：`页面渲染失败：{异常消息}` + 按钮 `重新加载页面`；`.blazor-error-boundary` 有 `::before { content: "⚠️ 出错了："; font-weight:600 }`。

---

## 3. 共享组件逐个规格（React 直接对应为 component）

### 3.1 `ProgressBar` —— 转换进度条

来源：`Components/Shared/ProgressBar.razor:1-263`

**Props**：`Label`（默认 `转换进度`）、`Progress`（0–100）、`Speed`（可空）、`Eta`（秒，可空）、`CurrentTime`、`TotalDuration`、`Status`（`TaskStatus?`，null 表示调用方不提供状态）。

**视觉结构（自上而下）**
1. 头部行 `.progress-header`：左侧 `.progress-label`（Label），右侧 `.progress-percentage`（`{百分比}%`，保留 **1 位小数**，颜色随状态：默认 `--primary-color`，completed `--success-color`，failed `--danger-color`，cancelled `--warning-color`）。
2. 轨道 `.ffui-progress-track` + `.ffui-progress-fill`（宽度 = 百分比），填充色随状态：pending `--border-strong`、running `--primary-color`、completed `--success-color`、failed `--danger-color`、cancelled/skipped `--warning-color`。悬停/进度容器底为 `--surface-3`，圆角 `--radius-md`，`padding:1rem`。
3. 统计行 `.progress-stats`（flex、`gap:1.5rem`、可换行），每个 `.stat-item` 是「小标签在上、值在下（等宽字体 0.95rem）」，按条件显示：
   | 条件 | 标签 | 值格式 |
   | --- | --- | --- |
   | `CurrentTime > 0 && TotalDuration > 0` | `已用/总时长` | `mm:ss / mm:ss`（≥1 小时则 `H:MM:SS`，小时不补零） |
   | `TotalDuration <= 0 && CurrentTime > 0` | `已用时长` | 同上单值 |
   | Speed 有值且 > 0 | `速度` | `{0.##}x`（如 `1.23x`） |
   | Eta 有值且 > 0 | `剩余时间` | `mm:ss` / `H:MM:SS` |
   | Status 非空 | （无标签） | 状态徽标，`title="{枚举名}"`，中文见下表 |
4. 状态徽标样式（`.status-badge`，padding `.25rem .75rem`，圆角 `--radius-sm`，font-size .85rem）：
   | 状态 | 中文 | 徽标底/字 |
   | --- | --- | --- |
   | `Completed` | `已完成` | `--success-bg` / `--success-text` |
   | `Failed` | `失败` | `--danger-bg` / `--danger-text` |
   | `Running` | `运行中` | `--info-bg` / `--info-text` |
   | `Cancelled` | `已取消` | `--warning-bg` / `--warning-text` |
   | `Skipped` | `已跳过` | `--surface-2` / `--text-muted` |
   | `Pending` / 其它 | `等待中` | `--surface-2` / `--text-secondary` |

**数值健壮性（务必照抄）**
- 百分比必须 `clamp(0,100)` 且 `NaN/Infinity → 0`；文本用不变文化 `F1` 输出（`ProgressBar.razor:84-89`）。
- 时间为负/非有限时按 0；上限 `int.MaxValue` 秒，避免溢出（`155-164`）。
- **进度条自身不判断业务状态**：`Status` 为 null 时，`100%` 也不会显示成「已完成」（`111` 注释）。

### 3.2 `LogViewer` —— 日志查看器（页内嵌入式）

来源：`Components/Shared/LogViewer.razor:1-617`

**Props**：`LogText`（整段日志字符串）、`Title`（默认 `FFmpeg 日志`）、`MaxHeight`（如 `260px`、`320px`）、`AutoScroll`（默认 true，仅作初始值）、`MaxLines`（默认 1000）。

**头部 `.log-header`（`--surface-3` 底，下边框）**
- 标题：`📜 {Title}`（id 供 `aria-labelledby` 使用）。
- 操作区（顺序固定）：
  1. 搜索框 `<input type="search" class="search-input" placeholder="搜索...">`（宽 150px，视觉隐藏的 `label` 文本为 `搜索日志`）。防抖 **250 ms**，搜索**字面量、不区分大小写**，命中片段用 `<mark>` 高亮（黄色系 `--warning-bg`/`--warning-text`）。
  2. 级别筛选 `<select>`（`aria-label="日志级别筛选"`），选项：`全部` / `仅错误` / `仅警告`。
  3. 复制日志按钮（`.ffui-icon-btn`，`aria-label="复制日志"`，`title="复制日志"`），成功状态图标由 `📋` 变 `✓`，持续 **1500 ms**。
  4. 下载日志按钮（`aria-label="下载日志"`，图标 `⬇️`）。
  5. 清空按钮（`aria-label="清空显示的日志"`，图标 `🗑️`）。
  6. `自动滚动` 复选框（默认勾选）。
- JS 初始化失败时在头部下方显示一条 `ffui-alert ffui-alert-warning` 提示（`role="status"`）：`自动滚动初始化失败，日志仍可查看，但不会自动滚到底部。`（同时弹一条 Warning Toast，附加 `（{异常消息}）`）。

**内容区 `.log-content`**：`--code-bg` 底、等宽字体、0.85rem、`overflow-y:auto`；每行 `.log-line` 由「行号（右对齐、min-width 44px、`user-select:none`、`--text-muted`）+ 内容（`word-break:break-all`）」组成。
- 行着色规则（按单词边界正则，避免文件名里的 error/failed 被误判，`LogViewer.razor:114-122, 415-428`）：
  | 判定 | 匹配内容 | 样式 |
  | --- | --- | --- |
  | `error` | `error(s)` / `failed` / `failure` / `invalid` / `fatal` / `unable` / `cannot` / `not found` / `no such file` | 底色 `--danger-bg`，文字 `--danger-text` |
  | `warning` | `warning(s)` / `warn` / `deprecated` | 底色 `--warning-bg`，文字 `--warning-text` |
  | `progress` | 包含 `time=` 或 `speed=` | 文字 `--info-text` |
  | 普通 | 其它 | 无底色；hover 时底色 `--surface-3`（**误/警行 hover 不覆盖语义底色**） |

**空状态与清空状态（精确文案）**
- 无任何日志：`暂无日志输出`
- 有日志但被搜索/筛选过滤光：`没有符合当前搜索/筛选条件的日志行`
- 用户点了清空：`日志已清空（新日志仍会继续显示）` + 按钮 `恢复显示`
- 页脚统计（`.log-footer`，`--surface-3` 底、0.78rem、`--text-muted`）：
  - 始终：`共 {总数} 行`
  - 有过滤且数量不同：`· 显示 {过滤后} 行`
  - 有搜索词或级别非「全部」：`· 匹配 {过滤后} 行`
  - 已清空：`· 已清空显示`

**行为细节（React 需复刻）**
- 文本变化时重建行集合，**只保留最后 `MaxLines` 行**（默认 1000），按 `\n` 切分、丢弃空行、每行 `TrimEnd('\r')`。
- 「清空」只是 UI 层隐藏，并记录 `_clearedMarker`（清空时的文本前缀）；**当传入的是完全不同的新日志流（不以该前缀开头）时自动恢复显示**。
- 自动滚动：贴底阈值 32px；用户手动上滚超过 32px 后 `_autoStick=false`，不再强制拉回底部；重新勾选「自动滚动」立即滚到底；仅当可见行数变化时才调用滚动 JS。
- 复制：日志为空 → Warning `没有可复制的日志。`；复制失败 → Error `复制失败，可能是浏览器不允许访问剪贴板（非安全来源）。可手动选中文本复制。`；异常 → Error `复制失败：{异常消息}。可手动选中文本复制。`；成功 → Success `日志已复制到剪贴板。`（复制的是**当前可见/过滤后的行**）。
- 下载：无日志 → Warning `暂无日志可下载。`；文件名 `ffmpeg-log-yyyyMMdd-HHmmss.txt`，MIME `text/plain;charset=utf-8`（下载的是**全部原始行**）；成功 → Success `日志已保存为 {文件名}`；浏览器拒绝 → Error `日志下载失败：浏览器拒绝了下载操作。`；异常 → Error `日志下载失败：{异常消息}`。
- 滚动失败提示（仅一次）：`自动滚动失败：{异常消息}`。

### 3.3 `FileSelector` —— 路径输入 + 浏览（文件/文件夹）

来源：`Components/Shared/FileSelector.razor:1-423`

> **重要设计变更**：由于浏览器只暴露拖入文件的 `File.name`、拿不到绝对路径，**拖放能力已被移除**，界面也不再出现「拖拽文件到此处」的提示（`FileSelector.razor:7-12` 注释）。

**Props**：`Label`（默认 `文件路径`）、`Value`、`ValueChanged`、`Placeholder`（默认 `输入文件路径，或点击「浏览」选择`）、`ShowBrowse`（默认 true）、`ShowFileInfo`（默认 true）、`ValidateExists`（默认 true）、`AllowedExtensions`（可空列表）、`IsDirectory`、`Disabled`。

**视觉结构**
1. `<label>`（中文标签，`for` 指向输入框 id）。
2. `.input-group`（flex，`gap:.5rem`）：文本框（等宽字体 0.9rem，`flex:1 1 auto`，`spellcheck=false`，`autocomplete=off`）+ 浏览按钮。
   - 浏览按钮 `.ffui-btn.ffui-btn-primary.browse-btn`：内容为 `📂` + `浏览`；浏览中变成 `.spinner` + `浏览`（此时 disabled，`aria-busy="true"`）；`aria-label` = `浏览并选择{Label}（文件）` 或 `浏览并选择{Label}（文件夹）`。disabled 条件：`Disabled || _isBrowsing`（防双击弹两个对话框）。
3. 校验错误行（存在时）：`⚠️ {错误文案}`，红色（`--danger-color`），0.85rem。
4. 文件信息行（仅非目录、`ShowFileInfo`、文件存在且文本非空）：`文件大小: {格式化大小}`，`--text-muted`，0.85rem。

**输入行为**
- 输入框 `oninput` 立即本地校验并显示错误（即时反馈），但**回调父组件延迟 350 ms**（防抖，每次新输入取消上一次）。
- `onchange`（失焦/回车）立即提交，不等防抖。
- 提交前规范化：`Trim()`；若首尾都是 `"` 则去掉引号（支持资源管理器「复制为路径」）。
- 规范化后的值通过 `ValueChanged` 回传；回调抛异常 → Toast.Error `路径未能保存：{异常消息}`。
- 父组件的值只在**真的变化**时才覆盖本地文本（区分「外部改值」与「用户正在输入」，`131-143`）。
- 当 `ValidateExists=false` 时也要显示文件大小（大小探测与存在性校验相互独立）。
- 路径里的 `~` 会被展开、并规整为绝对路径，**仅用于校验与打开对话框，绝不回写用户输入**（`224` 注释）。

**校验错误文案（精确）**
| 触发条件 | 文案 |
| --- | --- |
| 目录不存在（`IsDirectory=true`） | `目录不存在` |
| 文件不可访问 | `文件不存在或无法访问` |
| 扩展名不在允许列表（仅当没有其它错误且扩展名非空） | `不支持的文件格式，允许: {ext1, ext2, …}` |
| 探测过程抛异常 | `路径无法读取：{异常消息}` |
| `Path.GetExtension` 抛异常且无其它错误 | `路径格式无法识别：{异常消息}` |

**浏览按钮的失败文案（三档，逐字）**
1. 平台不支持对话框后端（点击时立即判断）：
   `当前环境不支持系统文件对话框（{后端描述}），请在输入框中手动粘贴路径。`（Warning）
2. 返回 null（用户取消**或**后端不可用，两种都提示）：
   `没有选择{文件|文件夹}：可能已取消，或系统对话框（{后端描述}）不可用。可直接把路径粘贴到输入框。`（Warning）
3. 打开对话框抛异常：
   `打开文件对话框失败：{异常消息}（对话框后端：{后端描述}）。可直接把路径粘贴到输入框。`（Error）
4. 无法识别当前路径所在目录时（回退默认目录）：
   `无法识别当前路径所在目录，将使用默认目录：{异常消息}`（Warning）

**对话框细节**
- 对话框标题：`选择{Label}`。
- 初始目录：当前文本（规范化 + 绝对化）——若是目录且存在则用它本身，否则取其父目录；解析异常则回退默认。
- 文件类型过滤串（Windows/zenity 风格）：`允许的文件|*.ext1;*.ext2|所有文件|*.*`（扩展名去点、去重、忽略大小写）；无允许列表时为 null。
- **后端描述（`BackendDescription`，会出现在上述文案的括号里）**：Windows → `Windows 文件对话框（PowerShell）`；macOS → `macOS 原生选择器（osascript）`；Linux 有 zenity → `zenity`；否则有 kdialog → `kdialog`；都没有 → `不可用（未安装 zenity 或 kdialog）`。对话框最长等待 5 分钟。

### 3.4 `EncoderCard` —— 编码器信息卡

来源：`Components/Shared/EncoderCard.razor:1-264`

**Props**：`Encoder`（HardwareEncoder，必填）、`PlatformHint`（内部按平台推导）。

**卡片结构（视觉顺序）**
1. 头部 `.encoder-header`：厂商图标 + 显示名 + 可用性徽标。
   - 厂商图标映射：Nvenc `🟢`、Qsv `🔵`、Amf `🔴`、VideoToolbox `⚪`、Vaapi `🟣`、Vulkan `🟠`、MediaFoundation `🟡`、Software `💻`、未知 `❓`。
   - 可用性徽标：可用 → `ffui-badge-success`，内容 `✓可用`（`aria-label="状态：可用"`）；不可用 → `ffui-badge-danger`，内容 `✗不可用`（`aria-label="状态：不可用"`）。
2. 详情行：等宽 `Encoder.Name`（`title="模板 RequiredEncoder 需要填写这个名称"`）+ 右侧族显示名（`title={Family}`；族为空时显示 `未知族`）。
3. 标签行：`{Codec}`（`ffui-badge-info`，`title="编码格式"`）+ 硬件/软件徽标（`硬件编码` 或 `软件编码`，`title` 分别为 `硬件编码器`/`软件编码器`）。**Codec 徽标里已出现的值不会在下面的 codecs 行重复**。
4. 附加 codecs 标签行（`.codec-tag`，去重、去空、忽略大小写）。
5. 不可用时的说明（按优先级）：
   - `MissingFromBuild` → 警告条 `当前 FFmpeg 构建未包含该编码器`；若有 `UnavailableReason` 再显示一行灰色原因；若同时是硬件编码器，再加一条信息条（按平台）：
     - Windows：`Windows：请安装显卡驱动，并使用带 nvenc / qsv / amf 的 FFmpeg 构建。`
     - macOS：`macOS：仅 VideoToolbox 可用，请使用带 videotoolbox 的 FFmpeg 构建。`
     - Linux：`Linux：需要 /dev/dri 设备与 VA-API 驱动（如 intel-media-driver / mesa-va-drivers）。`
     - 其它：`请改用带该编码器的 FFmpeg 构建。`
   - 否则若有 `UnavailableReason` → 单条警告条显示该原因。
6. 可用但不支持质量参数时：`ffui-badge-warning` + `该编码器在当前 FFmpeg 版本下只能使用最简参数`。
7. `Encoder` 为 null（预渲染）时渲染降级卡片：`编码器信息不可用`。

**样式**：卡片 `background:var(--card-bg)`、1px 边框、`--radius-md`、`padding:1rem`、`margin:.5rem`、`min-width:250px`；hover 加 `--shadow-md`；不可用态 `opacity:.75` 且底变 `--surface-3`。

### 3.5 `TemplateParameterEditor` —— 模板参数控件自动生成器

来源：`Components/Shared/TemplateParameterEditor.razor:1-164`

**Props**：`Template`（可空）、`Values`（`Dictionary<string,string>`，忽略大小写）、`ValuesChanged`、`Disabled`。

**生成规则**（按 `Template.Parameters` 顺序，跳过 `Name` 为空的项）
- 容器 `.param-grid`：`grid-template-columns: repeat(auto-fit, minmax(210px, 1fr))`，`gap:.85rem`。
- 每个参数一个 `.param-item`（纵向 flex，`gap:.25rem`）。
- 标签：`{parameter.Label}`；当 `Kind === Number` 且 `Min`/`Max` 至少有一个存在时，追加范围提示 `（{Min??'—'} ~ {Max??'∞'}）`，`Min`/`Max` 用 `0.##` 格式。
- 控件类型映射：
  | `Kind` | 控件 | 细节 |
  | --- | --- | --- |
  | `Select`（下拉选择） | `<select class="param-input">` | 选项 `<option value={Value} title={Description}>{Label}</option>`；**选中项的 `Description` 非空时**在控件下方显示为帮助文本 |
  | `Number`（数字输入） | `<input type="number" step="any">` | 带 `min`/`max`；`oninput` 即时回写 |
  | `Boolean`（开关） | `<label class="param-switch"><input type="checkbox">` + 文本 | 文本为 `已启用` / `已关闭`；勾选写入 `TrueValue`，取消写入 `FalseValue`。真值判定兼容 `TrueValue`（忽略大小写）以及 `1`/`true`/`True`/`yes`/`on` |
  | `Text`（默认） | `<input type="text">` | `placeholder="{DefaultValue}"`；`oninput` 即时回写 |
- 非 Select 类型且 `Description` 非空时，在控件下方显示 `.param-help`。
- 控件 id 规则：`param-{Name}` 并把空格替换成下划线（`150-151`）。
- **取值规则**：`Values` 里有该键就用它，否则回退 `parameter.DefaultValue`。
- **回写规则**：**绝不修改传入的字典实例**，而是复制一份再改该键后回传（否则父组件可能忽略变更，`155-162`）。
- `Template` 为 null 或可见参数为 0 时不渲染任何内容。
- `Disabled` 时所有控件禁用（如转换正在启动、批量处理进行中）。

### 3.6 `TemplateSelector` —— 模板选择器（转换页/批量页共用）

来源：`Components/Shared/TemplateSelector.razor:1-932`

**Props**：`SelectedTemplate`、`SelectedTemplateChanged`、`InputPath`（用于「输入格式可能不兼容」提示）、`ShowUnavailable`（默认 false = 隐藏不可用并给计数提示）。

**区块（视觉顺序）**
1. 头部 `.selector-header`（`--surface-3` 底）：标题 `选择转换模板` + 右侧分组切换按钮组（`role="group" aria-label="模板列表显示方式"`）：`按分类分组` / `平铺列表`，用 `aria-pressed` 表达当前态。
2. 加载态：`.ffui-loading-row` + `正在加载模板与硬件编码器…`（`role="status"`）。
3. 加载错误条（`ffui-alert-danger`，`role="alert"`）：`{_loadError}`（格式 `加载模板失败：{异常消息}`）+ 按钮 `重试`。
4. 能力摘要条（`role="status"`，类由硬件可用性决定 info/warning）：
   - 未检测到报告：`尚未检测到硬件编码能力，模板可用性未知。`
   - 报告存在但无硬件编码器：`未检测到硬件编码，将使用 CPU 软件编码。`
   - 有硬件编码器：`本机可用硬件编码：{前两个显示名，、分隔}{，若大于 2 则 " 等 N 项"}。`（例：`本机可用硬件编码：NVIDIA NVENC、Intel Quick Sync 等 3 项。`）
5. 推荐编码器查询失败提示（`ffui-alert-warning`，`role="status"`）：`查询推荐编码器失败：{异常消息}`。
6. 搜索框：标签 `搜索`，占位 `按名称、描述或标签搜索模板…`；防抖 **250 ms**；搜索匹配 Name / Description / Category / Tags（忽略大小写；注意 Category 也被匹配）。
7. 分类标签页（`role="group" aria-label="模板分类筛选"`）：第一个固定为 `全部`（`aria-label="显示全部分类的模板"`），其后按 `GetCategoriesAsync()` 返回顺序渲染，每个 tab `aria-label="筛选分类：{分类}"`；激活态 `.tab.active` 用主色底 + 反色文字。
8. 不可用模板提示条（仅当不可用数 > 0）：文本二选一
   - 当前显示不可用：`{N} 个模板需要本机没有的硬件编码器，当前仅作展示、无法选择。`
   - 当前隐藏不可用：`{N} 个模板需要本机没有的硬件编码器，已隐藏。`
   - 右侧按钮：`隐藏不可用模板` / `显示不可用模板`（条底色随之 info/warning）。
9. 模板列表 `.templates-list`（`max-height:420px; overflow-y:auto`）。分组模式下每组前有 `.ffui-section-title.group-title`：分组名（空分类显示 `未分类`）+ 数量徽标。
10. 每一行是一个**真 `<button class="template-item">`**（键盘可聚焦、回车/空格可触发）：
    - `title` = 不可用原因，或（格式风险时）`输入格式可能不兼容，仍然可以选择`。
    - `aria-pressed` = `"true"|"false"` 表示选中；`disabled` = 不可用。
    - 内容：`{名称}` + 类型徽标（`预设`（无修饰色）/ `自定义`（success））+ 编码器徽标（见下）+ 描述 + 元信息行 + 可能的行内提示。
    - 元信息行（`📁` 与 `📥` 与 `⚙`）：
      - `📁 输出: .{OutputExtension}`
      - 若 `SupportedInputFormats` 非空：`📥 输入: {前 3 个，逗号分隔}{超过 3 个则加 "…"}`
      - 若参数数 > 0：`⚙ {N} 个可调参数`
    - 行内提示：不可用时显示红色条 = 不可用原因；否则若输入格式风险显示黄色条 `输入格式可能不兼容（当前文件为 {ext}）`。
    - 视觉：选中行 `background:var(--primary-light)` + 左侧 3px 主色竖条；不可用行 `opacity:.65; cursor:not-allowed`；格式风险行左侧 3px 警告色竖条；focus-visible 用 2px 主色内描边（`outline-offset:-2px`）。

**编码器徽标生成规则（`BuildRow`，`457-547`）**
| 模板模式 | 情况 | 徽标文案 | 徽标样式 |
| --- | --- | --- | --- |
| `Required` | 未填 `RequiredEncoder` | `⚠ 硬件不可用` | danger；行不可用，原因 `模板未指定编码器族` |
| `Required` | 检测报告缺失 | `⚠ 可用性未知` | warning；行不可用，原因 `{族显示名} 可用性未知：未能完成硬件编码器检测` |
| `Required` | 该族无成员可用 | `⚠ 硬件不可用` | danger；行不可用，原因 `需要 {族显示名} 硬件编码器，本机不可用` |
| `Required` | 有成员可用 | `🚀 {编码器显示名}` | success；行可用 |
| `Auto` | 推荐编码器为 null | `🚀 自动选择编码器` | info |
| `Auto` | 推荐的是硬件编码器 | `🚀 将使用 {编码器显示名}` | info |
| `Auto` | 推荐的是软件编码器 | `将使用 {编码器显示名}（软件编码）` | warning |
| `None` | 有推荐编码器 | `🔧 使用 {编码器显示名}` | 默认（无色） |
| `None` | 无推荐编码器 | 不显示徽标 | — |

**其他行为**
- `RequiredEncoder` 为族名（如 `nvenc`）时按族展开成员，任一成员可用即算满足。
- 排序：分组模式 = 分类顺序（来自 `GetCategoriesAsync` 的索引，未知分类排最后）→ `SortOrder` → 名称（忽略大小写）；平铺模式 = **保持服务返回顺序（推荐优先）**。
- 搜索防抖用「代号（generation）」而不是 CancellationToken，避免残留计时器。
- 设置变化（`SettingsChanged` 事件）后重新检测硬件：**只有 FFmpeg 路径变化才强制完整探测**，其它设置用缓存报告（`372-395`）。
- 不可用行点击不触发选择；选择失败 → Toast.Error `选择模板失败：{异常消息}`。
- **空状态三态（精确文案）**
  | 条件 | 文案与操作 |
  | --- | --- |
  | 模板库为空 | `没有模板` + 次级说明 `模板库为空，请先在「模板管理」中创建模板或重置系统预设。` |
  | 有匹配但全部不可用 | `{N} 个匹配的模板在本机不可用` + 按钮 `显示不可用模板` |
  | 搜索/筛选无结果 | `没有匹配的搜索结果` + 按钮 `清除筛选` |
- `SelectTemplateAsync` **只通过 `SelectedTemplateChanged` 通知父组件，绝不修改 `[Parameter]`**（React 版对应为只调用 `onChange`，不直接写 props）。

### 3.7 `TemplateCard` —— 模板卡片（网格 / 列表两用）

来源：`Components/Shared/TemplateCard.razor:1-450`

**Props**：`Template`、`ViewMode`（`grid` 默认 / `list`）、`IsSelected`、`OnSelect`、`OnEdit`、`OnDelete`、`OnView`、`OnCopy`、`IsAvailable`（`true|false|null`）、`UnavailableReason`、`SelectedEncoderName`、`ShowSystemBadges`（默认 true）。

**内容（视觉顺序）**
1. 标题行：`{Name}` + 徽标组：
   - 类型徽标（`ShowSystemBadges`）：`预设`（默认色）或 `自定义`（success）。
   - 硬件徽标（当 `HardwareMode != None || RequiresHardwareAcceleration || IsAvailable` 有值时）：
     - `IsAvailable === false` → `⚠ 硬件不可用`（danger），`title` = 不可用原因（回退文案 `本机缺少该模板所需的硬件编码器`）
     - `IsAvailable === true` → `🚀 {SelectedEncoderName}`，`SelectedEncoderName` 为空时 `🚀 硬件可用`（success），`title` = `将使用 {编码器} 硬件编码`（无编码器名时 `本机已检测到该模板可用的硬件编码器`）
     - `null` → `🚀 硬件加速（未检测）`（默认色），`title="尚未检测到本机硬件编码能力"`
2. 描述 `{Description}`。
3. 参数摘要（有参数时）：`⚙ 可调参数（{N}）` + 参数标签列表（前 3 个用 `、` 连接，超过 3 个追加 ` 等 {N} 项`）。
4. 元信息：`📁 .{OutputExtension}` 与 `📂 {Category}`。
5. **列表模式专有**：命令区块 → 标题行 `FFmpeg 命令参数:` + 复制图标按钮；下方 `<code>{CommandArgs}</code>`。
6. 操作区（`.card-actions`）：
   - 🔍 查看详情（`aria-label="查看模板详情"`，`title="查看详情"`）
   - 复制命令参数（仅网格模式在此；图标 `📋`，成功后 2 秒变 `✓`；`aria-label` 在成功后变为 `命令参数已复制`）
   - 若系统模板 → 📑 复制为自定义模板（`aria-label="复制为自定义模板"`）；否则 → ✏️ 编辑（`aria-label="编辑模板"`）+ 🗑️ 删除（`aria-label="删除模板"`，hover 变红）
   - 主按钮 `.ffui-btn.ffui-btn-primary.ffui-btn-sm.use-btn`：系统模板文案 `直接使用`，用户模板 `选择`；`IsAvailable === false` 时 **disabled**，`title` = 不可用原因。
7. 无障碍：卡片内有一个 `visually-hidden` 的 `role="status" aria-live="polite"` 区域播报复制状态（`命令参数已复制`）。

**交互文案**
- 点击被禁用的「选择/直接使用」→ Toast.Warning(不可用原因)。
- 回调缺失 → Toast.Warning `{动作}功能不可用：父组件未提供对应回调`（动作为 `使用模板`/`选择模板`/`编辑模板`/`删除模板`/`查看详情`/`复制模板`）。
- 回调抛异常 → Toast.Error `{动作}失败：{异常消息}`。
- 复制命令参数：空命令 → Warning `该模板没有可复制的命令参数`；JS 抛异常 → Error `复制失败：{异常消息}`；返回 false → Error `复制失败：浏览器拒绝了剪贴板访问，请手动选择命令文本`；成功 → Success `命令参数已复制到剪贴板`。
- 卡片视觉：`--card-bg` 底、1px 边框、`--radius-md`、`padding:var(--spacing-4)`；hover 加 `--shadow-md`；选中态边框变主色 + `box-shadow:var(--focus-ring)`；不可用禁用态（class `blocked`）边框变警告色；网格模式操作区有上边框分隔。

### 3.8 `TemplateDetailViewer` —— 模板详情弹窗

来源：`Components/Shared/TemplateDetailViewer.razor:1-1010`

**Props**：`Template`、`IsOpen`、`IsOpenChanged`、`OnCopy`、`OnEdit`、`OnUse`、`ParameterValues`、`ParameterValuesChanged`。另有兼容旧写法的 `Open(template)` / `Close()` 公开方法。

- 外壳：`<Modal Size="lg">`，标题为 `📋 模板详情`（无模板时）或 **`📋 模板详情 · {Name}`**。
- 内容为空时：`没有可显示的模板。`

**区块顺序与内容**

1. 顶部提示（条件显示）：
   - 硬件信息读取失败（`ffui-alert-warning`，`role="status"`）：`读取硬件编码器信息失败：{异常消息}`
   - 模板不可用（`ffui-alert-warning`，`role="alert"`）：`该模板必须使用 {族显示名} 硬件编码，本机不可用：{缺失原因}`
2. **基本信息**
   - `模板名称` → `{Name}`（加粗 1.05rem）
   - `类型` → `系统预设`（默认色徽标）/ `用户自定义`（success 徽标）
   - `分类` → `📂 {Category}`
   - `描述` → `{Description}`，为空时显示 `无描述`
3. **命令与预览**
   - `FFmpeg 命令参数` 行 + 按钮（`aria-label="复制命令参数"`，文案 `📋 复制`，成功后 2 秒变 `✓ 已复制`）；下方代码框显示 `{CommandArgs}`。
   - `命令预览` 行 + 提示 `用占位符拼出的实际命令行，参数改动会立即反映在这里`。
     - 生成失败 → 红色条 `{_previewError}`（格式 `命令预览生成失败：{异常消息}`）
     - 生成中 → loading 行 `正在生成命令预览…`
     - 成功 → 主色描边的代码框显示 `ffmpeg {CommandLine}`；若有警告，下方以 `⚠ {警告}` 列表逐条显示。
   - 预览使用的占位路径是 `<输入文件>` 与 `<输出文件>`（`626-631`）。
4. **参数**
   - 无参数：`该模板没有可调参数`
   - 父组件提供了 `ParameterValuesChanged`：渲染 `<TemplateParameterEditor>`（可编辑，改动立即刷新预览并回传）
   - 未提供回调：只读列表，每项显示 `{Label}`、控件类型中文（见下表）、`默认：{DefaultValue}`（若有）、`{Description}`（若有）；末尾提示 `只读展示：父页面未提供 ParameterValuesChanged，暂时无法在此调整参数。`
     | `Kind` | 中文 |
     | --- | --- |
     | `Select` | `下拉选择` |
     | `Number` | `数字` |
     | `Boolean` | `开关` |
     | 其它 | `文本` |
5. **格式**
   - `输出格式` → `.{OutputExtension}`
   - `支持的输入格式` + 注记 `参考信息` → 有值时逐个加 `.` 前缀用 `, ` 连接；无值时 `不限制`；固定提示 `该列表只是参考资料，并不强制校验；能否处理取决于 FFmpeg 与源文件编码。`
6. **硬件加速**
   - 报告未就绪 → loading 行 `正在读取本机硬件编码能力…`
   - `策略` → 三选一：
     - `必须使用硬件编码器（本机不可用时该模板不可用）`
     - `自动：优先使用硬件编码器，没有可用硬件时回退到软件编码`
     - `不使用硬件加速（使用模板中指定的编码器）`
   - `编码器族` 行（当指定了族或策略为 Required 时）：未指定族 → `✗ 不可用` 徽标 + `模板未指定编码器族`；否则 `✓ 可用`（success）/ `✗ 不可用`（danger）徽标 + 族显示名 + 说明（可用：`将使用 {编码器显示名}`；不可用：{缺失原因}）。
   - `将使用的编码器` 行（当策略非 None 或命令含 `{encoder}` 时）：编码器徽标（硬件为 success）+ 说明：
     | 情况 | 说明文案 |
     | --- | --- |
     | 未解析出 | `（模板未使用 {encoder}，由模板内固定编码器决定）` |
     | Required 且是硬件 | `（模板要求的编码器族可用）` |
     | Required 但非硬件 | `（本机缺少该编码器族，实际执行可能失败）` |
     | Auto 且是硬件 | `（自动选择：本机可用的硬件编码器）` |
     | Auto 但非硬件 | `（未检测到可用硬件编码器，回退到软件编码）` |
     | 其它 | `（按模板中的 {encoder} 占位符解析）` |
   - **缺失原因推导顺序**：先取已探测到的族成员里的第一条 `UnavailableReason`；否则若有 `MissingFromBuild` → `当前 FFmpeg 构建不包含 {族显示名} 编码器`；否则 → `未检测到可用的 {族显示名} 编码器`。未指定族时 → `模板未指定编码器族`。
7. **其它**
   - `创建时间`：系统模板显示 `系统预设`（斜体弱化）；用户模板显示 `yyyy-MM-dd HH:mm:ss`（本地时间，仅当 `> DateTime.MinValue`）
   - `更新时间`：同上格式（有值时）
   - `排序权重` → `{SortOrder}`
8. **底部按钮栏**（从左到右）
   - `↻ 重新读取`（`.ffui-btn-ghost`，`margin-right:auto` 靠左；刷新中显示 spinner；disabled 条件 `_isRefreshing || Current == null`）
   - `关闭`
   - `📋 复制命令`（`aria-label="复制命令参数"`）
   - 系统模板 → `📑 复制为自定义模板`；否则 → `✏️ 编辑模板`
   - `🚀 使用此模板`（primary；disabled = 无模板或不可用；`title` = 不可用原因）

**动作与文案**
- 重新读取：`GetTemplateByIdAsync` 成功 → 替换当前实例 + 重建预览 + Success `已重新读取模板`；模板已不存在 → Warning `该模板已不存在，可能已被删除`；异常 → Error `重新读取模板失败：{异常消息}`。
- 复制命令：空命令 Warning `该模板没有可复制的命令参数`；异常 Error `复制失败：{异常消息}`；失败 Error `复制失败：浏览器拒绝了剪贴板访问，请手动选择命令文本`；成功 Success `命令参数已复制到剪贴板`。
- 「复制为自定义模板 / 编辑模板 / 使用此模板」都**先关闭弹窗再调用回调**，避免回调抛错把用户困在弹窗里（`761-784`）；回调缺失 → Warning `{动作}功能不可用：父页面未提供对应回调`；失败 → Error `{动作}失败：{异常消息}`（动作分别为 `复制为自定义模板` / `编辑模板` / `使用此模板`）。
- 关闭异常 → Error `关闭模板详情失败：{异常消息}`。
- 参数初始化异常 → Error `初始化模板参数失败：{异常消息}`；参数变更回传异常 → Error `参数变更通知失败：{异常消息}`。

### 3.9 `TemplateEditor` —— 模板编辑器（新建 / 编辑，xl 大弹窗）

来源：`Components/Shared/TemplateEditor.razor:1-2127`（模板部分 `25-666`，样式 `668-791`，逻辑 `793-2127`）

**Props**：`Template`（null = 新建）、`IsOpen`、`IsOpenChanged`、`IsSystem`（true 走系统预设 API）、`OnSaved`。

**弹窗标题**（`985-987`）
| 场景 | 标题 |
| --- | --- |
| 新建用户模板 | `新建模板` |
| 新建系统预设 | `新建系统预设` |
| 编辑用户模板 | `编辑模板` |
| 编辑系统预设 | `编辑系统预设` |

**顶部徽标**：`系统预设`（primary）或 `用户模板`（info），加一个 `新建` / `编辑` 徽标。

**顶部提示**
- 辅助信息加载中：loading 行 `正在读取分类与硬件编码器状态…`
- 辅助信息加载失败：警告条 `⚠️ 辅助信息加载失败：{异常消息}（不影响手动填写与保存）`
- 编辑系统预设时的信息条（`role="note"`）：`系统预设可以编辑，但应用升级时同名预设会被内置内容覆盖（按预设版本同步）。需要长期保留修改，请改完后复制成普通模板。`

**区块 1：基本信息**（`.te-section`，标题 `基本信息`）
| 字段 | 控件 | 默认 / 占位 | 校验与提示 |
| --- | --- | --- | --- |
| `模板名称`（必填，红色 `*`） | text | 占位 `例如：压缩 · 均衡（CRF 23）` | `oninput` 即时更新校验与预览；`onblur` 规范化（去首尾 `{}` 与空白） |
| `分类` | text + `<datalist>`（已有分类） | 占位 `选择已有分类，或直接输入新分类` | 提示 `留空会保存为「未分类」。` |
| — | 同名提示条（条件） | — | `已经有同名模板「{名称}」（{系统预设|用户模板}）。保存时服务会自动改名（追加序号）；如果这是有意覆盖同名模板，请忽略这条提示。` |
| `描述` | textarea rows=2 | 占位 `一句话说明这个模板做什么、什么场景用` | — |
| `输出扩展名`（必填） | text | 占位 `mp4` | 提示 `不需要写点号，会自动转成小写（mp4 / mkv / mp3 …）。` |
| `排序权重` | number | — | 提示 `数字越小越靠前。` |
| `支持的输入格式` | text | 占位 `mp4, mkv, avi（逗号或空格分隔）` | 提示 `留空表示不限格式；会自动去点号、转小写、去重。`；非空时把每个格式渲染成 info 徽标 chip，chip 内 `✕` 按钮 `aria-label="移除输入格式 {fmt}"` |
| `标签` | text | 占位 `压缩, 归档, 手机（逗号或空格分隔）` | 非空时 chip 列表，`✕` 按钮 `aria-label="移除标签 {tag}"` |
| `在模板列表中标记为推荐` | checkbox | — | — |

- 分隔符集合（格式与标签通用，`1272`）：`, ，、 空格 \t \r \n ; ；`。
- 格式额外处理：去前导 `.`、转小写、忽略大小写去重；标签只去重（当前文化忽略大小写）。

**区块 2：FFmpeg 命令参数**（标题 `FFmpeg 命令参数`）
- `命令参数`（必填）textarea rows=5，占位：
  `-i "{input}" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} -c:a aac "{output}"`
- 固定帮助文案：`不含 ffmpeg 本身；不要写 -y / -n（执行层会按「文件已存在时的处理方式」自动添加）。含空格的路径写成 "{input}" 这种带引号的形式。`
- **内置占位符插入行**：`内置占位符` 下拉（按分组 `<optgroup>`，选项文案 `{占位符} — {说明}`）+ 按钮 `插入占位符`。
  - 明确提示：`浏览器端拿不到可靠的光标位置（app.js 里没有「插入到光标」的函数），所以这里只有一种行为：把选中的占位符追加到命令末尾。`
  - 插入后把焦点交回命令 textarea。
- `<details>`：`全部内置占位符说明（共 {N} 个）`，内部表格两列 `占位符` / `含义`，按分组插入分组行。
  **完整占位符目录（分组 → 占位符 → 含义，逐字）**
  | 分组 | 占位符 | 含义 |
  | --- | --- | --- |
  | 路径类 | `{input}` | 输入文件的完整路径 |
  | 路径类 | `{output}` | 输出文件的完整路径（由输入文件名 + 输出扩展名生成） |
  | 路径类 | `{input_dir}` | 输入文件所在目录 |
  | 路径类 | `{input_name}` | 不带扩展名的输入文件名 |
  | 路径类 | `{input_filename}` | 含扩展名的输入文件名 |
  | 路径类 | `{input_ext}` | 输入扩展名（如 mp4，不含点） |
  | 路径类 | `{output_dir}` | 输出文件所在目录 |
  | 路径类 | `{output_name}` | 不带扩展名的输出文件名 |
  | 路径类 | `{output_ext}` | 输出扩展名（如 mp4，不含点） |
  | 媒体信息类 | `{duration}` | 输入文件时长（秒） |
  | 媒体信息类 | `{width}` | 输入视频宽度（像素） |
  | 媒体信息类 | `{height}` | 输入视频高度（像素） |
  | 媒体信息类 | `{fps}` | 输入视频帧率 |
  | 媒体信息类 | `{vcodec}` | 输入视频编码格式名 |
  | 媒体信息类 | `{acodec}` | 输入音频编码格式名 |
  | 媒体信息类 | `{sample_rate}` | 音频采样率（没有音频信息时为 44100） |
  | 媒体信息类 | `{channels}` | 音频声道数（没有音频信息时为 2） |
  | 编码器类 | `{encoder}` | 按目标格式与本机硬件自动选出的视频编码器名（如 h264_nvenc / libx264） |
  | 编码器类 | `{audio_encoder}` | 音频编码器名（默认 aac） |
  | 编码器类 | `{crf}` | 质量数值（等价于 quality，可被同名参数覆盖） |
  | 编码器类 | `{quality}` | 质量数值，通常由一个名为 quality 的参数驱动 |
  | 编码器类 | `{q}` | 质量数值的简写 |
  | 编码器类 | `{quality_args}` | 当前编码器的完整质量参数（-crf 23 / -cq 23 / -qp 23 …），需要配合 encoder 使用 |
  | 编码器类 | `{preset_args}` | 当前编码器的预设参数（如 -preset medium），与 encoder 配套 |
  | 编码器类 | `{hwupload_args}` | 显存上传滤镜（如 format=nv12,hwupload），VA-API 等需要；模板已写 -vf 时会自动并入 |
  | 编码器类 | `{device_args}` | 编码器所需的设备初始化参数（如 -vaapi_device /dev/dri/renderD128），由系统自动插入 |
  | 编码器类 | `{muxer_args}` | 容器兼容参数（如 HEVC 入 MP4 需要的 -tag:v hvc1），由系统自动插入 |
  | 编码器类 | `{threads}` | 线程数（未设置时为空） |
  | 编码器类 | `{hwaccel}` | 硬件解码参数（如 -hwaccel videotoolbox，未启用时为空） |
  | 时间类 | `{date}` | 当前日期 yyyyMMdd |
  | 时间类 | `{time}` | 当前时间 HHmmss |
  | 时间类 | `{datetime}` | 当前日期时间 yyyyMMdd_HHmmss |
  | 时间类 | `{random}` | 随机 8 位十六进制字符串（避免输出文件重名） |
  | 其他 | （系统新增而尚未写说明的占位符） | `系统提供的占位符` |

**区块 3：命令预览**（标题 `命令预览`）
- 说明：`按各参数的默认值展开；{input} 与 {output} 用占位路径代替。`
- 右侧按钮 `📋 复制`，`disabled="@(string.IsNullOrWhiteSpace(_previewCommand))"`。
- 三种状态：
  - 生成失败 → 红条 `❌ 命令预览生成失败：{异常消息}`
  - 命令为空 → 黄条 `⚠️ 命令参数为空，暂时无法生成预览。`
  - 成功 → `<pre class="te-code">ffmpeg {预览命令}</pre>`（最多 260px 高、可滚动）
- 预览警告（`.Warnings`）逐条用黄条展示。
- **刷新时机**：任何会改变模板定义或参数默认值的编辑后立即重算（`RefreshDerived()`），包括：名称、命令参数、输出扩展名、参数增删改、控件类型、默认值、选项值、硬件策略、所需编码器、目标编码格式。

**区块 4：试运行**（标题 `试运行`）
- 按钮：`🧪 试运行模板`；运行中显示 spinner + `试运行中…`（禁用）。
- 说明：`会用 FFmpeg 合成 3 秒测试素材真实执行一次，可验证模板在本机能否跑通（通常几秒钟）。`
- 结果块（success/danger 提示条，`role="status" aria-live="polite"`）：
  - `{test.Message}`
  - `耗时 {时长}`（格式：`{0.0} 秒`，≤0 时显示 `—`）
  - 若命令非空 → `<pre>` 显示命令
  - 若有输出 → `FFmpeg 输出（诊断信息）` 折叠块
- Toast：成功 `模板试运行成功，该模板在本机可以跑通`；失败 `模板试运行失败，请看下方详情`；异常 → `试运行出错：{异常消息}`。

**区块 5：校验结果（实时）**（标题 `校验结果（实时）`）
- 无问题：绿条 `✅ 没有发现问题。`
- 有问题：按严重度降序逐条渲染（颜色/图标/标签）：
  | 严重度 | 类 | 图标 | 标签 |
  | --- | --- | --- | --- |
  | Error | `ffui-alert-danger` | `❌` | `错误` |
  | Warning | `ffui-alert-warning` | `⚠️` | `警告` |
  | Info | `ffui-alert-info` | `ℹ️` | `提示` |
  - 每条格式：`{标签}{消息}`，若 `Field` 有映射则追加 `（字段：{中文标签}）`。
  - 字段中文映射：`Name` → `模板名称`、`CommandArgs` → `命令参数`、`OutputExtension` → `输出扩展名`、`Category` → `分类`、`Parameters` → `参数`、`RequiredEncoder` → `所需编码器`；其它原样显示。
- 固定说明：`这里是 ITemplateService.Validate 的完整结果（错误 / 警告 / 提示）；存在错误时无法保存。`

**区块 6：参数（`{N}`）**
- 「快速添加」行：说明 `添加常用参数：` + 预设按钮（文案逐字）：
  `＋ 质量 quality`、`＋ 音频码率 audio_bitrate`、`＋ 编码预设 preset`、`＋ 宽度 width`、`＋ 水印路径 watermark`。
  重复添加 → Warning `已经存在参数「{名称}」，无需重复添加。`
- **未声明占位符提示**（条件）：`命令里用到、但没有声明的占位符（保存后用户无法为它们填值）：`，每个 chip 显示 `{占位符}` + `＋` 按钮（`aria-label="为 {名称} 创建参数"`）。仅列出「非内置占位符」且未声明的项。
- 空参数提示：`还没有声明任何参数。无参数模板也能用，只是用户在转换时无法调整这一项。`
- 每个参数字卡（`.te-param`）：
  - 头部：`#{序号}`（primary 徽标）+ `命令中已使用`（success）/ `命令中未使用`（warning）+ 右侧 `↑`（`aria-label="上移参数 {名}"`，第一个禁用）、`↓`（`aria-label="下移参数 {名}"`，最后一个禁用）、`✕`（`aria-label="删除参数 {名}"`）。
    - 判定「已使用」：占位符集合包含该名；或该名是 `quality`/`q`/`crf` 且命令里出现 `{quality_args}`/`{crf}`/`{quality}`/`{q}`。
  - 字段网格：
    | 标签 | 控件 | 占位 | 说明 |
    | --- | --- | --- | --- |
    | `参数名（占位符）` | text（`onchange`） | `quality` | 提示 `命令里用 {名} 引用；会自动转小写、去花括号、非法字符换成下划线。`；`onblur` 规范化，若显示名为空则同步为参数名 |
    | `界面显示名称` | text | `质量` | — |
    | `控件类型` | select | — | 选项：`下拉选择` / `数字输入` / `文本输入` / `开关（是 / 否）` |
    | `默认值` | text | `留空表示不设置` | — |
    | `帮助说明` | text | `显示在参数控件下方的一句话说明` | — |
  - 参数名非法时的警告条：`参数名无效：只允许小写字母、数字和下划线，且不能以数字开头，否则 {名} 不会被识别成占位符。`
  - 仅 `Number` 时追加两列：`最小值`（number step=any）、`最大值`（number step=any）。
  - 仅 `Boolean` 时追加两列：`开启时替换成`（占位 `1`）、`关闭时替换成`（占位 `留空表示移除该片段`）。
  - 仅 `Select` 时展开选项编辑器：头部 `下拉选项（{N}）` + 按钮 `＋ 添加选项`；选项为空 → 警告 `下拉参数至少需要一个选项，否则转换时用户没有可选的值。`；每个选项一行三列（`选项值` 占位 `值`、`显示文本` 占位 `显示文本`、`选项说明` 占位 `说明（可选）`，前两列标签视觉隐藏）+ 上移/下移/删除图标按钮。
- 底部按钮：`＋ 添加空参数`（新增 `{ name: "param{N+1}", label: "", kind: Text, defaultValue: "" }`）。
- 参数名规范化算法：`Trim()` → 去 `{}` → 转小写 → 只保留 `a-z0-9_`，把 `-`、`.`、空格替换成 `_` → 去掉首尾 `_`。

**区块 7：硬件加速**（标题 `硬件加速`）
| 字段 | 控件 | 选项 / 说明 |
| --- | --- | --- |
| `硬件加速策略` | select | `不使用（纯软件编码）` / `自动（有硬件就用，没有回退软件）` / `必须使用硬件（没有就失败）`；提示 `「是否需要硬件加速」由这一项推导，两者不会再出现不一致。` |
| `所需编码器` | select | 未指定 → `（不指定）`；按族分组（`按编码器族`）：`{族显示名} · 按族自动挑选`（本机不可用时追加 `（本机不可用）`）；按具体编码器分组（`按具体编码器`）：`{名字} · {显示名}{后缀}`；当前值不在目录时补一项 `{值}（不在编码器目录中）`。**策略为「不使用」时禁用并会清空该项**。提示：`「不使用」时会清空这一项；标记「（本机不可用）」的选项仍可选——模板也可能是为别的机器准备的。` 追加「本机可用性来自 {yyyy-MM-dd HH:mm} 的硬件检测。」或加载中时 `正在检测本机编码器，完成后会刷新标记…` |
| `目标编码格式` | select | 默认 `自动推断`（空值）；其余选项标签：`H.264 / AVC`、`H.265 / HEVC`、`AV1`、`VP9`、`VP8`、`ProRes`、`AAC（音频）`、`MP3（音频）`、`Opus（音频）`、`FLAC（音频）`，未知编码器显示原名。提示：`决定 {encoder} 会挑哪一类编码器，也决定没有硬件时回退到哪个软件编码器。` |

- 不可用原因后缀（编码器选项里）：平台不支持 → `仅 {Windows / Linux|macOS|Linux|Windows|其它平台} 可用`；平台支持但本机没检测到 → `本机未检测到可用硬件`.
- 固定信息条（`role="note"`，逐字）：
  > 硬件加速模板推荐用 {encoder} + {quality_args} + {preset_args} + {hwupload_args} 组合：
  > - `{encoder}`：按目标格式和本机硬件自动挑编码器（h264_nvenc / h264_qsv / h264_videotoolbox / h264_vaapi / libx264 …）。
  > - `{quality_args}`：展开成当前编码器真正接受的质量参数（-crf / -cq / -qp / -q:v / -global_quality …），由名为 quality 的参数驱动。
  > - `{preset_args}`：展开成该编码器的预设参数（如 -preset medium / -quality balanced）。
  > - `{hwupload_args}`：需要把帧上传到显存时补上滤镜（VA-API 的 format=nv12,hwupload）。
  >
  > 不要硬写 -crf：硬件编码器不接受它，FFmpeg 只会打印一条警告然后忽略，命令照样成功退出，画质却完全没变——这种「看起来生效了」的模板最难排查。上面的校验会用警告提示这种情况。

**底部按钮栏（sticky）**
- 左侧状态徽标：有错误 → `{N} 个错误，无法保存`（danger）；否则有警告 → `{N} 个警告`（warning）；试运行中 → `试运行中…`（info）。
- 按钮：`取消`（disabled = 保存中或试运行中）、`保存`（primary，disabled = 保存中或错误数 > 0；保存中显示 spinner + `保存中…`）。
- **关闭确认**：有未保存修改时弹 ConfirmDialog：
  - 标题 `放弃未保存的修改？`
  - 正文 `模板「{名称或"未命名"}」有未保存的修改。`
  - Detail `关闭后会丢失本次编辑的全部内容，此操作无法撤销。`
  - 按钮 `放弃修改`（危险）/ `继续编辑`
  - Esc / 遮罩 / ✕ 也会走同一拦截（`OnModalIsOpenChanged` 检测到脏数据即回推 `true` 并打开确认框）。

**保存行为与文案**
- 保存前规范化：名称 `Trim().Trim('{','}')`、描述/命令/分类 `Trim()`、扩展名去点转小写、`TargetCodec` `Trim()`、`RequiredEncoder` 空则置 null、`RequiresHardwareAcceleration = (HardwareMode === Required)`、格式/标签重新拆分、参数名规范化、选项值 `Trim()` 且显示名为空时用值填充。
- 有错误时：Warning `还有 {N} 个错误未修正，无法保存。`
- 新建/更新的判定以「数据库里是否真的存在该 Id」为准（避免把新建当编辑）。
- 系统预设：新建走 `CreateSystemTemplateAsync`；更新失败 → Error `保存失败：系统预设可能已被删除，请刷新页面后重试。`
- 用户模板：新建走 `CreateTemplateAsync`；更新失败 → Error `保存失败：模板可能已被删除，或它是系统预设（系统预设请在系统模板管理中编辑）。`
- 成功 → Success `模板「{最终名称}」已保存`，随后关闭弹窗并回调 `OnSaved`。
- 异常 → Error `保存失败：{异常消息}`。

### 3.10 `BatchFileList` —— 批量待处理文件列表

来源：`Components/Shared/BatchFileList.razor:1-467`

**Props**：`Files`（`List<string>`）、`FilesChanged`、`FileStatuses`（`Dictionary<路径, TaskStatus 名>`）、`IsProcessing`（处理期间禁止改动列表）、`ScanLimit`（默认 5000）。

**头部**
- 标题：`📄 待处理文件` + 数量徽标 `{N}` + （总大小 > 0 时）`共 {格式化大小}`。
- 仅在已有文件时显示按钮 `清空`（`.ffui-btn-sm`，`disabled=IsProcessing`）。

**添加区 `.bf-add`**
- 文本框（`id="batch-path-input"`，视觉隐藏标签 `要添加的文件或文件夹路径`）：
  - 占位：`粘贴文件或文件夹的完整路径，按 Enter 添加（可一次粘贴多行）`
  - `disabled=IsProcessing`；**支持一次粘贴多行**（按 `\n`、`\r` 拆分并 Trim）
  - 按 `Enter` 或 `NumpadEnter` 且内容非空即添加
- `添加`（primary，`disabled = IsProcessing || 文本为空`）
- `选择文件`（`disabled = IsProcessing || _isScanning`）
- `选择文件夹`（同上）

**选项行 `.bf-options`**
- 复选框 `递归扫描子文件夹`（**默认勾选**；`disabled = IsProcessing || _isScanning`）
- 提示：`浏览按钮使用 {对话框后端描述}`

**扫描中提示**：`.ffui-loading-row` + spinner + `正在扫描目录…`

**筛选（仅当文件数 > 6 时出现）**：`<input type="search">` 占位 `按文件名筛选…`（视觉隐藏标签 `按文件名筛选`）；输入非空时右侧显示 `匹配 {过滤后} / {总数}`。筛选用 `Path.GetFileName` 包含、忽略大小写。

**空状态**
- 完全没文件：`还没有添加文件。<br />用上面的输入框粘贴路径，或点击「选择文件 / 选择文件夹」批量添加。`
- 有文件但筛选无结果：`没有匹配「{筛选词}」的文件。` + 按钮 `清除筛选`

**列表行（`.bf-item`，最多 380px 高、可滚动）**
- 图标（按状态优先，否则按扩展名）：
  | 情况 | 图标 |
  | --- | --- |
  | Running | `⏳` |
  | Completed | `✅` |
  | Failed | `❌` |
  | Cancelled | `⏹️` |
  | Skipped | `⏭️` |
  | Pending | `⏸️` |
  | 无状态 + 视频扩展名（mp4/m4v/mov/mkv/avi/webm/wmv/flv） | `🎬` |
  | 无状态 + 音频扩展名（mp3/wav/flac/aac/ogg/m4a/opus/wma） | `🎵` |
  | 无状态 + 图片扩展名（gif/png/jpg/jpeg/bmp/webp） | `🖼️` |
  | 其它 | `📄` |
- 文件名（粗体、超出省略、`title` 为完整路径）+ 目录（弱化、省略、`title` 为完整路径）
- 大小（等宽、弱化，`title="{字节数} 字节"`）
- 有状态时追加徽标：`已完成`（success）/`失败`（danger）/`进行中`（info）/`已取消`（warning）/`已跳过`（warning）/ 其它 `等待中`（默认色）
- 移除按钮 `✕`（`aria-label="移除 {文件名}"`，`disabled=IsProcessing`）
- **左侧 3px 状态竖条颜色**：running → `--info-color`、completed → `--success-color`、failed → `--danger-color`、cancelled/skipped → `--warning-color`、pending 或无 → `--border-strong`
- **性能要求**：文件大小必须缓存（`_sizeCache`），旧实现在每次渲染对每行做同步 IO，网络盘上会卡死界面（`198-220`）。

**添加路径的处理与文案**（`AddTypedPathAsync`，`231-276`）
- 每一行先展开 `~`、规整为绝对路径；文件 → 直接加入；目录 → 进入扫描；都不存在 → 记为缺失。
- 缺失提示（只列前 3 个，用 `、` 连接）：`以下路径不存在，已忽略：{前3个}`，若多于 3 个追加 ` 等共 {N} 个`（Warning）。
- 异常 → Error `添加失败：{异常消息}`。

**浏览文件/文件夹文案**
| 场景 | 文案 |
| --- | --- |
| 文件对话框标题 | `选择媒体文件` |
| 文件对话框取消/不可用 | `没有选择文件。如果对话框没有弹出，可以直接把路径粘贴到上方输入框。（当前后端：{后端描述}）`（Info 级 Toast） |
| 文件选择器异常 | `打开文件选择器失败：{异常消息}`（Error） |
| 文件夹对话框标题 | `选择包含媒体文件的文件夹` |
| 文件夹取消/不可用 | `没有选择文件夹。如果对话框没有弹出，可以直接把文件夹路径粘贴到上方输入框。（当前后端：{后端描述}）`（Info） |
| 文件夹选择器异常 | `打开文件夹选择器失败：{异常消息}`（Error） |

**扫描目录**（后台线程执行，`323-363`）
- 无找到任何媒体文件 → Warning `所选目录中没有找到可识别的媒体文件。`
- 超过上限：只取前 `ScanLimit` 个，并 Warning `该目录文件过多，只添加了前 {上限} 个。建议缩小范围后分批处理。`
- 异常 → Error `扫描目录失败：{异常消息}`

**添加结果 Toast**
- 全部是重复 → Info `这些文件已经在列表中了。`
- 没有任何可添加 → Info `没有可添加的文件。`
- 成功 → Success `已添加 {N} 个文件`，若有重复追加 `，跳过 {M} 个重复项`

**清空确认（ConfirmDialog）**
- 标题 `清空文件列表`（Danger）
- 正文 `确定要清空待处理文件列表吗？`
- Detail `将移除列表中的 {N} 个文件。不会删除磁盘上的任何文件。`
- 确认按钮 `清空列表`
- 成功 Toast：`已清空文件列表（磁盘上的文件未受影响）`

### 3.11 `BatchProgressPanel` —— 批量处理进度面板

来源：`Components/Shared/BatchProgressPanel.razor:1-310`

**Props**：`Tasks`（本批次全部任务）、`IsProcessing`、`OnRetry`、`OnCancel`、`OnOpenOutputFolder`。

**空状态**：`还没有开始批量处理。添加文件、选好模板后点击「开始批量处理」。`

**汇总区 `.summary`（`aria-live="polite"`）**
1. 主行：`{已完成数} / {总数}`（1.15rem 粗体）+ 弱化文字 `已处理` + 条件徽标：
   - 运行中 > 0 → `{N} 个进行中`（info 徽标）
   - 等待中 > 0 → `{N} 个等待中`（默认徽标）
   - 非处理中且全部结束 → `全部成功`（success，当失败+取消为 0）或 `已完成，含失败项`（warning）
2. 总进度条：`.ffui-progress-track` + `.ffui-progress-fill`，`role="progressbar"`，`aria-label="批量处理总进度"`，宽度 = `已结束任务数 / 总数 * 100`（`已结束` = 完成+失败+取消+跳过），保留 **0.xx** 小数（`0.##`），`aria-valuenow` 用整数（`0`）。
3. 计数行（`.counters`，颜色区分）：
   - `✅ 已完成 {N}`（恒显示，success 色）
   - 失败 > 0 → `❌ 失败 {N}`（danger 色）
   - 取消 > 0 → `⏹️ 已取消 {N}`（warning 色）
   - 跳过 > 0 → `⏭️ 已跳过 {N}`（warning 色）
   - 有已用时间 → `已用时 {mm:ss 或 H:MM:SS}`（弱化）
   - 有预计剩余 → `预计还需 {mm:ss 或 H:MM:SS}`（弱化）
     - 剩余时间算法：取所有「运行中且 ETA > 0」任务 ETA 的**最大值** + `等待中任务数 × 这些 ETA 的平均值`；没有可用 ETA 时不显示。
     - 已用时间 = `max(CompletedAt ?? UtcNow) - min(CreatedAt)`。

**任务行区（`.task-rows`，`role="list"`，最高 460px 可滚动）**
- 默认只显示「优先级任务」= 运行中/等待中/失败/已取消 的全部，再加最多 `8 - 优先级数量` 条按创建时间倒序的补充项，最后按创建时间正序排列。
- 每行 `role="listitem"`：
  - 状态图标（`✅`/`❌`/`⏳`/`⏹️`/`⏭️`/`⏸️`）
  - 文件名（`title` 为完整路径）
  - 副行：状态徽标；运行中追加 `{进度 0.#}%` 与（速度 > 0 时）`{0.##}x`；非运行中且输出大小 > 0 时显示输出文件大小；有错误信息时显示红色截断文本（**最多 90 字符，超出加 `…`**，`title` 为完整错误）
  - 操作按钮（互斥）：
    - Running → `终止`（danger）
    - Pending → `取消`
    - 其它 → `重试`
    - 失败或已取消且错误信息非空 → 追加 `复制错误`
    - Completed → 追加 `打开目录`
  - 左侧 3px 状态竖条：running info / completed success / failed danger / cancelled+skipped warning
- 展开/收起：「默认有隐藏项 或 已展开」时显示按钮：
  - 收起态文案：`还有 {N} 个任务，展开全部`
  - 已展开文案：`只看进行中与失败的任务`

**复制错误**：文本空 → 复制 `(没有错误信息)`；成功 → Success `错误信息已复制`；失败 → Error `复制失败：浏览器不允许访问剪贴板，请手动选中文本复制。`

---

## 4. 页面逐个规格

> 以下每页都给出：区块顺序 → 每个控件的标签/类型/默认值/校验/触发的服务调用 → 按钮与禁用条件 → 展示数据与格式 → 空/加载/错误态文案 → 实时行为 → 跨页跳转。

### 4.1 首页 `/`（Home）

来源：`Components/Pages/Home.razor:1-843`
截图：![首页](../../../FFmpegWebUI/readmefiles/index.png) / 深色：![深色首页](../../../FFmpegWebUI/readmefiles/dark-index.png)

**区块 0 · Hero**
- 标题 `🎬 FFmpeg WebUI`（h1）+ 副标题 `本地媒体文件转换工具 · v2.0.0`
- 快速开始导航（`aria-label="快速开始"`）：
  - `🔄 开始转换`（primary、lg）→ `/convert`
  - `📁 批量处理`（lg）→ `/batch`
  - `📋 模板`（lg）→ `/templates`

**区块 1 · 📊 环境状态**（`.ffui-panel`）
- 标题行：`📊 环境状态` + 健康徽标（已加载后）+ 弹性空隙 + `🔄 刷新` 按钮（`disabled=_isLoadingStatus`，加载中前置 spinner）。
  - 健康徽标：`环境正常`（success）/ `FFmpeg 不可用`（danger）。
- 首次加载态：loading 行 + `正在检测环境…首次检测需要试跑硬件编码器，可能需要几十秒。`，且当已用时长 > 0.05 秒时追加徽标 `已用 {时长}`。
- 未检测到 FFmpeg 时的红色提示（`role="alert"` 由外层 alert 承担）：
  - 粗体 `未检测到可用的 FFmpeg`
  - 安装指引（按平台，逐字）：
    - Windows：`请先安装 FFmpeg：从 ffmpeg.org 下载压缩包后解压，把 bin 目录加入 PATH（或执行 winget install Gyan.FFmpeg），也可以在「设置」里直接指定 ffmpeg.exe 的完整路径。`
    - macOS：`请先安装 FFmpeg：在终端执行 brew install ffmpeg（或从 ffmpeg.org 下载静态包），也可以在「设置」里直接指定 ffmpeg 的完整路径。`
    - Linux：`请先安装 FFmpeg：Debian/Ubuntu 用 apt install ffmpeg，Fedora 用 dnf install ffmpeg，也可以在「设置」里直接指定 ffmpeg 的完整路径。`
  - 操作：`前往设置`（primary、sm，→ `/settings`）、`下载 FFmpeg`（→ `https://ffmpeg.org/download.html`，`target="_blank" rel="noreferrer"`）
- 可用时的绿色提示：`FFmpeg 已就绪 · {版本}` + 下一行等宽路径 `{FFmpegPath}`（空值显示 `—`）。
- **状态网格 `.status-grid`**（`dl`，自适应 `minmax(260px,1fr)`）；每项「小标签 + 值」，空值一律 `—`：
  | 标签 | 值 |
  | --- | --- |
  | `FFmpeg 版本` | 版本字符串 |
  | `FFmpeg 路径` | 等宽、超长省略、`title` 全文 |
  | `路径来源` | 徽标：`自动探测` / `设置中指定` |
  | `平台` | 平台显示名 |
  | `.NET 版本` | 运行时版本 |
  | `数据目录` | 等宽路径（`title` 全文）+ 徽标 `可写`（success）/ `不可写（设置无法保存）`（warning） |
  | `硬件解码方式` | 无 → 弱化文字 `未检测到（FFmpeg 未启用或不可用）`；有 → 逐项等宽 chip |
  | `模板数量` | `{N} 个 · 管理模板`（`管理模板` 链接 → `/templates`） |
  | `任务队列` | `运行中 {N} 个 · 排队 {M} 个 · 查看任务`（`查看任务` 链接 → `/tasks`） |
- **通知列表**（`.Notices` 非空时）：逐条 `ffui-alert-warning`（FFmpeg 可用时）或 `ffui-alert-danger`（不可用时），内容为服务端下发的通知原文。
- 刷新按钮行为：重新取状态（`forceRefresh: true`）+ 硬件报告 + 最近任务；成功 → Success `环境状态已刷新。`；失败 → Error `读取环境状态失败：{异常消息}（可以稍后重试，或先到「设置」检查 FFmpeg 路径）`。

**区块 2 · 🖥️ 硬件编码能力**（`.ffui-panel`）
- 标题行：`🖥️ 硬件编码能力` + 报告徽标 + `🔄 刷新检测（可能需要数秒）` 按钮（检测中变 `正在检测…` 并禁用）。
  - 报告徽标：`{N} 个硬件编码器可用`（success）/ `没有可用的硬件编码器`（warning）。
- 检测中：loading 行 + `正在逐个试跑硬件编码器，**可能需要数秒到一分钟**，请不要关闭页面。` + 徽标 `已用 {时长}`（**每 200 ms 刷新一次计时**，`Home.razor:679-704`）。
- 附加提示（`_detectNote` 非空时）：黄条显示（内部用于「耗时显示异常/不可用」等）。
- 未检测过：空状态 `还没有检测结果。点击「刷新检测（可能需要数秒）」扫描本机可用的硬件编码器。`
- 已检测：
  - 报告摘要 `.report-summary`（服务端 Summary 原文，空则 `—`）
  - 元数据徽标行：`FFmpeg {版本}`、`检测时间 yyyy-MM-dd HH:mm:ss`、`硬件解码 {N} 种`、（本轮用时 > 0.05s 时）`本轮用时 {时长}`
  - 小标题 `硬件编码器` + 徽标 `{可用数} 个可用 / 共探测 {总数} 个`
    - 无可用硬件编码器 → 空状态（逐字）：`没有可用的硬件编码器，转换会自动使用软件编码（CPU）：速度慢一些，但结果稳定。如果本机确实有独立显卡或核显，请确认当前的 FFmpeg 构建启用了对应加速（NVENC / VideoToolbox / QSV / VA-API 等）。`
    - 有 → 卡片网格（`repeat(auto-fill, minmax(260px,1fr))`），每张 `<EncoderCard>`（见 §3.4）
  - 折叠块 1：`另有 {N} 个硬件编码器不可用（点开查看原因）` → 列表 `{编码器名}`（等宽）+ 原因（空则 `未通过可用性试跑`）
  - 折叠块 2：`软件编码（CPU 兜底）· {N} 个可用`
    - 无 → `当前 FFmpeg 构建里没有检测到软件编码器。`
    - 有 → 软件编码器卡片网格；另有不可用的软件编码器原因列表
- 检测按钮：成功 → Success `硬件检测完成，用时 {时长}。`；失败 → Error `硬件检测失败：{异常消息}（可以稍后重试；转换会自动改用软件编码）`
- **时长格式化**：`>= 60 秒` → `m:ss`；否则 `{0.0} 秒`。

**区块 3 · 🕒 最近任务**（`.ffui-panel`）
- 标题行：`🕒 最近任务` + 右侧 `查看全部任务`（sm 按钮 → `/tasks`）
- 空状态：`还没有任何任务，去「转换」页开始第一个吧。` + 按钮 `开始转换`（primary、sm → `/convert`）
- 表格（表头：`文件` / `状态` / `耗时` / `创建时间` / `操作`），取最近 **5** 条（`GetTaskHistoryAsync(5)`）：
  - 文件：仅文件名（`ellipsis` + `title` 全文）
  - 状态徽标：`已完成`(success) / `失败`(danger) / `运行中`(info) / `已取消`(warning) / `已跳过`(warning) / 其它 `等待中`(默认)
  - 耗时：`DurationSeconds` 有值 → `<=0` 显示 `-`；≥1 小时 `H:MM:SS`；否则 `MM:SS`；无值 → `-`
  - 创建时间：`MM-dd HH:mm`（本地时间）
  - 操作：仅**已完成**显示 `打开目录` 按钮（打开系统文件管理器）

**打开输出目录的行为与文案**（全站统一，见 §4.8）
- 目录为空或不存在 → Warning `输出目录不存在，可能已被移动或删除。`
- 成功 → Success `已在文件管理器中打开输出目录。`
- 异常 → Error `无法打开目录：{异常消息}`（Tasks 页文案为 `无法打开输出目录：{异常消息}`）

**实时行为**：首页**不订阅任务事件**，任务队列数字（`TaskService.ActiveCount` / `QueuedCount`）在渲染时读取；检测耗时用 200 ms 定时器刷新。

**跨页跳转**：`/convert`、`/batch`、`/templates`、`/tasks`、`/settings`、外链 ffmpeg.org。

### 4.2 转换 `/convert`（Convert）

来源：`Components/Pages/Convert.razor:1-1346`
截图：![转换](../../../FFmpegWebUI/readmefiles/cv.png)；命令预览细节：![转换 · 命令预览](../../../FFmpegWebUI/readmefiles/cv-preview.png)；深色：![深色转换](../../../FFmpegWebUI/readmefiles/dark-convert.png)

**页头**
- `🔄 媒体转换`（h1）
- 状态徽标（三选一）：
  - FFmpeg 不可用 → `FFmpeg 不可用`（danger）
  - 可用且有硬件编码器 → `FFmpeg {版本}`（success）+ `🚀 {N} 个硬件编码器可用`（info）
  - 可用但无硬件 → `FFmpeg {版本}`（success）+ `🧮 使用软件编码`（默认色）

**FFmpeg 不可用时的红色提示条**（`role="alert"`）：
- `⚠️` + 粗体 `未找到可用的 FFmpeg，无法开始转换。`
- `当前使用的路径：{FFmpegPath}`（等宽）
- 操作：`前往设置指定 FFmpeg 路径`（primary、sm → `/settings`）、`重新检测`（加载中前置 spinner 并禁用）
- 重新检测成功 → Success `已重新检测环境`；失败 → Error `重新检测失败：{异常消息}`

**布局**：`.convert-layout` 为两列网格（`minmax(0,1fr)` + `minmax(0,1fr)`，gap 1.25rem）；**≤1000px 时变单列**。

#### 面板 `1 · 输入与输出`
1. **输入文件**（`FileSelector`）：`Label="输入文件"`、占位 `输入视频或音频文件的完整路径`、`ValidateExists=true`、`ShowFileInfo=true`。
   - 路径变化后触发 `GetMediaInfoAsync` 探测（对比 `_lastProbedPath`，只在变化时执行）。
   - 探测中：loading 行 `正在读取媒体信息…`
   - 探测结果徽标行（`.media-info`）：
     | 条件 | 徽标文案 |
     | --- | --- |
     | 总是 | `{FormatDisplay}`（info）、`时长 {DurationDisplay}` |
     | 有视频 | `{宽}×{高}`、`{帧率 0.##} fps`、`{视频编码或"未知"}` |
     | 有音频 | `{音频编码或"未知"} · {声道数} 声道` |
     | 无音频 | `无音轨`（warning） |
     | 字幕 > 0 | `{N} 条字幕` |
   - 探测返回 null → Warning `无法读取该文件的媒体信息，可能是格式不受支持或文件已损坏。`；异常 → Error `读取媒体信息失败：{异常消息}`
2. **输出目录**（`FileSelector`）：`Label="输出目录"`、占位 `转换结果的保存目录`、`IsDirectory=true`、`ValidateExists=false`。默认值取设置里的 `DefaultOutputDirectory`。
3. **输出文件名**（text，`oninput` 绑定）：
   - 占位 = **自动生成文件名预览**：命名规则为「添加后缀」时 `{输入文件名}{后缀}.{输出扩展名}`（例 `video_converted.mp4`）；「保持原名」时 `{输入文件名}.{扩展名}`；输入名为空时用 `输入文件名`。默认展后缀为 `_converted`。
   - 帮助文案：`留空则按「设置 → 输出文件命名规则」自动生成。支持 {filename} {date} {now:yyMMdd_HHmm} {random:6}。` + 链接式按钮 `查看全部占位符` / `收起全部占位符`（点击展开）
   - 展开后的占位符说明列表（逐字）：
     - `{filename}` 原文件名（不含扩展名）
     - `{ext}` 原扩展名
     - `{outputext}` 输出扩展名
     - `{dir}` 原文件所在文件夹名
     - `{date}` 日期 yyyyMMdd
     - `{time}` 时间 HHmmss
     - `{datetime}` yyyyMMdd_HHmmss
     - `{year}` `{month}` `{day}` `{hour}` `{minute}` `{second}`
     - `{now:格式}` 自定义日期格式，如 `{now:yyMMdd_HHmm}`
     - `{random}` 8 位随机串
     - `{random:N}` N 位随机串，如 `{random:6}`

#### 面板 `2 · 选择转换模板`
- 内部就是 `<TemplateSelector SelectedTemplate=… SelectedTemplateChanged=OnTemplateSelectedAsync InputPath=_inputPath />`（见 §3.6）。
- **必须注意**：选择模板一定要触发「派生状态重算」（校验、参数默认值、自动文件名提示、命令预览），否则会出现「选完模板命令预览是空的」（`Convert.razor:137-139` 注释）。

#### 面板 `3 · 模板参数`（仅当已选模板时出现）
- 标题 `3 · 模板参数` + 模板名徽标。
- 若模板存在 Error 级校验问题 → 红色提示 `⛔ 该模板存在问题，无法执行：` + 逐条 `{issue.Message}`（列表项）。
- 参数数为 0 → 弱化文字 `该模板没有可调参数，将直接使用模板预设值。`
- 否则 → `<TemplateParameterEditor Disabled="_isStarting">`（启动中禁止改参数）。

#### 面板 `4 · 确认并开始`（仅当输入路径非空且已选模板时出现）
1. **预览摘要 `.preview-grid`**（三行，标签固定宽 56px）：
   | 标签 | 值 |
   | --- | --- |
   | `输入` | 完整输入路径（等宽、`title` 全文） |
   | `输出` | 解析后的输出路径（等宽）；未确定时 `尚未确定` |
   | `编码器` | 无画像 → 弱化 `由模板内部指定`；否则徽标 `🚀 硬件加速`（success）或 `🧮 软件编码` + ` · {编码器名}`，后跟弱化族显示名 |
2. **文件冲突提示**（`_conflictNotice` 非空时，黄条 + 两个按钮）：
   | 情形 | 文案 | 操作按钮 |
   | --- | --- | --- |
   | 设置 = 跳过 | `输出文件已存在：按当前设置将跳过本次转换，不会覆盖原文件。` | `覆盖已有文件` / `保留两者（自动改名）` |
   | 设置 = 重命名 | `输出文件已存在，将自动改名后保存，避免覆盖原文件。` | 同上 |
   | 设置 = 覆盖 | `输出文件已存在，将按设置覆盖。` | 同上 |
   | 输出 == 输入 | `输出路径与输入文件相同：已自动改名，避免把源文件覆盖掉。（把输出目录换成别的目录、或让输出文件名带上后缀，可以避免改名。）` | 同上 |
   - `覆盖已有文件` → 强制覆盖并重算预览；`保留两者（自动改名）` → 取消强制覆盖并重算。
3. **预览警告**（`_previewWarnings` 逐条，黄条 + `💡`）。其中最典型的是硬件不可用警告（逐字）：
   `❌ 该模板要求使用 {族显示名} 硬件编码，但本机没有可用的对应编码器，无法执行。建议改用「推荐」分类下会自动挑选编码器的模板。`
   生成异常时整条替换为：`生成命令预览失败：{异常消息}`
4. **完整命令区块**：
   - 头部标题：`完整命令（可直接复制到终端执行）`
   - 按钮 `📋 复制`（成功后 2 秒变 `✓ 已复制`）与 `自动换行` / `不换行` 切换（**默认自动换行**）
   - 代码块内容：`ffmpeg {预览命令}`（等宽 0.82rem，默认 `white-space:pre` 横向滚动；换行模式 `pre-wrap + break-all`）
5. **操作栏 `.action-bar`**
   | 按钮 | 文案 | 禁用条件 | 动作 |
   | --- | --- | --- | --- |
   | 主按钮 | `🚀 开始转换`（lg、primary）；启动中 spinner + `正在启动…` | `!CanStart` | 创建并启动任务 |
   | 试运行 | `🧪 试运行模板`；运行中 spinner + `试运行中…` | `_isTesting` | `TestTemplateAsync(模板, 当前参数值)` |
   | 重置参数 | `重置参数`（仅当模板有参数时） | `_isStarting` | 恢复模板默认参数值并重算预览 |
   - `CanStart` = 非启动中 且 FFmpeg 可用 且 输入非空 且 输出目录非空 且 已选模板 且 无 Error 级模板问题 且 模板不是「本机不可运行」
   - **不可开始的原因文案（按优先级，弱化文字显示在按钮右侧）**：
     1. `FFmpeg 不可用，请先到设置里指定路径`
     2. `请选择输入文件`
     3. `请选择转换模板`
     4. `请选择输出目录`
     5. `当前模板存在问题，请改用其他模板`
     6. `当前模板要求的硬件编码器在本机不可用，请改用「推荐」分类下的模板`
6. **试运行结果块**（成功绿条 / 失败红条，`role="status"`）：`{Message}`；失败且有输出时提供 `查看 FFmpeg 输出` 折叠块（内含 `<pre>`）；右下 `关闭` 按钮清空结果。
   - Toast：成功 `模板试运行成功，该模板在本机可用`；失败 `模板试运行失败，请看下方详情`；异常 `试运行出错：{异常消息}`

**开始转换的行为与文案**
- 先确保输出目录存在（不存在则创建）；再检查可写性；不可写 → Error `输出目录不可写：{目录}。请换一个目录或检查文件权限。`
- 输出路径无法确定 → Error `无法确定输出路径，请检查输出目录设置。`
- 成功 → Success `已开始转换：{输入文件名}`，并自动展开该任务的日志
- 任务已创建但未能立即调度 → Warning `任务已创建但未能立即调度，请到「任务列表」查看。`
- `FileNotFoundException` / `ArgumentException` → 直接显示异常消息（Error）
- 其它异常 → Error `创建任务失败：{异常消息}`

#### 面板 `⏱️ 进行中的转换`（`.activeTasks.Count > 0` 时）
- 标题：`⏱️ 进行中的转换` + 数量徽标 + `刷新` 按钮（重新拉取运行中 + 等待中合并列表，按创建时间倒序）
- 每个任务卡：
  - 头部：文件名（`title` 完整路径）+ 状态徽标 + 模板名（弱化）
  - `<ProgressBar>`（见 §3.1）
  - 操作按钮：
    | 条件 | 按钮 | 提示（`title`） | 行为与 Toast |
    | --- | --- | --- | --- |
    | Running | `⏏️ 优雅停止` | `发送 q 让 FFmpeg 写完文件尾再退出，已产出的文件仍然可用` | `CancelTaskAsync(id, graceful:true)`；成功 → Warning `已发送优雅停止指令，FFmpeg 会写完文件尾再退出。`；已在结束 → Warning `该任务已经不在运行中。` |
    | Running | `❌ 强制终止`（danger） | — | graceful:false；成功 → Warning `已取消任务，未完成的输出文件会被清理。` 并刷新；否则 Warning `该任务已经结束。` |
    | 非 Running | `取消排队`（danger） | — | 同上（强制） |
    | 总是 | `▼ 查看日志` / `▲ 收起日志` | — | 展开/收起该任务的日志区 |
  - 展开时渲染 `<LogViewer LogText MaxHeight="260px">`；首次展开如果本地没有缓冲，会从任务记录里读 `LogOutput` 兜底。

#### 面板 `📜 最近任务`（`_recentTasks.Count > 0` 时）
- 标题 `📜 最近任务` + 右侧 `查看全部`（→ `/tasks`）
- 表格表头：`输入文件` / `状态` / `耗时` / `创建时间` / `操作`；取最近 **8** 条
  - 输入文件：文件名 + `ellipsis`（max-width 260px）+ `title`
  - 状态徽标同全站映射
  - 耗时：`DurationSeconds` 格式化（`-` / `MM:SS` / `H:MM:SS`）
  - 创建时间：`MM-dd HH:mm`
  - 操作：`重试`（总是）+ `打开目录`（仅已完成）
    - 重试：找不到原任务 → Error `找不到原任务，无法重试。`；成功 → Success `已重新加入队列`；异常 → Error `重试失败：{异常消息}`

**实时行为（订阅的任务事件）**
| 事件 | 行为 | 节流/上限 |
| --- | --- | --- |
| `TaskProgressChanged` | 更新对应活动任务的 Progress / Speed / Eta 并重渲染 | 事件驱动，无额外节流 |
| `TaskStatusChanged` | Toast 播报（见下）+ 重新拉取活动与最近任务 | 无 |
| `TaskLogAppended` | 追加到该任务的实时日志缓冲；仅当该任务正展开时重渲染 | 内存最多保留 **800 行/任务**（超出丢最旧行） |
- 任务结束 Toast 文案（`HandleStatusChangedAsync`）：
  - 完成 → `✅ 转换完成：{文件名}`
  - 失败 → `❌ 转换失败：{文件名} — {错误消息}`
  - 取消 → `已取消：{文件名}`
  - 跳过 → `已跳过：{文件名}（输出文件已存在）`

**入口/出口跳转**：`页面 模板` → `/convert?template=<id>`（预选）；`/convert` → `/tasks`（查看全部）；`/convert` → `/settings`（FFmpeg 不可用）。

### 4.3 批量处理 `/batch`（BatchProcess）

来源：`Components/Pages/BatchProcess.razor:1-523`
截图：![批量处理](../../../FFmpegWebUI/readmefiles/batch.png)

**页头**：`📁 批量处理`（h1）+（当设置 `MaxConcurrentTasks > 1` 时）徽标 `并发 {N} 个任务`（info）。

**布局**：`.batch-layout` 两列（`minmax(0,1.05fr)` + `minmax(0,1fr)`），**≤1000px 单列**。

**面板 `① 选择文件`** → `<BatchFileList Files=… FileStatuses=… IsProcessing=… />`（见 §3.10）

**面板内 `② 输出目录` 子块**（用 1px 上边框隔离）
- `<FileSelector Label="输出目录" Placeholder="转换结果的保存目录" IsDirectory=true ValidateExists=false />`，默认取设置里的默认输出目录。
- 固定提示：`批量处理时，输出文件名由模板的扩展名 + 全局命名规则生成。同名文件按「设置 → 文件已存在时的处理方式」处理，不会卡住等待确认。`

**面板 `③ 选择模板`**
- `<TemplateSelector @bind-SelectedTemplate="_selectedTemplate" InputPath="{文件列表第 1 个文件或 null}" />`

**面板 `④ 模板参数`**（仅当选了模板且该模板参数数 > 0）
- 标题：`④ 模板参数` + 模板名徽标 + 弱化文字 `对本批次所有文件生效`
- `<TemplateParameterEditor Disabled="_isProcessing">`
- 模板变化时的参数重置规则：若当前参数字典为空，或存在「模板已不包含的键」，则整体重置为该模板的默认值（`206-218`）。

**操作栏 `.action-bar`**
| 元素 | 文案 | 显示/禁用条件 |
| --- | --- | --- |
| 主按钮 | `🚀 开始批量处理`（lg、primary）；处理中 spinner + `正在处理…` | `disabled = !CanStart` |
| 取消全部 | `❌ 取消全部`（danger） | 仅 `_isProcessing && _batchTask != null` |
| 预检文案 | `将处理 {N} 个文件，输出格式 .{扩展名}` | 有文件 + 已选模板 + 非处理中 |
| 不可开始原因 | 见下 | `!CanStart && !_isProcessing` |

**不可开始原因（逐字，按优先级）**：`请先添加要处理的文件` → `请选择转换模板` → `请选择输出目录`

**`CanStart` 条件**：非处理中 且 文件数 > 0 且 输出目录非空 且 已选模板。

**开始批量的行为与文案**
1. 输出目录不存在 → 创建；不可写 → Error `输出目录不可写：{目录}。请换一个目录或检查文件权限。` 并中止。
2. 清空文件状态与任务集合，调用 `StartBatchTaskAsync(文件列表, 输出目录, 模板Id, 参数)`；所有文件先标记为 `Pending`。
3. 成功 → Success `已提交 {N} 个文件到处理队列`，启动 **1 秒周期**的刷新定时器（定时器内只读数据库 + 更新内存对象，UI 更新回到渲染线程；重入用 `Interlocked` 保护）。
4. 异常 → Error `启动批量处理失败：{异常消息}` 并复位处理中标记。
5. **执行完全交给后台队列**：页面跳转/刷新都不会中断。

**面板 `📊 处理进度`**（当有批次任务或处理中时出现）
- 标题：`📊 处理进度` + 批次名徽标 + `刷新` 按钮 + 条件按钮：
  - 非处理中且已完成数 > 0 → `打开输出目录`（打开第一个已完成任务的目录；无已完成任务 → Info `还没有成功完成的文件。`）
  - 非处理中且失败数（失败+取消）> 0 → `重试全部失败项 ({N})`
- 内部 `<BatchProgressPanel Tasks=… IsProcessing=… OnRetry OnCancel OnOpenOutputFolder />`（见 §3.11）
- **单任务取消**：运行中发优雅停止，否则强制取消；成功 → Warning `已发送优雅停止指令。` 或 `已取消该任务。`；失败 → Info `该任务已经结束。`
- **重试单项**：找不到原任务 → Error `找不到原任务，无法重试。`；成功 → Success `已重新加入队列`，重新进入处理中并重启定时器；异常 → Error `重试失败：{异常消息}`
- **取消全部**：`CancelBatchAsync(batchId)` → Warning `已取消本批次中所有等待和运行中的任务。`；异常 → Error `取消失败：{异常消息}`
- **重试全部失败项**（失败 + 已取消）：
  - 空 → Info `没有需要重试的任务。`
  - 有排队成功 → Success `已重新排队 {N} 个任务`
  - 无一排队 → Error `重试失败：{第一条错误消息 或 没有任务被重新排队}`

**批次完成的判定与 Toast**（`RefreshBatchAsync`）
- 全部任务处于 `Completed | Failed | Cancelled | Skipped` 且此前在处理中 → 停止定时器：
  - 失败数为 0 → Success `✅ 批量处理完成，成功 {N} 个文件`
  - 否则 → Warning `批量处理完成：成功 {N} 个，失败 {M} 个`
- 统计口径：`_failedCount` = `Failed + Cancelled`；重试产生的任务（`RetryOfTaskId != null`）也纳入统计并去重。
- 定时刷新中的异常**静默忽略**（避免刷屏），只有显式刷新才报错。

**实时行为**
| 来源 | 频率 | 作用 |
| --- | --- | --- |
| `TaskProgressChanged` | 事件驱动 | 直接改内存中该任务的 Progress / Speed / Eta 并重渲染 |
| `TaskStatusChanged` | 事件驱动 | 仅当属于本批次时触发一次全量刷新 |
| 内部 `System.Threading.Timer` | **每 1 秒** | 重新读取批次任务列表，直到全部结束才停止 |

**跨页跳转**：页面内所有操作为就地更新，无路由跳转。



### 4.4 任务列表 `/tasks`（Tasks）

来源：`Components/Pages/Tasks.razor:1-1376`
截图：![任务列表](../../../FFmpegWebUI/readmefiles/tasks.png)

**页头**
- `🗂️ 任务列表`（h1）
- 徽标：`运行中 {N} · 排队 {M}`（实时计数）
- 右侧弹性空隙 + `🔄 刷新` 按钮（刷新中前置 spinner 并禁用）

#### 区块 1 · 🚀 进行中
- 标题：`🚀 进行中` + 数量徽标
- 空状态：`当前没有正在执行的转换。` + 按钮 `开始转换`（primary、sm → `/convert`）
- 每个任务卡 `.task-card`（`--surface-3` 底、1px 边框、`--radius-md`）：
  - 头部行：`{输入文件名}`（`ellipsis`，max-width 320px，`title` 全路径）+ 箭头 `➜` + `{输出文件名}`（同上）+ 徽标 `运行中`（info）+ 模板名（弱化）
  - `<ProgressBar Label="转换进度">`（含状态徽标 `运行中`）
  - 元信息行（弱化，三条）：`开始于 {HH:mm:ss 或 -}`、`已运行 {格式时长}`、`输入 {格式化大小}`
    - 「已运行」= `UtcNow - StartedAt`，格式化规则同耗时（`-` / `MM:SS` / `H:MM:SS`）
  - 操作按钮：
    | 按钮 | `title` | 行为与 Toast |
    | --- | --- | --- |
    | `⏏️ 优雅停止` | `发送 q 让 FFmpeg 写完文件尾再退出，已产出的文件仍然可用` | 成功 → Success `已发送优雅停止（q），FFmpeg 会写完文件尾再退出。` |
    | `❌ 强制终止`（danger） | `立即结束 FFmpeg 进程，未完成的输出文件会被清理` | 成功 → Success `已强制终止任务，未完成的输出文件会被清理。` |
    | `▼ 查看日志` / `▲ 收起日志` | — | 展开时渲染 `<LogViewer MaxHeight="320px">`；无日志时显示弱化文字 `还没有日志输出。` |
  - 取消失败（任务已结束/无法取消）→ Warning `这个任务已经结束或无法取消，列表可能不是最新的，已为你刷新。`；异常 → Error `停止任务失败：{异常消息}`

#### 区块 2 · ⏳ 等待中
- 标题：`⏳ 等待中` + 数量徽标
- 空状态：`队列里没有等待中的任务。`
- 每行（`.pending-item`）：`{输入文件名}`（`ellipsis`，`title` 全路径）+ 弱化 `创建于 MM-dd HH:mm`；右侧徽标 `等待中` + 按钮 `取消排队`
  - 成功 → Success `已取消排队，这个任务不会再开始执行。`

#### 区块 3 · 📜 历史记录
- 标题：`📜 历史记录` + 计数徽标（`aria-live="polite"`），格式：`{可见数} 条记录`；若被筛选则 `{可见数} / {总数} 条记录`
- **筛选栏 `.filters`**（5 个控件 + 刷新）：
  | 标签 | 控件 | 选项 / 默认 | 触发 |
  | --- | --- | --- | --- |
  | `状态` | select | `全部`（默认）/ `运行中` / `等待中` / `已完成` / `失败` / `已取消` / `已跳过` | 改变查询条件 → **重新读库** |
  | `文件名` | search 输入 | 占位 `按文件名筛选`；匹配输入文件名**或**输出文件名（忽略大小写） | 仅内存过滤，不查库 |
  | `显示条数` | select | `50`（默认）/ `100` / `500` | 重新读库 |
  | `排序` | select | `最新优先`（默认）/ `最早优先` | 仅内存排序 |
  | — | 按钮 `🔄 刷新` | 刷新中禁用 | 重新读库 |
- **批量操作栏 `.bulk-bar`**（弱化文字 `批量操作` + 三个按钮）：
  | 按钮 | 确认框标题 | 正文 | Detail | 确认按钮 | 危险样式 |
  | --- | --- | --- | --- | --- | --- |
  | `♻️ 重试全部失败任务` | `重试全部失败任务` | `将扫描最近 1000 条失败的任务，逐个重新加入队列执行。` | `会复用原来的输入文件、模板与参数。` | `重新排队` | 否 |
  | `🧹 清理失败与取消的任务` | `清理失败与取消的任务` | `将删除全部「失败」和「已取消」的任务记录，运行中与等待中的任务不受影响。此操作不可撤销。` | `只删除记录，不会删除任何输出文件。` | `确认` | 是 |
  | `🗑️ 清空历史` | `清空历史记录` | `将删除全部已完成 / 失败 / 已取消 / 已跳过的历史记录，运行中与等待中的任务保留。此操作不可撤销。` | `只删除任务记录，磁盘上的输出文件不会被删除。` | `确认` | 是 |
  - 确认框忙碌时禁用两键并显示 spinner；关闭/取消会清空待执行动作。
  - 执行结果 Toast：
    - 清理失败与取消：Success `已清理 {N} 条记录（失败 {F} 条、已取消 {C} 条）。`
    - 清空历史：Success `已清空 {N} 条历史记录（运行中与等待中的任务保留）。`
    - 重试全部失败：扫描最近 **1000** 条失败任务；结束后
      - 成功：Success `失败任务重试完成：已重新排队 {N} 个{，跳过 S 个}{，E 个出错（{首条错误}）}`
      - 有出错：Warning 同格式文案
      - 无失败任务 → Warning `没有失败的任务需要重试。`
      - 读取失败任务异常 → Error `读取失败任务失败：{异常消息}`
    - 未选择动作 → Warning `没有选择任何批量操作。`
    - 异常 → Error `批量操作失败：{异常消息}`

- **历史表格**（表头逐字：`输入文件` / `输出文件` / `状态` / `耗时` / `创建时间` / `操作`）：
  - 输入/输出文件：仅文件名 + `ellipsis`（max-width 240px，≤768px 时 160px）+ `title` 全路径
  - 状态徽标：全站统一映射（`已完成`/`失败`/`运行中`/`已取消`/`已跳过`/`等待中`）
  - 耗时：`-` / `MM:SS` / `H:MM:SS`
  - 创建时间：`MM-dd HH:mm`
  - **行级左侧色条**：失败 → `--danger-color` 内阴影 3px；已取消 / 已跳过 → `--warning-color`
  - 操作列（始终 `white-space:nowrap`）：
    | 按钮 | 显示条件 | 行为 |
    | --- | --- | --- |
    | `日志` / `收起` | 总是 | 展开/收起详情行 |
    | `重试` | 任务已结束（完成/失败/取消/跳过） | 重试；成功 → Success `已重新加入队列：{输入文件名}`；找不到 → Error `找不到原任务记录，无法重试。`；异常 → Error `重试失败：{异常消息}` |
    | `打开输出目录` | 同上 | 见 §4.8 |
    | `复制命令` | 总是 | 复制 `ffmpeg {ActualCommand}`；无实际命令 → Warning `这个任务还没有生成实际命令（可能尚未开始执行），暂时没有可复制的内容。` |
    | `复制错误信息` | 仅失败 | 无错误信息 → Warning `这个任务没有记录错误信息。` |
    | （复制统一失败文案） | — | Error `复制失败：浏览器不允许访问剪贴板，请在详情里手动选中文本。`；异常 → Error `{复制命令失败\|复制错误信息失败}：{异常消息}` |
  - **详情行**（展开时，跨 6 列，`--surface-3` 底）网格字段：
    | 标签 | 值 |
    | --- | --- |
    | `输入路径` | 等宽全路径（空 → `—`） |
    | `输出路径` | 等宽全路径（空 → `—`） |
    | `模板` | `TemplateName`（空 → `—`） |
    | `输入 / 输出大小` | `{输入} → {输出}`（输出缺失 → `—`） |
    | `创建 / 完成时间` | `yyyy-MM-dd HH:mm:ss`；有完成时间时 `{创建} → {完成}` |
    | `说明` | 仅状态为已跳过：`输出文件已存在，按「文件已存在时的处理方式」的设置跳过。` |
    | `错误信息` | 有值时以红色显示 |
    - 有日志 → `<LogViewer MaxHeight="320px">`；无 → 弱化 `暂无日志输出。`
  - **空状态（两种文案）**：
    - 有历史但被筛掉：`没有符合当前筛选条件的任务，试试清空筛选条件或点「刷新」。`
    - 完全没历史：`还没有任何任务，去「转换」页开始第一个吧。` + 两个按钮 `去「转换」页开始第一个`（primary、sm → `/convert`）与 `批量处理`（→ `/batch`）

**实时行为**
| 来源 | 频率 | 说明 |
| --- | --- | --- |
| `TaskProgressChanged` | 事件驱动 | 只在内存里改运行中任务的 Progress / Speed / Eta，**不查库** |
| `TaskStatusChanged` | 事件驱动 + **200 ms 合并延迟** | 播报 Toast 后重新读库；若上一次刷新还在进行会再等一个 200 ms，把密集变化合并为一次查询 |
| `TaskLogAppended` | 事件驱动 | **仅当展开的正是该任务**时才追加到缓冲并重渲染 |
| 运行中自动刷新循环 | **每 2 秒** | 只查「运行中」任务（`GetRunningTasksAsync`）；无运行中任务时跳过；刚好跑完则整体刷新一次 |
- 日志缓冲：每个展开任务最多 **800 行**；同时最多缓存 **12 个任务**的缓冲（超出回收最旧的，当前展开的不回收）。
- 状态变化 Toast（`NotifyStatusChange`，逐字）：
  - 完成 → `✅ 转换完成：{文件名}`
  - 失败 → `❌ 转换失败：{文件名}`，有错误消息时 `❌ 转换失败：{文件名} — {错误消息}`
  - 取消 → `已取消：{文件名}`
  - 跳过 → `已跳过：{文件名}（输出文件已存在）`
  - 等待中/运行中：不打扰用户
  - 任务名取不到 → `任务`
- 读取任务列表失败 → Error `读取任务列表失败：{异常消息}`；界面刷新失败 → Warning `界面刷新失败：{异常消息}`；自动刷新停止 → Warning `运行中任务的自动刷新已停止：{异常消息}`

**跨页跳转**：`/convert`（空状态按钮）、`/batch`（空状态按钮）。

### 4.5 模板管理 `/templates`（Templates）

来源：`Components/Pages/Templates.razor:1-1019`
截图：![模板](../../../FFmpegWebUI/readmefiles/tpl.png)

**页头**
- 标题区：`📋 模板管理`（h1）+ 副标题 `浏览、筛选并预览模板；点「选择」会带着该模板打开转换页面。`
- 计数徽标：`全部 {N}`、`系统预设 {N}`（info）、`我的模板 {N}`（primary）

**提示条堆叠 `.alert-stack`**
1. 系统预设同步结果（有值时）：
   - 有变化 → 绿条 `✅` + `{同步摘要}（每次打开本页都会自动同步一次系统预设）`
   - 无变化 → 蓝条 `ℹ️` + 同上格式
   - 摘要为空时的兜底文案：`系统模板已是最新（预设 v{版本}）`
2. 硬件能力条：
   - 正常时 `🚀` 前缀（info）；无硬件或检测失败时 `⚠️` 前缀（warning）
   - 文案优先级：服务端 `Summary` 非空则直接用（并追加 `（检测于 FFmpeg {版本}）`）；否则
     - 有硬件 → `检测到 {N} 个可用的硬件编码器。`（再追加 FFmpeg 版本）
     - 无硬件 → `本机未检测到可用的硬件编码器，仅软件编码可用。`
   - 初始占位：`正在检测本机可用的硬件编码器…`
   - 检测失败 → `硬件检测失败，暂时无法判断哪些模板可在本机运行：{异常消息}` + Warning Toast `硬件检测失败，模板可用性将显示为「未知」：{异常消息}`

**操作按钮行**
| 按钮 | 禁用条件 | 行为与 Toast |
| --- | --- | --- |
| `➕ 创建模板` | — | 打开编辑器（新建用户模板） |
| `⬇️ 导出全部` | 导出中 或 模板数为 0 | 下载 JSON |
| `⬇️ 导出当前筛选结果（{N}）` | 导出中 或 筛选结果数为 0 | 下载 JSON（仅当前筛选结果） |
| `📥 导入` | — | 打开导入弹窗 |

- 导出文件名为 `ffmpegwebui-templates-yyyyMMdd-HHmmss.json`，MIME `application/json`。
  - 成功 → Success `已导出{当前筛选结果|全部模板} {N} 个到 {文件名}`
  - 无内容 → Warning `没有可导出的模板。`
  - 浏览器未触发下载 → Error `导出失败：浏览器没有触发下载，请检查浏览器的下载权限或拦截设置。`
  - 异常 → Error `导出失败：{异常消息}`

**筛选工具栏 `.toolbar`**
| 控件 | 标签 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- | --- |
| 搜索 | `搜索` | search | 空 | 占位 `搜索名称、描述、分类、标签…`，`aria-label="搜索模板"`，**250 ms 防抖** |
| 分类 | `分类` | select | `全部分类`（空值） | 选项来自 `GetCategoriesAsync()` |
| 排序 | `排序` | select | `推荐优先` | `推荐优先` / `名称` / `最近修改` |
| 只看可用 | `只看本机可用的模板` | checkbox | 未勾选 | 只保留 `IsAvailable === true`（**未知状态不算可用**） |
| 视图 | — | 分段按钮组（`role="group" aria-label="视图切换"`） | `flat` | `▦ 平铺` / `☰ 分组`，`aria-pressed` 标记激活 |

- 搜索匹配字段（忽略大小写）：`Name`、`Description`、`Category`、`OutputExtension`、`RequiredEncoder`、`Tags`。
- 结果计数行（`aria-live="polite"`）：加载中 `正在加载模板…`；否则 `{筛选数} 个模板符合当前条件（共 {总数} 个）`。
- 排序规则：
  - `推荐优先`：`IsRecommended` 降序 → `SortOrder` → 名称（当前文化）
  - `名称`：名称（当前文化）
  - `最近修改`：`UpdatedAt` 降序 → 名称
- 分组模式：按 `Category` 分组（空 → `未分类`），组标题为分类名 + 数量徽标；卡片网格 `repeat(auto-fill, minmax(300px,1fr))`。

**空状态**
| 条件 | 文案与操作 |
| --- | --- |
| 无任何模板 | `还没有任何模板，先创建一个吧。` + 按钮 `创建第一个模板`（primary） |
| 筛选无结果 | `没有找到匹配的模板。` + 按钮 `清除筛选条件` |

**卡片可用性判定（`BuildCardState`，`620-666`）**
| 模板模式 | 判定 | 结果 |
| --- | --- | --- |
| `Required` 且未指定编码器 | — | `IsAvailable=false`，原因 `该模板要求硬件加速，但没有指定编码器族，无法在本机使用。` |
| `Required` 且硬件未检测完成 | — | `IsAvailable=null`，原因 `硬件检测未完成，无法确认该模板是否可以在本机运行。` |
| `Required` 且族有可用成员 | — | `IsAvailable=true`，`SelectedEncoderName` = 该成员显示名 |
| `Required` 且无可用成员 | — | `IsAvailable=false`，原因 `未检测到可用的 {族显示名} 编码器，本机无法使用该模板。` |
| `Auto` | 始终可用（会回退软件） | `IsAvailable=true`；若检测完成且命令含 encoder 类占位符，取缓存中的推荐编码器作为 `SelectedEncoderName` |
| `None` | 始终可用 | `IsAvailable=true`，无编码器名 |

- 「含 encoder 类占位符」判定：命令里出现 `{encoder}`、`{quality_args}`、`{preset_args}` 或 `{hwupload_args}`。
- 编码器缓存按 `(codec, requiredEncoder)` 去重查询，避免渲染时逐行调用服务；查询失败只提示一次 Warning `无法推算模板实际使用的编码器：{异常消息}`。

**卡片交互（委托给 `TemplateCard`，见 §3.7）**
| 动作 | 行为与文案 |
| --- | --- |
| 选择 / 直接使用 | Toast `已打开转换页面，模板已预选：{模板名}` 并跳转 `/convert?template={id}`；跳转异常 → Error `打开转换页面失败：{异常消息}`。**注意：系统模板卡片上的按钮文案是 `直接使用`，用户模板是 `选择`，两者行为相同** |
| 编辑 | 打开 `TemplateEditor`（系统模板会带 `IsSystem=true`，标题为「编辑系统预设」） |
| 删除 | 打开确认框（见下） |
| 查看详情 | 打开 `TemplateDetailViewer` |
| 复制为自定义模板 | `CopyTemplateAsync` → Success `已复制为「{新名字}」`；异常 → Error `复制模板失败：{异常消息}` |

**删除确认框（两种形态）**
| 目标 | 标题 | 正文 | Detail | 确认按钮 |
| --- | --- | --- | --- | --- |
| 用户模板 | `删除模板` | `确定要删除模板「{名称}」吗？此操作不可撤销。` | `删除只影响模板本身，已经用它执行过的任务记录不会受影响。` | `删除`（危险） |
| 系统预设 | `删除系统预设模板` | `「{名称}」是系统预设模板，无法在这里删除。` | `删除系统预设请前往「预设模板管理」页面；即使在那里删除，只要它仍然存在于内置预设列表中，下次系统预设同步（应用升级或重置）时它还会重新出现。` | `前往管理页面`（危险） |
- 点「前往管理页面」：Warning `系统预设模板不能在这里删除，已跳转到「预设模板管理」页面。` + 跳转 `/templates-admin`；跳转失败 → Error `跳转失败，请手动打开 /templates-admin：{异常消息}`
- 用户模板删除结果：成功 → Success `模板「{名称}」已删除`；服务返回未删除 → Error `删除失败：服务没有删除「{名称}」，它可能已经被删除，或它是系统预设模板。`；异常 → Error `删除模板失败：{异常消息}`
- 编辑器保存成功回调：`OnTemplateSaved` → `_editorOpen=false` + Toast `模板「{名称}」已保存` + 重新加载模板列表。

**导入弹窗（`<Modal Size="lg">`，标题 `导入模板`）**
- 说明（逐字）：`把「导出」得到的 JSON 内容粘贴到下面。浏览器出于安全限制不允许页面直接读取剪贴板，请手动粘贴（⌘/Ctrl + V）。`
- `<textarea rows="10">`，`aria-label="模板 JSON 内容"`，占位 `在此粘贴包含 templates 数组的 JSON`（`oninput` 绑定）
- 复选框：`导入为系统模板（高级选项）`
  - 勾选后出现警告（模板管理页原文）：`导入为系统模板后，这些模板会进入「系统预设」分组，并可能在后续的系统预设同步或重置时被内置预设覆盖、删除。只在明确知道用途时启用。`
  - 预设模板管理页的同一提示措辞不同：`导入为系统模板后，这些模板会被标记成系统预设，并可能在后续的系统预设同步或重置时被内置预设覆盖、删除。只在明确知道用途时启用。`
- 底部按钮：`取消`（导入中禁用）、`导入`（primary，导入中禁用 + spinner）
- 导入中的弹窗与遮罩都禁止关闭（`DisableClose` / `CloseOnBackdrop`)
- 结果 Toast：
  - 内容为空 → Warning `请先粘贴模板 JSON 内容。`
  - 成功 → Success `已导入 {N} 个模板` 或 `已导入 {N} 个模板（作为系统预设）`，随后关闭并清空、重新加载
  - 解析出 0 条 → Warning `没有导入任何模板：JSON 里没有含 name 与 commandArgs 的模板条目。`
  - 异常（含 `InvalidOperationException`）→ Error `导入失败：{异常消息}`

**初始化时的同步失败**：`同步系统预设模板失败：{异常消息}`（Error Toast），并且不显示同步提示条。

**跨页跳转**：`/convert?template={id}`（选择）、`/templates-admin`（删除系统预设时）、以及卡片上的编辑/详情弹窗。

### 4.6 预设模板管理 `/templates-admin`（TemplatesAdmin）

来源：`Components/Pages/TemplatesAdmin.razor:1-1103`

**页头**
- 标题区：`🔧 预设模板管理`（h1）+ 副标题 `系统预设模板的完整增删改查、预设重置与运行诊断。`
- 右侧徽标：`⚠️ 高级页面`（warning）

**顶部警告条（`role="alert"`，逐字）**
> 这个页面可以直接修改**系统预设模板**，改动会影响「转换」与「批量处理」里的选项。
>
> 应用默认只监听 127.0.0.1（仅本机可访问），因此没有额外的登录校验；如果把服务暴露到局域网，任何人都可以修改这里的模板。

**操作按钮行**
| 按钮 | 禁用条件 | 行为 |
| --- | --- | --- |
| `➕ 新建` | `IsBusy` | 新建**系统预设**（`Type=System`、`HardwareMode=None`、`SortOrder = 最大值+10`，无模板时为 10） |
| `⬇️ 导出全部` | `IsBusy` 或 无模板 | 导出全部（含系统与用户） |
| `📥 导入` | `IsBusy` | 打开导入弹窗（与模板页几乎相同，但复选框文案为 `导入为系统模板`、成功文案为 `已导入 {N} 个模板（作为系统预设）` 或 `已导入 {N} 个用户模板`） |
| `🔄 刷新` | `IsBusy` | 重新加载模板列表 + 诊断信息 |
- `IsBusy = 导出中 || 导入中 || 删除中 || 重置中 || 清理中`

**筛选工具栏**
| 控件 | 标签 | 类型 | 默认 |
| --- | --- | --- | --- |
| 搜索 | `搜索` | search（`aria-label="搜索模板"`） | 空；占位 `搜索名称、描述、分类、编码器…`；`oninput` + `after` 即时过滤（无防抖） |
| 类型 | `类型` | select（`aria-label="按类型筛选"`） | `全部` / `系统` / `用户` |
| 清除筛选 | 按钮 `清除筛选` | — | 仅当搜索非空或类型非 `全部` 时可用 |

- 结果计数：`显示 {可见数} 个模板（共 {总数} 个：系统 {N} · 用户 {M}）`
- 搜索匹配：`Name`、`Description`、`Category`、`OutputExtension`、`RequiredEncoder`、`Tags`（忽略大小写）

**模板表格面板（标题 `模板列表`）**
- 加载中：loading 行 `正在加载模板…`（`aria-busy="true"`）
- 空状态：
  - 完全没有模板：`数据库里还没有任何模板。可以点「新建」创建一个，或点「导入」导入 JSON。`
  - 筛选无结果：`没有符合当前筛选条件的模板。` + 按钮 `清除筛选`
- 表头（每列标题都是一个排序按钮，带 `aria-sort` 与指示符 `↕`/`▲`/`▼`）：
  `名称` / `分类` / `类型` / `输出格式` / `编码器` / `排序` / `更新时间` / `操作`
- 排序规则与默认方向（`749-761`）：
  - 默认列 `排序`（`SortOrder`）升序
  - `名称`/`分类`/`编码器` 首次点击默认**升序**，`类型`/`输出格式`/`排序`/`更新时间` 首次点击默认**降序**
  - 再次点击同列切换升/降；始终以名称（当前文化）作为次级排序键
- 单元格内容：
  | 列 | 内容 |
  | --- | --- |
  | 名称 | 模板名 + （推荐时）徽标 `推荐`（primary） |
  | 分类 | 分类名，空 → `未分类` |
  | 类型 | 徽标 `系统`（info）/ `用户`（success） |
  | 输出格式 | 等宽 `.{扩展名}`，空 → `-` |
  | 编码器 | `EncoderDisplay`：策略为「不使用硬件加速」→ `不使用硬件加速`；否则 `{必须\|自动}：{族显示名}`（未指定编码器时为 `任意可用编码器`） |
  | 排序 | 数字（`font-variant-numeric: tabular-nums`） |
  | 更新时间 | `yyyy-MM-dd HH:mm`（本地时间） |
  | 操作 | `✏️ 编辑`（`IsBusy` 时禁用）、`🗑️ 删除`（danger，`IsBusy` 时禁用） |

**运行诊断面板（标题 `运行诊断`）**
| 键 | 值 |
| --- | --- |
| `数据目录` | 等宽路径 + 徽标 `可写`（success）/ `不可写`（danger） |
| `模板数量` | `{N} 个（系统 {S} · 用户 {U}）` |
| `任务队列` | `运行中 {A} · 等待中 {Q} · 历史记录 {H}` |
| `历史清理` | 内联输入 `保留最近 [数字] 天`（`aria-label="历史保留天数"`，min 1 / max 3650，**默认 30**）+ 按钮 `🧹 清理历史`（清理中 spinner 并禁用） |
| `FFmpeg` | 查询中 → `查询中…`；可用 → 版本号；不可用 → 徽标 `不可用`（danger）；下一行等宽 `FFmpegPath` |
| `预设版本` | `v{PresetVersion}` |
- 清理历史行为：天数 clamp 到 1–3650；成功 → Success `已清理 {N} 条早于 {天} 天的历史任务记录（运行中/等待中的任务不受影响）` 或 `没有早于 {天} 天的历史任务记录需要清理`；异常 → Error `清理历史任务失败：{异常消息}`；成功后刷新诊断。
- 诊断读取失败 → Warning `读取运行状态失败：{异常消息}` / `无法确认数据目录是否可写：{异常消息}`

**危险区域（`<details>`，标题 `⚠️ 危险区域`，danger 配色）**
- 小节标题 `重置系统预设模板`，说明（逐字）：
  `删除当前数据库里的**全部系统预设模板**，然后按内置预设（v{PresetVersion}）重新写入。**用户自己创建的模板会保留**，但对系统预设做过的所有手工修改都会丢失。`
- 按钮 `🔄 重置系统模板`（danger，`IsBusy` 时禁用）
- 确认框：标题 `重置系统预设模板`，正文 `确定要删除所有系统预设模板并重新写入内置预设吗？`，Detail `用户自己创建的模板会被保留；对系统预设模板做过的所有手工修改都会丢失，且无法撤销。`，确认按钮 `确认重置`
- 成功 → Success `系统预设模板已重置到 v{版本}（用户自建模板未受影响）`；异常 → Error `重置系统预设失败：{异常消息}`

**删除确认框**
- 正文：`确定要删除「{名称}」吗？`
- Detail：系统模板 → `这是系统预设模板：删除后，只要它仍然存在于内置预设列表中，下次系统预设同步（应用升级或重置）时它还会重新出现。`；用户模板 → `这是用户自建模板：删除只影响模板本身，已经用它执行过的任务记录不受影响。`
- 确认按钮 `删除`；忙碌时禁用 + spinner（`IsBusy`）
- 成功 Toast：
  - 系统：`已删除系统预设「{名称}」；只要它仍在内置预设列表中，下次预设同步或重置时就会重新出现。`
  - 用户：`已删除用户模板「{名称}」`
- 服务返回未删除 → Error `删除失败：服务返回未删除「{名称}」，它可能已经不存在，或类型不匹配。`；异常 → Error `删除模板失败：{异常消息}`

**实时行为**：订阅 `TaskStatusChanged` 事件，仅触发一次重渲染（用来刷新诊断面板里的运行中/等待中计数）。

**跨页跳转**：无（全部在页内完成）。

### 4.7 设置 `/settings`（Settings）

来源：`Components/Pages/Settings.razor:1-1629`
截图：![设置](../../../FFmpegWebUI/readmefiles/setting.png)

**页头**
- `⚙️ 设置`（h1）
- 页面说明：`所有设置都保存在本机数据目录的数据库里，不会上传；修改后需要点击页面底部的「保存设置」才会写入。`
- 整页最大宽度 **1020px**（`.settings-page { max-width: 1020px }`）。
- 未就绪：loading 行 `正在读取设置…`

#### 面板 1 · 🩺 环境信息
- 标题 `🩺 环境信息`（检测中前置 spinner）
- 工具栏：按钮 `重新检测`（检测中变 `检测中…` + spinner 并禁用）+ 说明 `重新读取 FFmpeg、ffprobe、硬件解码能力与数据目录状态（可能需要几秒）。`
- 状态未就绪 → loading 行 `正在检测环境…`
- **键值网格**（两列 `max-content` + `1fr`）：
  | 键 | 值 |
  | --- | --- |
  | `应用版本` | `v{AppVersion}` |
  | `平台` | 平台显示名 |
  | `.NET 版本` | 运行时版本 |
  | `数据目录` | 等宽路径 + 徽标 `可写`（success）/ `不可写`（danger）；不可写时追加 `设置与任务历史可能无法保存，请检查目录权限（可用环境变量 FFMPEGWEBUI_DATA_DIR 换一个位置）。` |
  | `FFmpeg` | 可用 → 绿徽标 `可用` + 版本；不可用 → 红徽标 `未找到` + `转换功能不可用，请先安装 FFmpeg（见下方指引）。` |
  | `FFmpeg 路径` | 等宽路径 + 徽标 `自动探测`（默认色）/ `来自设置`（info） |
  | `ffprobe 路径` | 等宽路径；FFmpeg 不可用时追加 `FFmpeg 不可用时无法确认是否真的存在。` |
  | `硬件解码方式` | 无 → `当前 FFmpeg 未报告可用的硬件解码方式。`；有 → 逐项 info 徽标 |
- **通知区**：
  - 无通知 → 绿条 `✅ 一切正常：FFmpeg 可用，未发现需要注意的问题。`
  - 有通知 → 逐条：FFmpeg 可用时黄条（`⚠️` 前缀），不可用时红条（`⛔` 前缀），内容为通知原文
- **FFmpeg 安装指引**（仅 FFmpeg 不可用时）——标题 `安装 FFmpeg（{平台}）`，分平台有序列表（逐字）：
  - **Windows**
    1. `下载 FFmpeg 完整构建包（如 gyan.dev 或 BtbN 的 release），解压到 C:\ffmpeg。`
    2. `把 C:\ffmpeg\bin 加入系统 PATH，或直接在下面的「FFmpeg 可执行文件」里填写 ffmpeg.exe 的完整路径。`
    3. `也可以用包管理器：winget install Gyan.FFmpeg / choco install ffmpeg / scoop install ffmpeg。`
    4. `改完 PATH 需要重启本应用，再点上面的「重新检测」。`
  - **macOS**
    1. `推荐 Homebrew：brew install ffmpeg（会自动带上 ffprobe）。`
    2. `Apple 芯片装到 /opt/homebrew/bin，Intel 机器装到 /usr/local/bin。`
    3. `装完点上面的「重新检测」；若仍未找到，可在下面直接指定完整路径。`
  - **Linux**
    1. `Debian/Ubuntu：sudo apt install ffmpeg`
    2. `Fedora/RHEL：sudo dnf install ffmpeg`
    3. `Arch：sudo pacman -S ffmpeg`
    4. `装完点上面的「重新检测」；若用 Docker，请确认容器镜像里也装了 FFmpeg。`
  - **查找顺序说明**：`会按顺序在这些位置查找：`
    - `设置里填写的路径（优先）`
    - `应用所在目录下的内置副本：ffmpeg{.exe}、ffmpeg/ffmpeg{.exe}、tools/ffmpeg/ffmpeg{.exe}`
    - Windows：`系统 PATH`、`C:\ffmpeg\bin\ffmpeg.exe`、`C:\ProgramData\chocolatey\bin\ffmpeg.exe`、`%LOCALAPPDATA%\Microsoft\WinGet\Links\ffmpeg.exe`、`%USERPROFILE%\scoop\shims\ffmpeg.exe`
    - macOS：`系统 PATH`、`/opt/homebrew/bin/ffmpeg`（Apple 芯片 Homebrew）、`/usr/local/bin/ffmpeg`、`/opt/local/bin/ffmpeg`（Intel Homebrew / MacPorts）
    - Linux：`系统 PATH`、`/usr/bin/ffmpeg`、`/usr/local/bin/ffmpeg`、`/usr/lib/jellyfin-ffmpeg/ffmpeg`（对 QSV/VAAPI 支持更完整）、`/snap/bin/ffmpeg`、`/var/lib/flatpak/exports/bin/ffmpeg`

#### 面板 2 · 🚀 硬件加速能力
- 标题 `🚀 硬件加速能力` + 徽标 `{N} 个可用硬件编码器`（有硬件 success / 无硬件 warning；仅当报告已加载且非扫描中）
- 工具栏：按钮 `重新检测硬件编码器`（扫描中变 `检测中…` + spinner 并禁用）；扫描中显示 `已用时 {秒 0.0} 秒，请勿刷新页面…`，否则显示 `上次检测：yyyy-MM-dd HH:mm:ss`
- 说明：`检测会逐个真实试跑本机的硬件编码器（每个约 1~3 秒），通常需要几秒到十几秒，期间可以继续操作其它设置。`
- 报告未就绪 → loading 行 `正在读取硬件编码器列表…`
- 报告就绪：
  - 有 `Summary` → 信息条 `ℹ️` + Summary + 换行弱化 `检测时间 {时间}`（有版本时追加 ` · FFmpeg {版本}`）
  - **`首选硬件编码器`** select（`id="preferred-encoder"`）：
    - 选项 1：`自动（按编码格式选择，推荐）`（值 `""`）
    - 之后列出可用硬件编码器：`{显示名}（{编码器名}）`
    - 若已保存的编码器当前不可用，追加选项 `{编码器名}（当前不可用）`
    - `disabled = 无可用硬件编码器`
    - 帮助文案：
      - 有硬件：`只影响「优先使用硬件编码」的模板；留空时由应用按 h264/hevc/av1 等编码格式自动挑选。`
      - 无硬件：`本机没有可用的硬件编码器（或检测尚未完成），因此暂不能指定。转换会使用 CPU 软件编码。`
    - 已保存的编码器不可用时额外显示红字 `⚠ 已保存的首选编码器「{名字}」当前不可用（可能驱动未安装或 FFmpeg 未编译该编码器）。` + 按钮 `改为「自动」`（清空该字段）
- **硬件加速开关**（小标题 `硬件加速开关`，三项复选框，每个都是「勾选框 + 粗体标签 + 说明」）：
  | 标签 | 说明 |
  | --- | --- |
  | `优先使用硬件编码（GPU 编码）` | `模板没有指定编码器时，自动选择本机可用的硬件编码器（NVENC / QSV / AMF / VideoToolbox / VA-API 等），速度更快、CPU 占用更低。` |
  | `优先使用硬件解码（输入侧）` | `硬件解码可以减轻 CPU 负担，但个别老驱动可能不稳定；如果转换出现花屏、卡住或崩溃，请关闭此项。` |
  | `硬件编码失败时自动改用软件编码重试` | `硬件编码失败时自动改用软件编码重试，避免任务直接失败。只有在 FFmpeg 还没有写出有效数据时才会回退，因此不会产出损坏的文件。` |
- 无可用硬件且编码器表非空 → 黄条 `⚠️ 未检测到可用的硬件编码器，转换会使用 CPU 软件编码（较慢但兼容性最好）。`
- **编码器分组展示**：按族分组（组标题 = 族显示名，无族归入 `其他编码器`），标题右侧徽标 `{可用数} / {总数} 可用`（可用数 > 0 时 success）；组内为 `EncoderCard` 网格（`repeat(auto-fill, minmax(250px,1fr))`）。
  - 排序：先按「可用数 > 0」降序，再按族固定顺序（nvenc 0 / qsv 1 / amf 2 / videotoolbox 3 / vaapi 4 / vulkan 5 / mf 6 / software 7 / 其它 8），最后按组标签；组内先按可用降序再按显示名。
- 扫描结果 Toast（仅用户主动扫描时）：
  - 成功 → Success `硬件检测完成：{N} 个可用硬件编码器（耗时 {秒 0.0} 秒）`；若 0 个再追加 Warning `本机没有检测到可用的硬件编码器，转换会使用 CPU 软件编码。`
  - 异常 → Error `硬件编码器检测失败：{异常消息}`
- **扫描期间每 0.4 秒刷新一次已用时长**（`1062-1106`）。

#### 面板 3 · 📁 FFmpeg 路径
- `FileSelector Label="FFmpeg 可执行文件"`：占位 `留空则自动探测（系统 PATH / 常见安装位置 / 内置副本）`、`ValidateExists=false`、`ShowFileInfo=true`
  - 下方解析行：`当前实际使用：` + 等宽路径（未解析 → `尚未解析到 ffmpeg`；检测中 → `检测中…`）+ 弱化 `（保存或「重新检测」后更新）`
  - 按钮 `测试 FFmpeg`（测试中变 `测试中…` + spinner），结果以 `test-result ok|bad` 文本显示
  - 帮助：`示例：C:\ffmpeg\bin\ffmpeg.exe 或 /opt/homebrew/bin/ffmpeg。留空时会测试系统 PATH 中的 ffmpeg。`
  - **测试结果文案（逐字）**：
    | 情况 | 文案 |
    | --- | --- |
    | 能拿到版本 | `✓ 可用（{路径「X」或 系统 PATH}）：{版本}` + Success `FFmpeg 可用` |
    | 能执行但读不到版本 | `✗ 能执行但读不到版本信息（{来源}）：ffmpeg -version 没有返回可识别的内容，请确认它真的是 FFmpeg。` |
    | 有自定义路径但不可用 | `✗ 不可用：无法执行「{路径}」（文件不存在、没有执行权限，或不是 FFmpeg 可执行文件）。` |
    | 未填路径且 PATH 中找不到 | `✗ 不可用：系统 PATH 中没有找到可执行的 ffmpeg，请在上面填写完整路径。` |
    | 抛异常 | `✗ 测试出错：{异常消息}` |
    - 失败统一 Toast：Error `FFmpeg 测试未通过`；异常 → Error `测试 FFmpeg 出错：{异常消息}`
  - **结果过期规则**：只有当前输入框内容与上次测试的路径一致时才显示结果（一改输入即作废）。
- 分隔线后 `FileSelector Label="ffprobe 可执行文件"`：占位 `留空则自动探测（通常与 ffmpeg 同目录）`、`ValidateExists=false`、`ShowFileInfo=true`
  - 同样有解析行与按钮 `测试 ffprobe`（检查中变 `检查中…`）
  - 帮助（逐字）：`读取输入文件的时长、分辨率等媒体信息需要 ffprobe（用 ffmpeg 本身无法可靠地拿到）。这里只能检查「文件是否存在且可读 / 是否在 PATH 中」，不会真的执行 ffprobe —— 如需确认真实可用，请用上面的「测试 FFmpeg」确认版本信息能正常解析。`
  - 测试结果：
    | 情况 | 文案 |
    | --- | --- |
    | 文件存在且可读 | `✓ 文件存在且可读：{路径}（只检查了文件是否可访问，没有真的执行 ffprobe）` |
    | 在 PATH 中找到 | `✓ 在系统 PATH 中找到：{路径}（只检查了文件是否存在，没有真的执行 ffprobe）` |
    | 均未找到 | `✗ 未找到可用的 ffprobe：{「X」不是可读取的文件 \| 输入框为空，应用也还没有解析出 ffprobe 路径}，系统 PATH 中也没有 {查找名}。（本次只做了文件检查，没有执行 ffprobe；ffprobe 通常与 ffmpeg 一起安装，可填写 ffmpeg 同目录下的 ffprobe{.exe}）` |
    | 抛异常 | `✗ 检查出错：{异常消息}` + Error `测试 ffprobe 出错：{异常消息}` |

#### 面板 4 · 📂 输出设置
| 字段 | 控件 | 默认 | 说明 / 校验 |
| --- | --- | --- | --- |
| `默认输出目录` | `FileSelector`（`IsDirectory=true`、`ValidateExists=false`） | 空 | 占位 `例如 /Users/you/Videos 或 D:\Output`；帮助 `新建转换/批量任务时预填的输出目录；留空时转换页面会要求你手动选择。保存时会检查这个目录能否创建并写入，不可写则拒绝保存。`；不可写时红字 `✗ {错误文案}`，保存时错误为 `不可写或无法创建：{目录}。请换一个目录，或先创建它并授予写权限。` + Error Toast `默认输出目录不可写，设置未保存。` |
| `输出文件命名规则` | select | `添加后缀` | 选项 `保持原名` / `添加后缀`；帮助 `示例：{预览}` |
| `输出文件后缀` | text（`oninput`） | `_converted` | 仅在命名规则为「添加后缀」时显示；占位 `_converted`；帮助 `常用值：_converted、_out、-1080p。预览：{预览}`；校验：为空 → 红字 `✗ 选择「添加后缀」时后缀不能为空，否则输出文件会与原文件同名。`；含非法字符 → 红字 `✗ 后缀包含文件名不允许的字符：{字符}（不能包含 / \ : * ? " < > \| 等）` |
| `文件已存在时的处理方式` | select | `询问用户` | 选项 `询问用户` / `覆盖` / `跳过` / `重命名`；帮助文案见下表 |

- **命名规则预览文案**：
  - 保持原名：`video.mp4 → video.mp4（保持原名）`
  - 添加后缀：`video.mp4 → video{后缀}.mp4`
- **「文件已存在时的处理方式」帮助文案（逐字）**：
  | 选项 | 文案 |
  | --- | --- |
  | `询问用户` | `转换前在界面询问你；批量处理时自动改为重命名，避免卡住等你确认。` |
  | `覆盖` | `直接覆盖同名文件（已有输出的内容会丢失）。命令里会自动加 -y，不会等待确认。` |
  | `跳过` | `保留已有文件、跳过该文件的转换，任务会标记为「已跳过」。` |
  | `重命名` | `自动换成不冲突的新名字（如 video (1).mp4），原文件和已有输出都保留。` |
  | 其它 | `未知的处理方式，实际会按「重命名」执行。` |

#### 面板 5 · ⚡ 性能与数据
| 字段 | 控件 | 默认 | 说明 |
| --- | --- | --- | --- |
| `同时执行的转换任务数` | select 1–8 | `1` | 帮助：`默认 1（串行）。大于 1 会同时跑多个 FFmpeg 进程：显卡/CPU 的编码器通常无法真正并行，多个任务容易互相抢占资源，反而每个都变慢；只有在批量处理大量小文件时可能略快。` |
| `单个任务保留的日志行数` | number（min 50 / max 20000 / step 50） | `500` | 帮助：`允许范围 50 ~ 20000；超出后丢弃最早的行，数值越大占用内存越多。` |
| `任务历史保留天数` | number（min 0 / max 3650） | `30` | 帮助：`应用每次启动时按这个天数清理一次历史记录（0 表示启动时不自动清理）。` |

- 按钮栏：
  - `立即清理历史`（清理中变 `清理中…` + spinner；`disabled = 清理中 || 保留天数 <= 0`）
    - 保留天数为 0 时点击 → Warning `保留天数为 0，没有可清理的时间范围；请先填写天数，或使用「清空全部历史」。`
    - 成功 → Success `已清理 {N} 条历史记录` 或 `没有需要清理的历史记录`；并把内联信息设为 `已清理 {时间点} 之前的记录：删除 {N} 条（保留天数 {天} 天）。`
    - 异常 → Error `清理历史失败：{异常消息}`
  - `清空全部历史`（danger；清空中禁用）→ 确认框：标题 `清空全部历史`、正文 `确定删除所有已结束的任务记录吗？`、Detail `删除后无法恢复；正在运行和排队中的任务会保留。`、确认按钮 `清空历史`
    - 成功 → Success `已清空 {N} 条历史记录` + 内联信息 `已清空历史记录，共删除 {N} 条；运行中和等待中的任务已保留。`；异常 → Error `清空历史失败：{异常消息}`
  - 保留天数为 0 时的弱化提示：`保留天数填的是 0（不按天数清理），要清空请用「清空全部历史」。`
- 说明：`「立即清理历史」按上面填写的天数执行一次（使用当前输入值，不需要先保存设置）。`
- 内联结果条（有值时）：`ffui-alert-info` + `{历史消息}`
- **高级折叠区**（`<details class="advanced">`，标题 `全局 FFmpeg 参数（高级）`）：
  - `附加到每条 FFmpeg 命令的参数`（text，占位 `-threads 4`）
  - 帮助：`留空表示不追加。参数会被插入到输出文件之前，对每一次转换都生效。`
  - 警告条：`这些参数会附加到每一条命令，写错会让所有转换都失败；请先确认语法。不要在这里写 -y / -n，覆盖策略由「文件已存在时的处理方式」自动处理。`

#### 面板 6 · 🎨 界面与通知
| 字段 | 控件 | 默认 | 说明 |
| --- | --- | --- | --- |
| `主题` | select | `跟随系统` | 选项 `跟随系统` / `浅色` / `深色`；帮助 `切换后立即生效并自动记住，下次打开时保持（不需要点「保存设置」，也不需要刷新页面）。`（见 §2.3） |
| `任务完成后发送浏览器通知` | checkbox | 勾选（默认 `true`） | 帮助 `勾选时浏览器会弹出权限请求；如果权限被拒绝或在设置里被关闭，通知不会出现（可在浏览器地址栏的站点设置里重新允许）。`；勾选时立即请求权限：授予 → Success `浏览器已允许通知：任务完成后会弹出提醒。`；拒绝 → Warning `浏览器没有授予通知权限：勾选会保留，但转换完成时不会弹出系统通知。可在浏览器地址栏的站点设置里重新允许。`；异常 → Error `请求通知权限失败：{异常消息}` |
| `监听端口` | number（min 0 / max 65535） | `5265` | 帮助 `修改后需要**重启应用**才会生效；0 表示自动挑选一个空闲端口。实际端口的优先级为：命令行 --port > 环境变量 FFMPEGWEBUI_PORT > appsettings.json 的 FFmpegWebUI:ListenPort > 这里保存的值。只要上面几项都没有显式设置，这里改的就是实际使用的端口。` |

#### 底部行动栏（`role="group" aria-label="设置操作"`，`position:sticky; bottom:0`，带阴影）
- 顶部错误提示（二选一）：
  - 设置读取失败 → 红条 `⛔ 设置读取失败，为避免覆盖已有配置，保存已禁用。请先点击「重新读取设置」。`
  - 校验错误（> 0 条）→ 红条 `⚠️ 有 {N} 项需要修正，保存已禁用：` + 逐条 `<li>`
- 按钮与状态：
  | 元素 | 文案 | 条件 |
  | --- | --- | --- |
  | 保存 | `💾 保存设置`（lg、primary；保存中 spinner + `保存中…`） | `disabled = !CanSave` |
  | 恢复默认 | `🔄 恢复默认`（lg；重置中 spinner） | `disabled = 保存中 \|\| 重置中` |
  | 重新读取 | `重新读取设置` | 仅读取失败时显示 |
  | 状态徽标 | `● 有未保存的更改`（warning）/ `✓ 已保存`（success） | 非读取失败时显示 |
  | 说明 | `设置只写在本机数据目录，不会上传。` | 总是 |
- `CanSave = 已就绪 && !读取失败 && !保存中 && 校验错误为 0`
- **校验错误全清单（逐字，前缀为字段名）**：
  1. `输出文件后缀：选择「添加后缀」时不能为空。`
  2. `输出文件后缀：包含文件名不允许的字符「{字符}」。`
  3. `同时执行的转换任务数：必须在 1 ~ 8 之间。`
  4. `单个任务保留的日志行数：必须在 50 ~ 20000 之间。`
  5. `任务历史保留天数：必须在 0 ~ 3650 之间。`
  6. `监听端口：必须在 0 ~ 65535 之间（0 表示自动选择空闲端口）。`
  7. `主题：取值无效，请重新选择。`
  8. `默认输出目录：{目录错误}`（目录不可写时）
- 保存流程：先单独校验默认输出目录可写性（会尝试创建并写测试文件），不可写则中止并报错；保存成功后**以服务端规范化后的设置为准重新同步表单**（避免界面与实际不一致）；成功 → Success `设置已保存`；异常 → Error `保存失败：{异常消息}`
- 恢复默认确认框：标题 `恢复默认设置`、正文 `确定把全部设置恢复为默认值吗？`、Detail `FFmpeg 路径、输出规则、硬件加速、性能与界面设置都会被覆盖；任务历史不受影响。`、确认按钮 `恢复默认`（危险）
  - 成功 → Success `已恢复默认设置`（并清空路径测试结果、目录错误、历史消息，重新应用主题）；异常 → Error `恢复默认失败：{异常消息}`
- 重新读取成功 → Success `设置已重新读取`

**实时/副作用行为**
- 订阅 `SettingsChanged`：设置变化后若「FFmpeg/ffprobe 路径可能变了」则强制重扫环境（路径留空且原本是自动探测时不重扫），否则只做缓存刷新。避免「改主题也去试跑编码器」（`970-1001`）。
- 渲染失败只提示一次：Warning `界面刷新失败（页面可能已关闭）：{异常消息}`

### 4.8 错误页 / 404 页

**`/Error`（`Components/Pages/Error.razor:1-56`）**
- 标题 `😵 应用遇到了一个错误`（h1）
- 正文 `这次请求没有正常完成。多数情况下重新加载页面即可恢复；如果反复出现，请把下面的信息反馈给开发者。`
- 请求 ID 行（有值时）：`请求 ID：` + 等宽 `<code>{RequestId}</code>`
- `<details>` 标题 `排查建议`，列表：
  1. `确认「设置」里的 FFmpeg 路径指向真实可执行文件（可用「测试 FFmpeg」按钮验证）。`
  2. `确认输出目录存在且可写。`
  3. `数据目录（{DataDirectory}）需要有读写权限；如果被占用或损坏，可删除其中的 data.db 重新初始化。`
  4. `查看运行此应用的终端窗口，那里有更详细的错误日志。`
- 操作按钮：`返回首页`（primary → `/`）、`打开设置`（→ `/settings`）

**`/not-found`（`Components/Pages/NotFound.razor:1-21`）**
- 标题 `404 — 页面不存在`（h1）
- 正文 `你访问的地址没有对应的页面，可能是链接已失效或输入有误。`
- 按钮：`返回首页`（primary）、`开始转换`（→ `/convert`）
- 该组件同时被 `Router NotFoundPage` 使用，因此**任意未匹配路由都会渲染它**（浏览器地址不变）。

### 4.9 全站「打开输出目录」行为（统一规格）

来源：`Home.razor:722-754`、`Convert.razor:1159-1189`、`Tasks.razor:937-969`、`BatchProcess.razor:448-478`

- 取 `OutputPath` 的目录名；为空或不存在 → Warning `输出目录不存在，可能已被移动或删除。`（不再执行任何进程调用）
- 平台命令：Windows `explorer.exe "目录"`（`UseShellExecute=true`）；macOS `open 目录`；Linux `xdg-open 目录`
- 成功（仅 Home / Tasks）→ Success `已在文件管理器中打开输出目录。`
- 异常 → Error `无法打开目录：{异常消息}`（Tasks 页为 `无法打开输出目录：{异常消息}`）

### 4.10 全站状态映射总表（唯一真相）

| 内部状态 | 中文 | 徽标类 | 图标 | 左侧竖条色 | 进度条填充 |
| --- | --- | --- | --- | --- | --- |
| `Pending` | `等待中` | 默认（无色） | `⏸️` | `--border-strong` | `--border-strong` |
| `Running` | `运行中` / 批量文件列表用 `进行中` | `ffui-badge-info` | `⏳` | `--info-color` | `--primary-color` |
| `Completed` | `已完成` | `ffui-badge-success` | `✅` | `--success-color` | `--success-color` |
| `Failed` | `失败` | `ffui-badge-danger` | `❌` | `--danger-color` | `--danger-color` |
| `Cancelled` | `已取消` | `ffui-badge-warning` | `⏹️` | `--warning-color` | `--warning-color` |
| `Skipped` | `已跳过` | `ffui-badge-warning` | `⏭️` | `--warning-color` | `--warning-color` |

> ⚠️ 注意「运行中」在 `ProgressBar` / Tasks / Convert / Home 页显示为 `运行中`，但**批量文件列表 `BatchFileList` 显示为 `进行中`**（`BatchFileList.razor:452`），`BatchProgressPanel` 用 `进行中`（`BatchProgressPanel.razor:282`）。React 版必须保留这处不一致，否则与现有视觉不符。

### 4.11 全站格式化规则总表

| 数据 | 规则 | 示例 |
| --- | --- | --- |
| 文件大小 | `bytes <= 0 → "0 B"`；否则按 1024 进位到 B/KB/MB/GB/TB/PB，保留 **最多 2 位小数**（`0.##`） | `0 B`、`1.5 KB`、`2.25 MB`、`1.75 GB` |
| 百分比（进度条） | `clamp(0,100)`，不变文化，固定 **1 位小数** + `%` | `85.5%` |
| 百分比（批量汇总/任务行） | 批量总进度 `0.##`；任务行 `0.#` | `33.33%`、`12.5%` |
| 速度 | `0.##` + `x` | `1.23x` |
| 时长（秒 → 文本） | `<= 0 → "-"`；≥1 小时 `H:MM:SS`（小时不补零、分秒补零）；否则 `MM:SS` | `-`、`03:07`、`1:02:59` |
| 检测耗时 | `< 60 秒 → "{0.0} 秒"`；`>= 60 秒 → "m:ss"` | `3.4 秒`、`1:12` |
| 试运行耗时 | `<= 0 → "—"`；否则 `"{0.0} 秒"` | `2.1 秒` |
| 日期时间（详情） | 本地时间 `yyyy-MM-dd HH:mm:ss` | `2025-09-28 14:35:02` |
| 日期时间（列表） | 本地时间 `MM-dd HH:mm` | `09-28 14:35` |
| 日期时间（模板管理表格） | 本地时间 `yyyy-MM-dd HH:mm` | `2025-09-28 14:35` |
| 日期时间（检测标记） | 本地时间 `yyyy-MM-dd HH:mm` | `2025-09-28 14:35` |
| 空值占位 | `—`（U+2014）用于路径/模板名等；文件大小空值用 `0 B`；耗时用 `-`（半角连字符） | — |
| 文件名展示 | 长路径一律「只显示文件名 + CSS 省略号 + `title` 全文」 | — |

---

## 5. 专题：命令预览（Command Preview）

命令预览在本项目中有**四个入口**，行为与刷新时机各不相同，必须逐个实现。全站统一格式：整行命令 = `ffmpeg ` + `BuildCommand(...).CommandLine`（**注意前缀是半角空格**）。

### 5.1 转换页「完整命令」区块

来源：`Components/Pages/Convert.razor:238-251, 825-915, 1002-1026`

- **展示位置**：面板 `4 · 确认并开始` 内；标题文案 `完整命令（可直接复制到终端执行）`。
- **展示内容**：`<pre class="command-code">ffmpeg {预览命令}</pre>`，等宽 0.82rem、行高 1.55。
  - 默认 **自动换行**（`white-space:pre-wrap; word-break:break-all`），按钮文案为 `不换行`。
  - 切到不换行后：`white-space:pre; overflow-x:auto`，按钮文案为 `自动换行`。
  - 显示开关是纯本地 UI 状态，不触发任何服务调用。
- **刷新时机**（每次重建都会先清空旧值与警告）：
  1. 输入路径变化（`OnParametersSetAsync` 中检测 `_lastProbedPath`）
  2. 输出目录变化
  3. 输出文件名变化
  4. 选中模板变化（`OnTemplateSelectedAsync` → `RebuildPreviewAsync`）
  5. 模板参数值变化（`OnParameterValuesChanged` → 异步重建）
  6. 点击 `重置参数` 后
  7. 点击冲突提示里的 `覆盖已有文件` / `保留两者（自动改名）` 后（会改变 `FileExistsAction` 的取值再重算）
  - 前置条件：已选模板 + 输入路径非空 + 输出目录非空；否则预览为空字符串且不显示。
- **预览的解析过程（决定用户看到什么）**：
  1. 生成「请求的输出路径」（输出文件名非空时按命名模板展开，否则 `输入文件名 + 后缀` + `.扩展名`）
  2. 按「文件已存在时的处理方式」（或强制覆盖）调用 `OutputPathResolver.Resolve`，得到最终输出路径与「跳过/改名/覆盖」标记
  3. 若文件已存在（且未强制覆盖）→ 生成冲突提示文案（见 §4.2）
  4. 若「输出路径 == 输入文件」→ 覆盖为更准确的提示：`输出路径与输入文件相同：已自动改名，避免把源文件覆盖掉。（把输出目录换成别的目录、或让输出文件名带上后缀，可以避免改名。）`
  5. 组装 `CommandContext`：`InputPath`、`OutputPath`（解析后）、`Parameters`、`Media`（媒体信息）、`AudioEncoder`（按命令里的 `libopus`/`libmp3lame`/`flac`/`aac` 或扩展名推断：webm/ogg→libopus、mp3→libmp3lame、flac→flac、其它→aac）
  6. 若命令含 `{encoder}` 或模板硬件模式非 None → 解析推荐编码器；**严格模式（Required）拿不到编码器时**标记模板不可运行并追加警告，并禁用「开始转换」
  7. `BuildCommand` 返回命令行与警告列表；异常时预览内容被替换为单条警告 `生成命令预览失败：{异常消息}`
- **复制行为（`CopyCommandAsync`）**
  - 复制内容 = `"ffmpeg " + _previewCommand`
  - 成功 → 按钮文案 2 秒内变为 `✓ 已复制`，并 Success Toast `命令已复制到剪贴板`
  - 剪贴板 API 返回 false → Error `复制失败：浏览器不允许访问剪贴板。请手动选中命令文本后复制。`
  - （此入口未单独捕获 JS 异常；若 `copyText` 抛异常会冒泡到 Blazor 错误处理）

### 5.2 模板详情弹窗内的「命令预览」

来源：`Components/Shared/TemplateDetailViewer.razor:66-112, 609-648`

- 标题 `命令预览`，紧随其后的提示：`用占位符拼出的实际命令行，参数改动会立即反映在这里`
- 占位路径固定为 `<输入文件>` / `<输出文件>`（尖括号是字面文本）
- 参数取值：优先用父组件传入的 `ParameterValues`，否则用 `CreateDefaultParameterValues(template)`
- **刷新时机**：打开详情时、`重新读取` 后、**任意参数控件改动后**（`OnValuesChanged` → `RebuildPreviewAsync(resolveEncoder:false)`）
- 编码器画像只在打开/重新读取时解析（严格模式下拿不到编码器则不展示替代编码器，避免展示错误的 `{quality_args}`）
- 三种展示态：错误红条 `命令预览生成失败：{异常消息}` / loading `正在生成命令预览…` / 主色描边代码框 + 警告列表（每条前缀 `⚠ `）
- 复制按钮与底部 `📋 复制命令` 复制的是 **模板原始命令参数 `CommandArgs`**（不含 `ffmpeg` 前缀、不含展开后的参数），文案见 §3.8；**注意这与转换页的「完整命令」不同**。

### 5.3 模板编辑器内的「命令预览」

来源：`Components/Shared/TemplateEditor.razor:228-272, 1065-1135`

- 说明文案：`按各参数的默认值展开；{input} 与 {output} 用占位路径代替。`
- 预览用参数值 = 每个参数的 `DefaultValue`
- 预览用编码器画像解析优先级：具体编码器名 → 该族中「支持当前平台」优先、再按 `SortPreference` → 目标格式的软件编码器 → `libx264`
- **刷新时机**：任何会影响模板定义或参数默认值的编辑后立即重算（名称、分类、描述、输出扩展名、命令参数、参数增删改/排序、控件类型、默认值、选项值、硬件策略、所需编码器、目标编码格式、插入占位符）
- 展示：`<pre class="te-code">ffmpeg {预览命令}</pre>`（最多 260px 高）
- 复制按钮 `📋 复制`（无内容时禁用）：复制 `"ffmpeg " + _previewCommand`
  - 成功 → Success `命令已复制到剪贴板`
  - 失败 → Error `复制失败：浏览器不允许访问剪贴板，请手动选中命令文本后复制。`
  - 异常 → Error `复制失败：{异常消息}`

### 5.4 模板卡片 / 任务行的复制

- 模板卡片（网格与列表模式）：复制的是 `Template.CommandArgs` 原文（**不是**展开后的命令）。成功 → Success `命令参数已复制到剪贴板`；空 → Warning `该模板没有可复制的命令参数`；被拒 → Error `复制失败：浏览器拒绝了剪贴板访问，请手动选择命令文本`；异常 → Error `复制失败：{异常消息}`。
- 任务列表的 `复制命令`：复制 `"ffmpeg " + ActualCommand`（**任务执行时的实际命令**）。命令为空 → Warning `这个任务还没有生成实际命令（可能尚未开始执行），暂时没有可复制的内容。`；成功 → Success `命令已复制到剪贴板`；被拒 → Error `复制失败：浏览器不允许访问剪贴板，请在详情里手动选中文本。`

### 5.5 复制到剪贴板的统一实现约定（React 版必须等价）

来源：`wwwroot/js/app.js:43-71`

1. 入参为 null/undefined → 直接返回 false。
2. 优先 `navigator.clipboard.writeText`，且要求 `window.isSecureContext`。
3. 失败或不可用 → 回退到隐藏 `<textarea readonly>` + `select()` + `setSelectionRange(0, len)` + `document.execCommand('copy')`。
4. 全过程用布尔值表达成功/失败，**不抛异常**；任何异常都返回 false。
5. 上层根据布尔值给出「已复制」或「复制失败」的 Toast，并且**每条失败信息都要提示用户「可手动选中文本复制」**。

### 5.6 命令预览的全局规则（来自 `docs/ui-conventions.md:214-219`）

- FFmpeg 命令里**不要**写 `-y` / `-n`，执行层会按「文件已存在时的处理方式」自动添加。
- 覆盖策略由输出路径解析器在任务执行时处理（`Ask` 在批处理中等价于自动重命名），**任何分支都不会让 FFmpeg 阻塞等待输入**。
- 硬件加速模板应使用 `{encoder}` + `{quality_args}` + `{preset_args}` + `{hwupload_args}`，而不是硬编码厂商编码器。
- `StartTaskAsync` 立即返回，任务在后台队列执行，**页面跳转不会中断转换**。

---

## 6. 专题：文件选择 UX（File Picker）

### 6.1 四种输入路径的现状与结论

来源：`FileSelector.razor:1-12`、`BatchFileList.razor:222-363`

| 方式 | 是否支持 | 说明 |
| --- | --- | --- |
| **服务端原生对话框**（`浏览` 按钮） | ✅ 支持（推荐路径） | 有 `IFileDialogService` 后端时可用；后端起进程调用系统对话框，最长等待 5 分钟 |
| **粘贴多行路径** | ✅ 支持（批量页专属） | 批量页输入框支持一次粘贴多行，按 `\n`/`\r` 拆分、逐行 Trim、逐行判定 |
| **手工键入/粘贴单行路径** | ✅ 支持（全站） | 单文件页与设置页的 `FileSelector` |
| **文件夹扫描** | ✅ 支持（批量页） | `选择文件夹` 或粘贴目录路径 → 递归（可选）扫描媒体文件 |
| **拖放（drag & drop）** | ❌ **已移除** | 浏览器出于安全限制只暴露 `File.name`，拿不到绝对路径；旧实现既拿不到路径又会导致浏览器导航、丢页面状态。**界面已删除「拖拽文件到此处」等全部拖放提示**，全局也注册了 `dragover`/`drop` 的 `preventDefault` 防止浏览器跳转（`app.js:152-157`） |

> React 版请**不要**添加拖放，除非先解决「浏览器拿不到绝对路径」的问题（例如改用 `<input type="file" webkitdirectory>` 或后端上传）。若一定要保留拖放，必须同时阻止默认导航行为。

### 6.2 单文件路径选择器（`FileSelector`）交互摘要

- 输入即校验（350 ms 防抖回传父组件）；失焦/回车立即提交。
- 规范化：`Trim()`，并能去掉资源管理器「复制为路径」产生的首尾引号。
- 路径里的 `~` 只在内部展开用于校验/打开对话框，**不回写用户输入**。
- 校验错误 → 输入框红色边框 + `⚠️ {错误}`；`aria-invalid="true"` 与 `aria-describedby` 指向错误/文件信息节点。
- 文件信息：`文件大小: {格式化大小}`（仅文件、存在且文本非空）。
- 浏览按钮：`📂 浏览`，点击时若后端不可用立即给出警告；打开成功后把选中路径写回并按同一套校验刷新。

**每种失败模式的确切文案（逐字）**

| 失败模式 | 级别 | 文案 |
| --- | --- | --- |
| 后端不可用（点击时） | Warning | `当前环境不支持系统文件对话框（{后端描述}），请在输入框中手动粘贴路径。` |
| 用户取消 **或** 后端返回 null | Warning | `没有选择文件：可能已取消，或系统对话框（{后端描述}）不可用。可直接把路径粘贴到输入框。`（目录版把 `文件` 换成 `文件夹`） |
| 打开对话框抛异常 | Error | `打开文件对话框失败：{异常消息}（对话框后端：{后端描述}）。可直接把路径粘贴到输入框。` |
| 父组件未接收路径（回调异常） | Error | `路径未能保存：{异常消息}` |
| 校验：文件不存在/不可访问 | 内联红字 | `文件不存在或无法访问` |
| 校验：目录不存在 | 内联红字 | `目录不存在` |
| 校验：扩展名不允许 | 内联红字 | `不支持的文件格式，允许: {允许列表}` |
| 校验：读路径抛异常 | 内联红字 | `路径无法读取：{异常消息}` |
| 校验：路径格式无法识别 | 内联红字 | `路径格式无法识别：{异常消息}` |
| 初始目录无法识别 | Warning | `无法识别当前路径所在目录，将使用默认目录：{异常消息}` |

- 对话框标题：`选择{Label}`（如 `选择输入文件`、`选择输出目录`）；文件过滤串 `允许的文件|*.mp4;*.mkv|所有文件|*.*`。
- 后端描述取值：`Windows 文件对话框（PowerShell）` / `macOS 原生选择器（osascript）` / `zenity` / `kdialog` / `不可用（未安装 zenity 或 kdialog）`。

### 6.3 批量页的文件/文件夹添加（`BatchFileList`）

- **多行粘贴**：输入框占位 `粘贴文件或文件夹的完整路径，按 Enter 添加（可一次粘贴多行）`；一行为文件 → 直接加入；一行为目录 → 触发扫描；一行不存在 → 记为缺失。
- **`选择文件` 按钮**：`PickFileAsync("选择媒体文件", null, 初始目录)`；初始目录 = 已有文件的第一个文件的父目录；只添加**单个**文件（每次点击加一个）。
- **`选择文件夹` 按钮**：`PickFolderAsync("选择包含媒体文件的文件夹")`，随后按「递归扫描子文件夹」选项扫描。
- **递归扫描**：复选框 `递归扫描子文件夹`（默认**勾选**）；扫描期间显示 `正在扫描目录…` 且两个浏览按钮禁用。
- **扫描上限**：默认 5000 个，超限只取前 N 并提示。
- **去重**：路径绝对化后按序号比较去重；已存在的算「重复项」。
- **大小缓存**：每个文件大小只探测一次并缓存，避免重复同步 IO。
- **筛选**：文件数 > 6 时出现按文件名筛选框，显示 `匹配 {x} / {y}`。

**每种失败模式的确切文案（逐字）**

| 场景 | 级别 | 文案 |
| --- | --- | --- |
| 粘贴的路径不存在（前 3 条） | Warning | `以下路径不存在，已忽略：{p1、p2、p3}`（超过 3 条追加 ` 等共 {N} 个`） |
| 添加过程异常 | Error | `添加失败：{异常消息}` |
| 文件对话框返回 null | Info | `没有选择文件。如果对话框没有弹出，可以直接把路径粘贴到上方输入框。（当前后端：{后端描述}）` |
| 文件选择器异常 | Error | `打开文件选择器失败：{异常消息}` |
| 文件夹对话框返回 null | Info | `没有选择文件夹。如果对话框没有弹出，可以直接把文件夹路径粘贴到上方输入框。（当前后端：{后端描述}）` |
| 文件夹选择器异常 | Error | `打开文件夹选择器失败：{异常消息}` |
| 目录里没有媒体文件 | Warning | `所选目录中没有找到可识别的媒体文件。` |
| 目录文件过多 | Warning | `该目录文件过多，只添加了前 {上限} 个。建议缩小范围后分批处理。` |
| 扫描异常 | Error | `扫描目录失败：{异常消息}` |
| 全部重复 | Info | `这些文件已经在列表中了。` |
| 没有可添加项 | Info | `没有可添加的文件。` |
| 添加成功 | Success | `已添加 {N} 个文件`（有重复时追加 `，跳过 {M} 个重复项`） |
| 清空列表成功 | Success | `已清空文件列表（磁盘上的文件未受影响）` |

### 6.4 设置页的两处路径输入

- `FFmpeg 可执行文件` / `ffprobe 可执行文件` 都用 `FileSelector`，但 **`ValidateExists=false`**（允许：留空自动探测、填可执行文件名让系统 PATH 解析、填非标准路径）。
- 因此这两个字段**没有**「文件不存在」内联错误，而是通过 `测试 FFmpeg` / `测试 ffprobe` 按钮给出结论（文案见 §4.7）。
- 解析结果只在「保存」或「重新检测」后更新（弱化提示 `（保存或「重新检测」后更新）`）。

---

## 7. 专题：批量处理 UX（Batch）

### 7.1 添加 / 移除文件

- **添加**：多行粘贴 + Enter、`选择文件`、`选择文件夹`（递归可选）；三种方式都会去重、绝对化、并在成功后弹 `已添加 {N} 个文件`。
- **移除单个**：每行右侧 `✕`（`aria-label="移除 {文件名}"`），处理中禁用。
- **清空**：头部 `清空` 按钮（有文件时才出现），经确认框后清空（文案见 §3.10）。
- **处理期间锁定**：`IsProcessing` 为真时，添加框、添加按钮、两个浏览按钮、递归复选框、每个移除按钮、清空按钮**全部禁用**；模板参数编辑器也禁用。
- **筛选**：文件数 > 6 时可按文件名筛选，并显示命中数。

### 7.2 列表状态（per-file 状态）

- 状态来源：`FileStatuses[完整路径] = TaskStatus 名称`，由批次任务列表轮询/事件填充。
- 视觉：左侧 3px 竖条颜色 + 图标（`⏳/✅/❌/⏹️/⏭️/⏸️`，无状态时按扩展名给 `🎬/🎵/🖼️/📄`）+ 状态徽标（`进行中`/`已完成`/`失败`/`已取消`/`已跳过`/`等待中`）。
- 未开始时所有文件无状态徽标（图标由扩展名决定）。

### 7.3 每文件进度显示

- **列表行不显示百分比**；百分比显示在 `BatchProgressPanel` 的汇总与任务行里（`{0.#}%` + `{0.##}x`）。
- 汇总进度条 = 「已结束任务数 / 总数」，不是「平均进度」。已结束 = 完成 + 失败 + 取消 + 跳过。
- 每 1 秒轮询时以**数据库为准**刷新状态与进度；`TaskProgressChanged` 事件到来时立即就地更新内存对象的 Progress/Speed/Eta 并重渲染（更细腻的实时感）。
- 单文件日志在批量页**不提供**（只有单文件转换页与任务列表页提供日志查看）；批量页只提供 `复制错误`。

### 7.4 重试全部失败

来源：`BatchProcess.razor:111-116, 319-355`

- 按钮仅在「非处理中 且 失败数（失败+取消）> 0」时出现：`重试全部失败项 ({N})`
- 逐个调用重试接口，统计成功/失败：
  - 有成功 → 进入处理中状态、重启 1 秒轮询、Success `已重新排队 {N} 个任务`
  - 全失败 → Error `重试失败：{第一条错误消息 或 没有任务被重新排队}`
  - 没有可重试项 → Info `没有需要重试的任务。`
- **注意**：批量页的「重试全部失败项」**没有二次确认**；而任务列表页的「♻️ 重试全部失败任务」**有确认框**（见 §4.4）。
- 单任务重试会产生新任务（带 `RetryOfTaskId`），刷新时会一并纳入统计并去重。

### 7.5 批量页的取消语义

- `❌ 取消全部`：取消本批次所有**等待与运行中**任务，仅触发一次 Toast：`已取消本批次中所有等待和运行中的任务。`
- 单任务取消（面板内）：运行中使用**优雅停止**，否则强制取消；对应 Toast `已发送优雅停止指令。` / `已取消该任务。` / `该任务已经结束。`

### 7.6 批量页的批量参数

- 参数区标题明确标注 `对本批次所有文件生效`。
- 模板变化时：若当前参数字典为空或包含模板已不存在的键，则重置换为模板默认值。

---

## 8. 专题：模板 UX（Templates）

### 8.1 列表 / 搜索 / 筛选 / 排序（模板管理页）

- **搜索**：250 ms 防抖；匹配 名称 / 描述 / 分类 / 输出扩展名 / 所需编码器 / 标签，忽略大小写。
- **筛选**：分类下拉（`全部分类` + 全部分类）、`只看本机可用的模板` 复选框（未知状态不算可用）。
- **排序**：`推荐优先`（推荐 → 排序权重 → 名称）/ `名称` / `最近修改`（更新时间降序 → 名称）。
- **视图**：`▦ 平铺` / `☰ 分组`（分组按分类，空分类归入 `未分类`）。
- **可见性徽标**：`预设` / `自定义`；硬件徽标 `🚀 {编码器}` / `🚀 硬件可用` / `🚀 硬件加速（未检测）` / `⚠ 硬件不可用`。
- **可用性灰化规则（完整）**：
  | 情形 | 卡片表现 | 交互限制 | 原因文本 |
  | --- | --- | --- | --- |
  | Required + 未指定编码器族 | 可用性 false | 「选择/直接使用」禁用并置灰 | `该模板要求硬件加速，但没有指定编码器族，无法在本机使用。` |
  | Required + 硬件检测未完成 | 可用性 null（未知） | 不禁用（未知不等于不可用） | `硬件检测未完成，无法确认该模板是否可以在本机运行。` |
  | Required + 族不可用 | 可用性 false | 禁用「选择/直接使用」（按钮 `title` = 原因） | `未检测到可用的 {族显示名} 编码器，本机无法使用该模板。` |
  | Auto / None | 始终可用 | 无限制 | — |
  | 点击已禁用的「选择」 | — | Toast.Warning(不可用原因) | 同上 |
- 模板选择器（转换/批量页）里的灰化更严格：**不可用模板整行 `disabled` 且 `opacity:.65`**，并在行内直接显示原因；默认隐藏不可用模板（只留计数提示），可一键显示/隐藏。

### 8.2 模板卡片内容（`TemplateCard`）

名称、类型徽标、硬件徽标、描述、`⚙ 可调参数（N）` + 参数标签（前 3 个 + ` 等 N 项`）、`📁 .{扩展名}`、`📂 {分类}`、（列表模式）完整命令参数 + 复制按钮、操作图标组（查看/复制/编辑或复制为自定义/删除）、主按钮（`选择` / `直接使用`）。

### 8.3 详情查看器内容（`TemplateDetailViewer`）

五个区块：**基本信息 / 命令与预览 / 参数 / 格式 / 硬件加速 / 其它**，字段与文案见 §3.8。底部六个动作：`↻ 重新读取`、`关闭`、`📋 复制命令`、`📑 复制为自定义模板`（系统）或 `✏️ 编辑模板`（用户）、`🚀 使用此模板`。

### 8.4 编辑器表单结构与分组（`TemplateEditor`）

七个区块，顺序固定：**基本信息 → FFmpeg 命令参数 → 命令预览 → 试运行 → 校验结果（实时） → 参数（N） → 硬件加速**，底部 sticky 按钮栏。字段级标签、占位、提示、校验文案见 §3.9。

### 8.5 参数编辑器控件生成规则

按 `TemplateParameter.Kind` 生成（`Select` → 下拉、`Number` → 数字、`Boolean` → 复选框开关、`Text` → 文本框），并：
- `Number` 额外支持 `Min`/`Max`（标签里显示 `（min ~ max）`）
- `Boolean` 额外支持 `TrueValue` / `FalseValue`（勾选/取消时替换命令里的 `{参数名}` 片段）
- `Select` 额外支持选项列表（`Value` / `Label` / `Description`，选中项的 Description 显示在控件下方）
- 取值回退 `DefaultValue`；回写时复制字典，绝不原地修改
- 编辑参数时若参数名不符合「小写字母/数字/下划线且不以数字开头」→ 黄色警告条提示该参数不会被识别为占位符

### 8.6 试运行（Trial Run）交互与结果展示

- 两个入口：
  - 模板编辑器内 `🧪 试运行模板`（用**草稿**模板，即未保存的当前编辑内容；会先规范化再校验）
  - 转换页 `🧪 试运行模板`（用当前选中的模板 + **当前参数值**）
- 说明文案（编辑器内）：`会用 FFmpeg 合成 3 秒测试素材真实执行一次，可验证模板在本机能否跑通（通常几秒钟）。`
- 运行中：按钮内 spinner + `试运行中…` 且禁用；编辑器底部同时显示徽标 `试运行中…`。
- 结果块（`role="status" aria-live="polite"`）：状态图标（`✅`/`❌`）+ 消息 + 编辑器内额外显示 `耗时 {x.x 秒}` + 命令 + 可折叠的 `FFmpeg 输出（诊断信息）`。
- Toast：成功 `模板试运行成功，该模板在本机可以跑通` / `模板试运行成功，该模板在本机可用`；失败 `模板试运行失败，请看下方详情`；异常 `试运行出错：{异常消息}`。
- 转换页结果块可手动 `关闭`（清空结果）。

### 8.7 导入 / 导出对话框

**导出**
- 两个按钮：`⬇️ 导出全部`、`⬇️ 导出当前筛选结果（{N}）`（后者仅在模板管理页）。
- 无下载行为：文件名 `ffmpegwebui-templates-yyyyMMdd-HHmmss.json`，MIME `application/json`，通过创建 Blob + 隐藏 `<a download>` 触发。
- 成功/失败文案见 §4.5。

**导入**
- 弹窗标题 `导入模板`（`Size="lg"`），说明、textarea、复选框、警告条、`取消`/`导入` 按钮，见 §4.5。
- 关键限制：**浏览器不允许页面读取剪贴板，必须由用户手动粘贴**（React 版若用 `navigator.clipboard.readText()` 需注意权限与安全上下文，且要保留这条说明文案）。
- 导入为系统模板的选项在模板管理页标注为 `导入为系统模板（高级选项）`，在预设管理页为 `导入为系统模板`。

### 8.8 模板页与其它页的联动

- `选择` / `直接使用` → `/convert?template={id}`（预选，Toast `已打开转换页面，模板已预选：{名称}`）。
- 删除系统预设 → 引导到 `/templates-admin`。
- 模板变化会广播 `SettingsChanged` 之外无跨页通知；但模板选择器订阅 `SettingsChanged` 以在 FFmpeg 路径变化时重扫硬件。

---

## 9. 截图清单与页面映射

所有截图位于 `FFmpegWebUI/readmefiles/`（下表的相对路径以本文档所在目录为基准）。

| 文件 | 内容 | 对应章节 |
| --- | --- | --- |
| `index.png` | 首页（浅色）：Hero、环境状态网格、硬件编码能力、最近任务 | §4.1 |
| `dark-index.png` | 首页（深色） | §2.3 / §4.1 |
| `cv.png` | 转换页：输入输出面板、模板选择器、参数、确认区 | §4.2 |
| `cv-preview.png` | 转换页的命令预览区（完整命令 + 复制/换行按钮 + 警告） | §4.2 / §5.1 |
| `dark-convert.png` | 转换页（深色） | §2.3 / §4.2 |
| `tpl.png` | 模板管理页：计数徽标、工具栏、模板卡片网格 | §4.5 / §8.1 |
| `batch.png` | 批量处理页：文件列表、输出目录、模板选择、进度面板 | §4.3 / §7 |
| `tasks.png` | 任务列表页：进行中卡片 + 进度条、等待中、历史表格与筛选 | §4.4 |
| `setting.png` | 设置页：环境信息、硬件能力、路径、输出、性能、界面与通知 | §4.7 |

Markdown 引用（可直接复制）：

```markdown
![首页](../../../FFmpegWebUI/readmefiles/index.png)
![深色首页](../../../FFmpegWebUI/readmefiles/dark-index.png)
![转换](../../../FFmpegWebUI/readmefiles/cv.png)
![转换 · 命令预览](../../../FFmpegWebUI/readmefiles/cv-preview.png)
![深色转换](../../../FFmpegWebUI/readmefiles/dark-convert.png)
![模板](../../../FFmpegWebUI/readmefiles/tpl.png)
![批量处理](../../../FFmpegWebUI/readmefiles/batch.png)
![任务列表](../../../FFmpegWebUI/readmefiles/tasks.png)
![设置](../../../FFmpegWebUI/readmefiles/setting.png)
```

> 缺图提示：**尚没有**「模板详情弹窗」「模板编辑器」「预设模板管理」「404 / 错误页」的截图；这些界面的视觉请按本文档描述 + `app.css` 令牌实现。

---

## 10. React 组件树提案

### 10.1 目录与组件清单（props 均显式列出）

```
src/
├── app/
│   ├── App.tsx                     # 主题初始化 + 路由 + 全局 Provider
│   ├── router.tsx                  # 路由表（§1）
│   └── providers/
│       ├── ThemeProvider.tsx       # theme: 'System'|'Light'|'Dark'; 写 localStorage + cookie
│       ├── ToastProvider.tsx       # useToast(): { show, success, warning, error, dismiss }
│       ├── TasksProvider.tsx       # 任务事件流（SSE/WS/轮询）；activeCount, queuedCount, subscribe(taskId)
│       └── SettingsProvider.tsx    # 设置读写 + 变更广播（对应 SettingsChanged）
├── layout/
│   ├── AppShell.tsx                # .page 布局骨架
│   ├── SidebarNav.tsx              # props: items: NavItem[]
│   ├── TopBar.tsx                  # props: activeCount, queuedCount, version
│   └── NavFooter.tsx               # props: platform, dotnetVersion, version
├── ui/                             # 对应 app.css 的全局 class 封装
│   ├── Button.tsx                  # variant: default|primary|danger|ghost; size: sm|md|lg; loading
│   ├── IconButton.tsx              # props: ariaLabel (必填), title, onClick, disabled
│   ├── Panel.tsx                   # props: title?, children
│   ├── Badge.tsx                   # props: tone: default|primary|success|danger|warning|info
│   ├── Alert.tsx                   # props: tone: info|success|warning|danger; role
│   ├── EmptyState.tsx              # props: children
│   ├── SectionTitle.tsx            # props: title, badge?, actions?
│   ├── Spinner.tsx                 # props: size: sm|lg
│   ├── LoadingRow.tsx              # props: text
│   ├── Modal.tsx                   # props: title, open, onOpenChange, size, closeOnBackdrop, disableClose, children
│   ├── ConfirmDialog.tsx           # props: open, onOpenChange, title, message, detail?, confirmText?, cancelText?, danger?, busy?, onConfirm, onCancel?
│   ├── Toaster.tsx                 # 渲染 ToastProvider 的列表（右下角）
│   └── TextField.tsx / Select.tsx / Checkbox.tsx / TextArea.tsx  # 统一 label/help/error 排版
├── features/
│   ├── progress/
│   │   └── ProgressBar.tsx         # props: label?, progress, speed?, eta?, currentTime, totalDuration, status?
│   ├── logs/
│   │   └── LogViewer.tsx           # props: logText, title?, maxHeight?, autoScroll?, maxLines?
│   ├── filepicker/
│   │   ├── FileSelector.tsx        # props: label, value, onChange, placeholder?, showBrowse?, showFileInfo?, validateExists?, allowedExtensions?, isDirectory?, disabled?
│   │   └── useDebouncedCommit.ts   # 350ms 防抖 + 失焦立即提交
│   ├── encoder/
│   │   └── EncoderCard.tsx         # props: encoder
│   ├── templates/
│   │   ├── TemplateSelector.tsx    # props: selectedTemplate, onChange, inputPath?, showUnavailable?
│   │   ├── TemplateCard.tsx        # props: template, viewMode, isSelected, isAvailable?, unavailableReason?,
│   │   │                           #        selectedEncoderName?, showSystemBadges?, onSelect/onEdit/onDelete/onView/onCopy
│   │   ├── TemplateDetailViewer.tsx# props: template, open, onOpenChange, onCopy?, onEdit?, onUse?, parameterValues?, onParameterValuesChange?
│   │   ├── TemplateEditor.tsx      # props: template?, open, onOpenChange, isSystem, onSaved
│   │   ├── TemplateParameterEditor.tsx # props: template?, values, onChange, disabled?
│   │   ├── PlaceholderPalette.tsx  # 内置占位符分组表 + 插入按钮
│   │   ├── ParameterItemEditor.tsx # 单个参数的字段卡（含 options 编辑器）
│   │   ├── ImportTemplatesDialog.tsx # props: open, onOpenChange, asSystemDefault, onImported
│   │   └── useTemplateValidation.ts  # 校验结果 + 命令预览 + 使用中的占位符（本地派生）
│   └── batch/
│       ├── BatchFileList.tsx       # props: files, onChange, fileStatuses?, isProcessing, scanLimit?
│       └── BatchProgressPanel.tsx  # props: tasks, isProcessing, onRetry, onCancel, onOpenOutputFolder
└── pages/
    ├── HomePage.tsx                # 4 个区块（Hero / 环境状态 / 硬件编码能力 / 最近任务）
    ├── ConvertPage.tsx             # 4 个编号面板 + 进行中 + 最近任务
    ├── BatchPage.tsx
    ├── TemplatesPage.tsx
    ├── TemplatesAdminPage.tsx
    ├── TasksPage.tsx
    ├── SettingsPage.tsx
    ├── ErrorPage.tsx
    └── NotFoundPage.tsx
```

### 10.2 全局 Store（跨页面/跨组件共享）

| Store | 状态 | 理由 |
| --- | --- | --- |
| `ThemeStore` | `mode: 'System'\|'Light'\|'Dark'`，`resolved: 'Light'\|'Dark'` | 需要在首屏（HTML 层）与应用多处生效；持久化到 cookie + localStorage，并监听 `prefers-color-scheme` |
| `ToastStore` | `messages: {id, text, level, icon}[]`（最多 5 条、自动过期） | 全局唯一 Toast 容器，任何页面/异步任务都能推消息 |
| `TasksStore` | `activeCount`、`queuedCount`、`byId`、事件订阅（progress/status/log） | 顶栏计数、单文件页、批量页、任务列表页都要订阅同一事件流；由 MainLayout 级别的连接统一分发 |
| `SettingsStore` | `settings`、`loaded`、`changedAt` | 「设置变化 → 模板选择器/设置页重新检测」需要广播；主题变更也走它 |
| `HardwareStore` | `report`、`availableEncoders`、`lastDetectionAt` | 模板可用性、编码器卡片、设置页共享同一份检测结果（避免重复检测） |
| `TemplatesStore` | `templates`、`categories`、`presetVersion` | 模板页、选择器、编辑器、管理页共享；保存/删除/导入后统一失效 |

### 10.3 局部状态（component-local）

| 组件 | 局部状态 |
| --- | --- |
| `FileSelector` | 输入文本、防抖计时器、错误、文件大小/存在性、浏览中标记 |
| `LogViewer` | 行集合、搜索词（输入 + 已应用）、级别筛选、自动滚动、清空标记、复制成功标记 |
| `TemplateSelector` | 加载/错误、分类、搜索输入与已应用搜索、分组开关、显示不可用覆盖开关、行模型缓存 |
| `TemplateEditor` | 草稿对象（深拷贝）、原始指纹（脏检查）、校验/预览/警告、试运行结果、分类与编码器查找缓存、丢弃确认开关 |
| `TemplateDetailViewer` | 参数值、报告、预览/警告、复制标记、重新读取中 |
| `TasksPage` | 三个列表、筛选/排序/条数、展开的 taskId、日志缓冲（上限 12 个 × 800 行）、批量确认状态 |
| `ConvertPage` | 输入/输出/文件名、已选模板、参数值、媒体信息、状态、编码器画像、预览与警告、冲突提示、进行中/最近任务、实时日志缓冲（≤800 行/任务） |
| `BatchPage` | 文件列表、文件状态映射、输出目录、模板、参数、批次任务、处理中、轮询定时器 |
| `SettingsPage` | 表单副本 + 已保存快照（脏检查）、状态/报告、各种 loading 标记、路径测试结果与过期规则、历史消息、两个确认框 |
| `TemplatesAdminPage` | 模板列表、筛选/排序、诊断、清理天数、编辑器/删除/重置/导入状态 |

### 10.4 关键实现差异（Blazor → React 必须注意）

1. **`InvokeAsync(StateHasChanged)` 不存在了**：所有事件回调（进度/状态/日志）在 React 里改用 store + 订阅，组件只订阅自己关心的切片，避免 800 行日志导致整页重渲染。
2. **文本输入的性能约束**：原实现大量使用「输入即渲染 + 防抖回传」。React 里 `LogViewer` 的高频追加必须走虚拟化或分段渲染（原实现每次追加都会重建行列表）。
3. **时间/百分比的 CSS 注入**：React 也要用「不变文化的数字字符串」，否则 `width: 85,5%` 会被浏览器丢弃（原注释明确提到这一点）。
4. **`@key` 语义**：Blazor 的 `@key` 用来保持 DOM 复用；React 里对列表用稳定 id（`template.Id`、`task.Id`、文件路径）。
5. **Esc 关闭**：原实现是「全局监听 + 点最上层弹窗的关闭按钮」。React 里建议用事件监听器栈（后开先处理）或 `useEffect` + `keydown`，并**补上焦点陷阱与关闭后焦点归还**（原实现缺失）。
6. **模态内的 sticky 页脚**：`.ffui-modal-footer` 用负外边距 + `position:sticky; bottom:0` 贴住滚动区底部，React 版直接复用同一套 CSS 即可。
7. **服务端调用即业务逻辑**：不要把校验、命令构建、覆盖策略搬到前端重写一遍；前端只负责「展示 + 收集输入 + 调 API」，预览与校验结果都来自后端（原实现明确要求「界面里不重复实现一套近似校验」，`TemplateEditor.razor:16-18`）。

---

## 11. 附：仍需后端提供的接口（UI 契约）

按 §1 的服务清单（`docs/ui-conventions.md:79-179`）整理成 REST 视图。UI 只依赖以下能力：

| 领域 | 接口（建议） | UI 使用点 |
| --- | --- | --- |
| 环境 | `GET /api/status?refresh=` | 首页、转换页、设置页、预设管理诊断 |
| 硬件 | `GET /api/hardware?refresh=`、`GET /api/hardware/recommend?codec=&required=&strict=`、`GET /api/hardware/encoders/:name/available` | 编码器卡片、模板可用性、设置页首选编码器 |
| FFmpeg 信息 | `GET /api/ffmpeg/info?refresh=`、`GET /api/media/info?path=` | 首页、转换页媒体信息徽标 |
| 命令预览 | `POST /api/command/preview`（模板 + 上下文） | 转换页/详情/编辑器三处预览 |
| 模板 | `GET/POST/PUT/DELETE /api/templates`、`/api/templates/categories`、`/api/templates/:id/copy`、`/api/templates/sync`、`/api/templates/reset`、`/api/templates/export`、`/api/templates/import`、`POST /api/templates/validate`、`POST /api/templates/test` | 模板页、选择器、编辑器 |
| 任务 | `GET /api/tasks?status=&limit=`、`GET /api/tasks/running`、`GET /api/tasks/pending`、`POST /api/tasks`、`POST /api/tasks/:id/start`、`POST /api/tasks/:id/retry`、`POST /api/tasks/:id/cancel?graceful=`、`DELETE /api/tasks?status=`、`DELETE /api/tasks/history?before=` | 转换页、任务列表、批量页 |
| 批任务 | `POST /api/batches`、`GET /api/batches/:id/tasks`、`POST /api/batches/:id/cancel` | 批量页 |
| 文件 | `GET /api/fs/scan?dir=&recursive=&limit=`、`GET /api/fs/size?path=`、`GET /api/fs/writable?path=` | 批量文件列表 |
| 原生对话框 | `POST /api/dialog/file`、`POST /api/dialog/folder`（返回 `{path, supported, backendDescription}` 或 `null`） | 所有 `浏览` 按钮 |
| 设置 | `GET/PUT /api/settings`、`POST /api/settings/reset`、`POST /api/settings/test-ffmpeg`、`POST /api/settings/test-ffprobe` | 设置页 |
| 实时 | `GET /api/events`（SSE）或 WebSocket：`task.progress` / `task.status` / `task.log` / `settings.changed` | 顶栏计数、进度条、实时日志、任务列表 |

**实时事件 → UI 映射（这是「哪些值会实时更新」的完整清单）**

| 实时值 | 更新方式 | 频率 |
| --- | --- | --- |
| 顶栏「N 个转换进行中（M 个排队）」 | 事件驱动 | 状态变化时 |
| 首页检测耗时徽标 | 本地定时器 | 200 ms |
| 转换页活动任务进度/速度/ETA | 事件驱动 | 解析到进度行时 |
| 转换页展开任务的日志 | 事件驱动 | 每行日志到达；内存上限 800 行/任务 |
| 批量页汇总进度与每文件状态 | 本地轮询 + 事件 | 轮询 1 秒；事件即时 |
| 任务列表「进行中」区块 | 本地轮询 + 事件 | 轮询 2 秒；进度事件即时；状态事件有 200 ms 合并延迟 |
| 任务列表展开日志 | 事件驱动 | 每行到达；缓冲上限 12 个任务 × 800 行 |
| 设置页硬件检测耗时 | 本地定时器 | 400 ms |
| 模板选择器的硬件可用性 | 设置变化事件（仅 FFmpeg 路径变化时强制重扫） | 事件驱动 |
| 主题（跟随系统时的系统切换） | `prefers-color-scheme` 监听 | 系统变化时 |

---

## 12. 中文文案速查表（按出现位置索引，供逐字核对）

> 本节列出最容易写错的高频文案，完整集合见各章节。

**全局**
- `确认操作`、`确定`、`取消`、`关闭对话框`、`关闭提示`、`重试`
- `页面发生了未处理的错误。`、`重新加载`、`页面渲染失败：{消息}`、`重新加载页面`

**格式与占位**
- `—`（空值）、`-`（无耗时）、`0 B`、`{N} 个`、`共 {大小}`

**状态词**
- `等待中`、`运行中`、`进行中`、`已完成`、`失败`、`已取消`、`已跳过`

**易错处（务必逐字）**
1. `查看全部占位符` / `收起全部占位符`
2. `模板库为空，请先在「模板管理」中创建模板或重置系统预设。`
3. `该模板没有可调参数`（详情）vs `该模板没有可调参数，将直接使用模板预设值。`（转换页）
4. `还没有任何任务，去「转换」页开始第一个吧。`（首页/任务列表）vs `去「转换」页开始第一个`（按钮）
5. `没有符合当前筛选条件的任务，试试清空筛选条件或点「刷新」。`
6. `将删除全部「失败」和「已取消」的任务记录，运行中与等待中的任务不受影响。此操作不可撤销。`
7. `已提交 {N} 个文件到处理队列` / `✅ 批量处理完成，成功 {N} 个文件` / `批量处理完成：成功 {N} 个，失败 {M} 个`
8. `重试全部失败项 ({N})`（批量页，圆括号是半角）
9. `♻️ 重试全部失败任务`（任务列表页）
10. `已重新排队 {N} 个任务`（批量）vs `已重新加入队列：{文件名}`（任务列表）
11. `该模板要求使用 {族} 硬件编码，但本机没有可用的对应编码器，无法执行。建议改用「推荐」分类下会自动挑选编码器的模板。`
12. `系统预设可以编辑，但应用升级时同名预设会被内置内容覆盖（按预设版本同步）。需要长期保留修改，请改完后复制成普通模板。`
13. `不要硬写 -crf：硬件编码器不接受它…`（完整段落见 §3.9）
14. `「不使用」时会清空这一项；标记「（本机不可用）」的选项仍可选——模板也可能是为别的机器准备的。`
15. `把「导出」得到的 JSON 内容粘贴到下面。浏览器出于安全限制不允许页面直接读取剪贴板，请手动粘贴（⌘/Ctrl + V）。`
16. `没有导入任何模板：JSON 里没有含 name 与 commandArgs 的模板条目。`
17. `已在文件管理器中打开输出目录。`（Home/Tasks）vs 无提示（Convert/Batch 内部调用）
18. `输出目录不存在，可能已被移动或删除。`
19. `主题已在本页生效，但没能记住（保存设置时会一起写入）：{异常消息}`
20. `切换后立即生效并自动记住，下次打开时保持（不需要点「保存设置」，也不需要刷新页面）。`
