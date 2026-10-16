# FFmpegWebRust

一个**跑在你本机的 FFmpeg 图形界面**：用浏览器操作，转换在本机执行，不依赖任何云服务。

本项目是 [FFmpegWebUI](../FFmpegWebUI)（C# / .NET 10 / Blazor Server）的 **Rust + React 重写版**：
服务端换成 **axum + tokio + SQLite**，前端换成 **Vite + React + TypeScript + Tailwind**，
前端构建产物在编译期被内嵌进二进制，**一个可执行文件即可运行**。

```
┌──────────────────────────────────────────────────────────┐
│  浏览器 (http://localhost:5266)                           │
│         ↕ axum（REST + WebSocket，仅监听 127.0.0.1）        │
│  FFmpeg / ffprobe  ← 本机 GPU 硬件编码器                   │
│  数据：SQLite（data.sqlite3）                             │
└──────────────────────────────────────────────────────────┘
```

> 为什么用 Rust 重写：交付物从「自带运行时的多文件发布目录」变成**单个可执行文件**，
> 内存占用与启动时间都显著下降；同时保持与参考实现一致的功能与界面。

---

## 功能

### 跨平台硬件编码加速

会自动探测本机**真实可用**的硬件编码器，并且**用真实参数试跑验证**，而不是只看编码器名字在不在列表里。

| 平台 | 支持的硬件编码 |
| --- | --- |
| Windows | NVIDIA NVENC（H.264/H.265/AV1）、Intel QSV、AMD AMF、Windows Media Foundation、Vulkan Video |
| macOS | Apple VideoToolbox（H.264 / H.265 / ProRes） |
| Linux | Intel / AMD VA-API、NVIDIA NVENC、Intel QSV、Vulkan Video |

设计要点（与参考实现一致）：

- **同一份模板在所有平台通用。** 模板写 `-c:v {encoder} {quality_args}`，应用会按当前机器挑选编码器，
  并把质量参数展开成该编码器真正支持的写法（NVENC 用 `-cq`，QSV 用 `-global_quality`，
  VideoToolbox 用 `-q:v`，VA-API 用 `-qp`，软件编码用 `-crf`）。
- **两档试跑。** 先试「完整参数」，失败再试「最简参数」，因此不同 FFmpeg 构建之间的选项差异不会误判为不可用；
  真的不可用时会把 FFmpeg 的原始报错归纳成可读原因显示出来。
- **硬件失败自动回退。** 如果硬件编码器初始化失败、且**没有产出任何输出文件**，会改用软件编码重试一次。
- **绝不把决定权交给 FFmpeg 的交互提问。** 所有「文件已存在」的分支都由应用预先决定（覆盖/跳过/重命名），
  命令行里总是显式带 `-y` 或 `-n`，不会出现进程卡在等待输入、进度条永远停在 0% 的情况。

### 模板

- 内置 **51 个**预设模板，覆盖转码、压缩、分辨率、音频提取、水印、字幕、抽帧、变速等常见需求。
- 每个模板可以声明**参数**（质量、码率、预设、宽度…），界面会自动生成对应控件，而不是让你手写命令。
- **模板试运行**：保存或使用前先拿一段自动生成的测试片段跑一遍，直接告诉你这个模板在本机能不能用、用的是哪个编码器。
- **模板校验**：缺少 `{input}`/`{output}`、引号不成对、占位符没有取值、在硬件编码器上使用 `-crf`
  这类问题都会在界面里明确指出来。
- 模板可以导入/导出 JSON，方便备份与分享。
- 系统预设带版本号，升级应用时会**自动增量同步**内置模板，同时保留你自己创建和修改的模板。

### 任务

- 转换在**后台队列**里执行：页面跳转、刷新、甚至断线重连都不会中断正在进行的转换。
- 并发数可配置（1–16）；日志有长度上限，不会因为长时间录制把内存吃满。
- 支持优雅停止（发送 `q` 让 FFmpeg 写完文件尾，产出文件仍然可用）与强制终止。
- 失败/取消的任务可以一键重试；批量处理支持重试全部失败项。
- 实时进度与日志通过 WebSocket 推送（服务端按 100–200ms 合并日志，避免高频输出打满浏览器）。

### 交互

- 跨平台原生文件/文件夹选择器：macOS 用 `osascript`，Linux 用 `zenity`/`kdialog`，Windows 用 PowerShell。
  任何平台不可用时都会明确提示，而不是按钮点了没反应。
- 浅色 / 深色 / 跟随系统主题，所有界面元素都跟随主题变量。
- 全局轻提示、确认对话框、键盘 Esc 关闭弹窗、可见的键盘焦点、模态框焦点陷阱。
- 完整命令实时预览，并支持一键复制到剪贴板（复制失败会提示原因）。

