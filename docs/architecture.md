# 架构说明

本项目是 [FFmpegWebUI](../FFmpegWebUI)（C# / .NET 10 / Blazor Server）的 **Rust + React 重写版**：
服务端用 **axum + tokio + SQLite**，前端用 **Vite + React + TypeScript + Tailwind**，
构建产物由 `rust-embed` 内嵌进二进制，**单文件启动、浏览器操作、转换在本机执行**。

```
┌──────────────────────────────────────────────────────────────┐
│  浏览器 (http://127.0.0.1:5266)                                │
│    React SPA ──HTTP /api/*──┐                                 │
│              ──WS  /api/ws──┤                                 │
│                             ▼                                 │
│  axum（单进程、仅监听 127.0.0.1）                              │
│    ├── AppState：Settings / Templates / Tasks / Queue / ...    │
│    ├── ConversionQueue（常驻后台 worker，与 UI 连接无关）       │
│    └── 子进程：ffmpeg / ffprobe（每个任务一个）                 │
│  SQLite（data.sqlite3）：settings / templates / tasks / …      │
└──────────────────────────────────────────────────────────────┘
```

## 1. 模块划分与参考实现的对应关系

| 本项目 | 参考实现（C#） | 职责 |
| --- | --- | --- |
| `src/cli.rs` | `Program.cs`（部分） | 启动参数、绑定地址、端口、自动开浏览器 |
| `src/paths.rs` | `Services/AppPaths.cs`、`AppInfo.cs` | 平台判定、数据目录、路径规整 |
| `src/models/` | `Models/*.cs` | 领域模型、枚举、DTO |
| `src/ffmpeg/locator.rs` | `Services/FFmpegLocator.cs` | ffmpeg / ffprobe 定位 |
| `src/ffmpeg/service.rs` | `Services/FFmpegService.cs` | 版本/能力探测、ffprobe、子进程执行与进度解析 |
| `src/ffmpeg/encoder_catalog.rs` | `Services/EncoderCatalog.cs` | 31 条编码器参数画像 |
| `src/ffmpeg/hardware.rs` | `Services/HardwareDetectionService.cs` | 硬件编码器检测（两层试跑 + 三层缓存） |
| `src/command/` | `Services/CommandLineBuilder.cs` | 模板分词、占位符替换、参数注入位置 |
| `src/templates/` | `Services/TemplateService.cs`、`SystemTemplatePresets.cs` | 模板 CRUD、校验、试运行、导入导出、系统预设同步 |
| `src/store/` | `Data/LiteDbContext.cs` | SQLite 持久化（替代 LiteDB） |
| `src/services/` | `Services/{Settings,File,FileDialog,OutputPath,AppStatus}Service.cs` | 设置、文件、原生对话框、状态快照 |
| `src/queue/` | `Services/ConversionQueue.cs`、`TaskService.cs` | 后台转换队列、任务生命周期 |
| `src/api/` | Blazor Server 的 SignalR circuit | REST + WebSocket，见 `api-contract.md` |
| `src/web_assets.rs` | `wwwroot/` + `MapStaticAssets` | 内嵌前端产物与 SPA 回落 |
| `web/` | `Components/*.razor` + `wwwroot/app.css` | React 前端 |

## 2. 数据流

1. **转换不依赖 UI 连接**：任务在 `ConversionQueue` 的常驻 worker 里跑；页面刷新、断线、
   关掉浏览器都不影响（这是参考实现最重要的不变式）。
2. **进度来自 `-progress pipe:2`**：解析 `key=value` 块，而不是解析人读的 `-stats` 文本。
3. **服务端推送**：队列事件 → 广播通道 → WebSocket。日志在服务端按 100–200ms 合并，
   避免高频 stderr 打满前端。
4. **前端不做推断**：命令预览、输出路径冲突解析、模板可用性、覆盖策略全部由服务端算好返回。

## 3. 与原实现的有意差异

| # | 项 | 参考实现 | 本项目 | 原因 |
| --- | --- | --- | --- | --- |
| 1 | 存储 | LiteDB `data.db` | SQLite `data.sqlite3` | 事务/崩溃安全/生态；**刻意不复用文件名**，避免把 LiteDB 文件当 SQLite 打开而损坏 |
| 2 | 传输 | Blazor Server circuit | REST + WebSocket | 前端换成 React |
| 3 | 硬件回退判定 | `ProducedAnyOutput` 在判定前被清空，「已产出文件就不回退」形同虚设 | 按注释表达的意图修正 | 这是参考实现的缺陷 |
| 4 | 运行期进度落库 | 只在结束时写 | 每 1 秒落一次 `progress` / `current_time` | 崩溃后能显示中断位置 |
| 5 | 入队去重 | 同一 taskId 可被两个 worker 各跑一次 | 同一个任务只会被执行一次 | 缺陷 |
| 6 | 并发上限 | 模型 1..16，设置页表单 1..8 | 统一 1..16 | 消除不一致 |
| 7 | 软件模板试运行 | 没有 `{encoder}` 的软件模板会被判「缺少硬件编码器」 | 区分「不需要画像」与「没有可用编码器」 | 缺陷 |
| 8 | 端口 | 5265 | **5266** | 允许与参考实现同时运行 |
| 9 | 环境变量前缀 | `FFMPEGWEBUI_*` | `FFMPEGWEBRUST_*` | 同上 |
| 10 | 启动读取偏好 | 在 Web 主机启动前另开一次数据库读 `ListenPort` | 先异步打开数据库读设置，再决定监听地址 | `main` 本身就是异步的，不需要旁路读取 |
| 11 | 模态框无障碍 | 无焦点陷阱、无焦点归还 | 补上焦点陷阱与焦点归还 | 参考实现缺失 |
| 12 | 模板 id | 随机 ObjectId，同步靠 Name 匹配 | 系统预设使用由稳定 key 派生的确定性 id | 改名不再丢同步关系 |

## 4. 目录结构

```
Cargo.toml                 Rust 依赖与打包配置
src/                       服务端
  main.rs / lib.rs         入口 / 库入口
  api/                     REST 路由、WebSocket、错误映射
  bin/                     自检工具（真实 ffmpeg 冒烟测试）
web/                       React 前端（Vite）
  src/{api,components,lib,pages,store}
docs/
  architecture.md          本文件
  api-contract.md          HTTP / WebSocket 契约（冻结）
  reference/               移植规格：从参考实现提取的逐字行为说明
tests/                     端到端集成测试
tools/verify/              浏览器点击回归脚本（无头 Chrome）
```

## 5. 开发与运行

```bash
# 后端（同时托管已构建的前端产物）
cargo run

# 前端开发模式（Vite 5173，/api 代理到 5266）
cd web && pnpm dev

# 前端构建（产物进 web/dist，被 rust-embed 内嵌）
cd web && pnpm build

# 测试
cargo test
cargo run --bin e2e_smoke      # 真实 ffmpeg 端到端
node tools/verify/browser-check.mjs   # 无头浏览器点击回归
```

打包：`cargo build --release` 产出单个可执行文件；前端产物已在编译期内嵌，
目标机器只需有 ffmpeg（或用 `FFMPEGWEBRUST_FFMPEG_DIR` 指定随包附带的二进制）。