---

## 环境要求

| 项目 | 要求 |
| --- | --- |
| Rust | 1.85+（仅自行编译时需要） |
| Node.js | 20+ 与 pnpm（**仅在需要重新构建前端时**需要） |
| FFmpeg | 推荐 7.x 或更新（已在 9.0.2 上验证） |
| 浏览器 | Chrome / Edge / Safari / Firefox 任一现代浏览器 |

### 安装 FFmpeg

| 平台 | 命令 |
| --- | --- |
| Windows | `winget install Gyan.FFmpeg` 或 `choco install ffmpeg`，或从 [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) 下载后把 `bin` 加入 PATH |
| macOS | `brew install ffmpeg` |
| Debian / Ubuntu | `sudo apt install ffmpeg` |
| Fedora / RHEL | `sudo dnf install ffmpeg`（需要启用 RPM Fusion） |
| Arch | `sudo pacman -S ffmpeg` |

也可以把 `ffmpeg` / `ffprobe` 放到可执行文件旁边的 `ffmpeg/` 或 `tools/ffmpeg/` 目录，
应用会优先使用它们（发布时就不要求用户自己装 FFmpeg）。

### 关于 FFmpeg 版本

应用会在启动时读取 FFmpeg 主版本号。需要注意：**FFmpeg 9.0 移除了 `-vsync`**（改用 `-fps_mode`）、
`-filter_complex_script`、`-top`，以及一批 NVENC 旧选项。内置模板不使用这些选项，因此在 7.x–9.x 上都能工作。

---

## 快速开始

```bash
# 1. 构建前端（产物会被内嵌进二进制；只需在改了界面后重跑）
cd web && pnpm install && pnpm build && cd ..

# 2. 构建并运行
cargo run --release
```

浏览器会自动打开 `http://localhost:5266`（若 FFmpeg 未安装，首页会给出指引）。

> **只跑 `cargo run` 也可以**：没有构建过前端时，页面会显示一段提示，告诉你去执行
> `cd web && pnpm build`。前端产物是**编译期**内嵌的，构建完需要重新编译一次。

### 打包

```bash
cargo build --release
```

产物是单个可执行文件 `target/release/ffmpeg-web-rust`（Windows 为 `.exe`），
前端已在其中，目标机器无需安装 Node。用户只需有 FFmpeg。

---

## 配置

优先级：命令行 > 环境变量 > 数据库里保存的设置 > 默认值。

命令行：

| 参数 | 说明 |
| --- | --- |
| `--port <n>` / `-p <n>` | 监听端口（`0` = 自动挑一个空闲端口） |
| `--bind <addr>` | 监听地址（默认 `127.0.0.1`，设为 `0.0.0.0` 才能局域网访问） |
| `--data-dir <path>` | 覆盖数据目录 |
| `--no-browser` | 启动时不自动打开浏览器 |

环境变量：

| 变量 | 作用 |
| --- | --- |
| `FFMPEGWEBRUST_PORT` | 覆盖监听端口 |
| `FFMPEGWEBRUST_BIND` | 覆盖监听地址（默认 `127.0.0.1`） |
| `FFMPEGWEBRUST_DATA_DIR` | 覆盖数据目录（设置后**不再**追加应用名子目录） |
| `FFMPEGWEBRUST_DATABASE_PATH` | 覆盖数据库文件位置 |
| `FFMPEGWEBRUST_NO_BROWSER=1` | 禁用自动打开浏览器 |

### 安全说明

应用默认**只监听 127.0.0.1**。这是有意的：这个界面能在本机执行 FFmpeg 并读写任意文件，
不应默认暴露到局域网。如果你确实需要远程访问，请设置 `FFMPEGWEBRUST_BIND=0.0.0.0`
并自行在前面加一层带认证的反向代理。

注意：原生文件选择器是在**服务端**弹出的，所以它本来就是为「服务端和你在同一台机器」设计的。

### 数据位置

| 平台 | 路径 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\FFmpegWebRust\data.sqlite3` |
| macOS | `~/Library/Application Support/FFmpegWebRust/data.sqlite3` |
| Linux | `$XDG_DATA_HOME/FFmpegWebRust/data.sqlite3`（默认 `~/.local/share/…`） |

删除 `data.sqlite3` 会清空模板（内置预设会在下次启动时重建）、任务历史和设置。

---

## API

服务端对外提供 REST + WebSocket 接口，完整契约见 [`docs/api-contract.md`](docs/api-contract.md)。

```bash
curl -s localhost:5266/api/health | jq
```

`/api/health` 返回版本、平台与 FFmpeg 解析结果，便于排查。

---

## 开发

```
src/
  main.rs / lib.rs          入口 / 库入口
  cli.rs                    启动参数、端口与浏览器
  paths.rs                  跨平台路径与应用元信息
  models/                   领域模型、枚举、DTO
  ffmpeg/                   定位、能力探测、进程执行与进度解析、编码器画像、硬件检测
  command/                  命令行构建（分词、占位符、参数注入位置）
  templates/                51 个内置预设 + 模板服务（校验、试运行、导入导出、系统同步）
  files/                    输出路径解析、文件名占位符、原生文件对话框
  store/                    SQLite 持久化
  services/                 设置、状态快照
  queue/                    后台转换队列、任务/批次服务、命令预览
  api/                      REST 路由、WebSocket、错误映射
  web_assets.rs             内嵌前端产物与 SPA 回落
web/                        React 前端
docs/
  architecture.md           模块划分与架构
  api-contract.md           HTTP / WebSocket 契约（冻结）
  reference/                从参考实现逐字提取的移植规格（行为依据）
tests/                      端到端集成测试
tools/verify/               离线与浏览器回归验证
```

构建与测试：

```bash
cargo build            # 构建（含前端内嵌）
cargo test             # 320 个单元/集成测试（其中大量用真实 ffmpeg 跑）
cargo run --bin ffmpeg_selftest   # 真实 ffmpeg 自检：定位 → 探测 → 转换 → 进度 → 取消 → 硬件检测
cargo run --bin store_selftest    # 存储层自检（39 项）
```

前端开发（Vite 热更新，`/api` 自动代理到 5266）：

```bash
cd web && pnpm dev
```

### 验证

| 检查 | 命令 | 当前结果 |
| --- | --- | --- |
| 单元/集成测试 | `cargo test` | **320 项通过** |
| FFmpeg 端到端自检 | `cargo run --bin ffmpeg_selftest` | 定位 / 能力探测 / 真实转换（进度 49.9% → 74.3% → 100%）/ 优雅取消 / 硬件检测 全通过 |
| 存储层自检 | `cargo run --bin store_selftest` | 39 项通过 |
| 浏览器端到端 | `node tools/verify/browser-check.mjs` | **21 项通过**（含在界面上真的跑完一次转换） |
| 前端类型与静态检查 | `cd web && pnpm build && pnpm lint` | 0 TS 错误、0 lint 告警 |

浏览器回归脚本会用无头 Chrome（DevTools Protocol）真的点一遍界面：
逐页确认不是白屏、模板不可用时显示原因、设置页能保存、在转换页填入文件、
选中模板后**命令预览由服务端返回并渲染**、点「开始转换」、在任务列表里看到「已完成」，
并在最后断言整条流程没有 `console.error` 或未捕获异常。

```bash
cargo run -- --port 5271 --no-browser
SAMPLE=/path/to/video.mp4 node tools/verify/browser-check.mjs
```

### 文档

- [`docs/architecture.md`](docs/architecture.md) —— 模块划分、数据流、**与参考实现的有意差异清单**
- [`docs/api-contract.md`](docs/api-contract.md) —— REST + WebSocket 契约
- [`docs/reference/`](docs/reference/) —— 四份逐字移植规格：`ffmpeg-core.md`（FFmpeg 执行与硬件检测）、
  `templates.md`（模板子系统与 51 个内置模板全目录）、`backend-data.md`（持久化/队列/输出路径/设置）、
  `ui-spec.md`（路由、设计令牌、逐页面逐组件规格）

---

## 与参考实现（FFmpegWebUI）的差异

功能对齐，但有若干**有意的改进**（完整清单见 `docs/architecture.md` §3）：

| 项 | 参考实现 | 本项目 |
| --- | --- | --- |
| 交付物 | .NET 发布目录（多文件） | **单个可执行文件**（前端已内嵌） |
| 存储 | LiteDB `data.db` | SQLite `data.sqlite3` |
| 硬件回退判定 | 判定条件写错，实际总是回退 | 按注释表达的意图修正：**没有产出文件**才回退 |
| 运行期进度 | 只在结束时落库，崩溃后丢失 | 每 1 秒落一次进度 |
| 重复入队 | 同一任务可能被执行两次 | 入队去重 |
| 软件模板试运行 | 会被误判为「缺少硬件编码器」 | 区分「不需要编码器画像」与「没有可用编码器」 |
| 并发上限 | 模型 1–16 / 表单 1–8 不一致 | 统一 1–16，且调小立即对后续任务生效 |
| 模态框无障碍 | 无焦点陷阱、关闭后不归还焦点 | 已补上 |
| 端口 | 5265 | **5266**（可与参考实现同时运行） |

---

## 许可

见 [LICENSE](LICENSE)。
