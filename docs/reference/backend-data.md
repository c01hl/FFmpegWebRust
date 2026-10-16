# FFmpegWebUI 后端数据与运行时行为参考规范

> **用途**：这是 Rust 重写版（FFmpegWebRust）的移植参考。目标读者是**只能看到本文、看不到原 C# 源码**的 Rust 开发者。
> 本文描述的是**参考实现（C# / .NET 10 / Blazor Server）的实际行为**，不是理想设计；凡实现中存在
> 「注释与代码不一致」「有明显副作用」的地方都会显式标注 ⚠️，并给出移植建议。
>
> **代码基线**：`/Users/c01h/code/github/FFmpegWebUI`（`FFmpegWebUI/` 为项目根，下表路径均相对于该根）。
>
> **阅读约定**（重要）：
> - 【必须存储】= 换实现后仍必须持久化的**数据语义**；
> - 【原实现】= 参考实现**碰巧怎么存的**（LiteDB/BSON 细节），Rust 侧不必照抄，但迁移/兼容时要看；
> - 每个小节标题后都给出 `源文件:行范围`，行号基于上述基线。
>
> **本章约定**：文中的 `{...}` 若出现在 JSON 代码块里表示 JSON 对象；出现在表格/文本里表示「按名字替换」。

---

## 0. 总体结构与移植边界

参考实现是"单进程桌面工具"形态：一个 Blazor Server 进程既跑 UI 又在同一进程里跑转换队列。

```
浏览器 ──HTTP/WebSocket(circuit)──▶ ASP.NET Core(Blazor Server)
                                     ├── LiteDbContext（单例，5 个集合）
                                     ├── SettingsService（单例，内存缓存 + 落库）
                                     ├── FFmpegLocator（单例，带缓存）
                                     ├── FFmpegService（单例，进程登记表 ObjectId→Process）
                                     ├── HardwareDetectionService（单例，检测报告缓存）
                                     ├── TemplateService（单例，系统模板同步）
                                     ├── ConversionQueue（单例，Channel + N 个 worker）
                                     ├── TaskService（单例，CRUD + 转发 ConversionQueue 事件）
                                     └── AppStatusService（单例，10 秒缓存的状态快照）
                                            │
                                            └── 子进程：ffmpeg / ffprobe（每个任务一个）
```

移植时必须复刻的**跨层不变式**（这些是原实现的"外部行为契约"）：

1. **转换不依赖 UI 连接**：任务在后台执行；页面跳转/刷新/断线都不影响（`ConversionQueue.cs:11-15`）。
   因此 Rust 侧任务执行器必须是独立于 WebSocket 会话的常驻服务。
2. **唯一的执行入口是队列**：`TaskService` 只做 CRUD 与入队，任何"运行"都必须经过队列（`TaskService.cs:8-12`）。
3. **绝不把冲突决定权交给 ffmpeg 交互提问**：命令行总是显式带 `-y` 或 `-n`
   （`FFmpegService.cs:561-562`），否则 ffmpeg 会在"文件已存在"时读 stdin 永久挂起。
4. **进度来自 `-progress pipe:2` 的 key=value 块**，而不是解析人读的 `-stats` 文本
   （`FFmpegService.cs:563-565, 942-1041`）。
5. **`Id` 是 12 字节 ObjectId**（24 位小写十六进制字符串）。它出现在任务的 `BatchId`/`RetryOfTaskId`、
   模板 `TemplateId`、前端路由与所有 API 中。

### 0.1 术语与枚举速查（完整定义见 §2.1）

| 枚举 | 取值 | 语义 |
| --- | --- | --- |
| `TaskStatus` | `Pending=0` | 待执行（已入库、未入队或已入队未开始） |
| | `Running=1` | 正在执行（ffmpeg 子进程存活） |
| | `Completed=2` | ffmpeg 退出码 0 |
| | `Failed=3` | 非 0 退出 / 超时 / 模板缺失 / 目录不可写等 |
| | `Cancelled=4` | 用户取消 |
| | `Skipped=5` | 输出已存在且策略为跳过 |
| `FileExistsAction` | `Ask=0` / `Overwrite=1` / `Skip=2` / `Rename=3` | 输出已存在时的处理（见 §4.1、§4.9） |
| `OutputNamingRule` | `Original=0` / `Suffix=1` | 输出命名规则 |
| `EncoderType` | `Software=0, Nvenc=1, Qsv=2, Amf=3, VideoToolbox=4, Vaapi=5, Vulkan=6, MediaFoundation=7, Other=99` | 编码器类型 |
| `HardwareAccelerationMode` | `None=0` / `Auto=1` / `Required=2` | 模板对硬件的依赖 |
| `TemplateType` | `System=0` / `User=1` | 系统预设（不可删）/ 用户模板 |
| `TemplateParameterKind` | `Select=0` / `Number=1` / `Text=2` / `Boolean=3` | 模板参数控件 |
| `TemplateIssueSeverity` | `Info=0` / `Warning=1` / `Error=2` | 模板校验严重度 |

> **移植建议**：数据库/JSON 存储沿用整数（与 LiteDB 一致，便于对照），对外 API 用字符串枚举名
> （可自解释、便于前端调试）。映射表就是上表。

---

## 1. 持久化

### 1.1 有哪些集合 / 表 —— 【必须存储】

`源文件: Data/LiteDbContext.cs:8-15, 44-48`

| 集合名（原实现） | 实体 | 语义 | 大致规模 |
| --- | --- | --- | --- |
| `templates` | `CommandTemplate` | FFmpeg 命令模板（系统预设 + 用户自定义） | 数十条 |
| `tasks` | `ConversionTask` | 转换任务（含日志、进度、结果） | 增长最快，按历史保留天数清理 |
| `batch_tasks` | `BatchTask` | 批次（一次批量提交，N 个子任务） | 与 tasks 同量级 |
| `encoders` | `HardwareEncoder` | 硬件编码器检测结果缓存 | 十几条 |
| `settings` | `AppSettings` | **单条**文档（全局设置） | 1 条 |

集合名是稳定的字面量（`"templates"` / `"tasks"` / `"batch_tasks"` / `"encoders"` / `"settings"`）。

### 1.2 索引 —— 【必须存储】语义

`源文件: Data/LiteDbContext.cs:50-69`

| 集合 | 索引字段 | 唯一 | 支撑的查询 |
| --- | --- | --- | --- |
| templates | `Name` | ✅ 是 | 模板名唯一（用户模板命名冲突检测） |
| templates | `Type` | 否 | 系统/用户分组 |
| templates | `Category` | 否 | 按分类筛选 |
| tasks | `Status` | 否 | 运行中/等待中/按状态清理与批量删除 |
| tasks | `CreatedAt` | 否 | 历史倒序、按时长清理 |
| tasks | `BatchId` | 否 | 批次内任务列表、批次状态聚合 |
| batch_tasks | `Status` | 否 | 批次筛选 |
| batch_tasks | `CreatedAt` | 否 | 批次清理 |
| encoders | `Name` | ✅ 是 | 编码器名唯一（检测结果 upsert） |
| encoders | `Type` | 否 | 按类型筛选 |

> `settings` 集合**没有**索引（只取第一条）。

### 1.3 【原实现】LiteDB 的存储细节（迁移/兼容时才需要）

`源文件: Data/LiteDbContext.cs:23-42`、`Program.cs:296-324`

- 单文件嵌入式 NoSQL：`data.db`（LiteDB v5）。构造函数里 `new LiteDatabase(dbPath)` 之后立即建索引。
- 文档字段名 = C# 属性名（PascalCase），但 **`Id` 属性映射到 BSON 的 `_id` 字段**（LiteDB 约定）。
- `ObjectId` 是 12 字节值：4B 秒级时间戳（大端）+ 5B 随机 + 3B 递增计数器；
  字符串形式为 24 位小写十六进制。JSON 序列化时呈现为 `{"$oid":"..."}`（扩展 JSON）。
- `DateTime` 以 BSON date 存储（UTC 毫秒精度）；`DateTime?` 为 null/BsonNull。
- `int?`/`double?`/`long?` 可空数值存 null。
- 字符串集合（如 `SupportedCodecs`）存 BSON 数组。
- `Dictionary<string,string>? TemplateParameters` 存 BSON 文档；null 时字段缺失或为 null。
- **写并发**：LiteDB 单文件写入是串行的；进程崩溃时可能残留写日志（重开自动恢复）。
- ⚠️ **启动前读取**：`Program.cs:296-324` 在 DI 容器建立之前，**再打开一次同一个 `data.db`**，
  直接用 `db.GetCollection("settings").FindAll().FirstOrDefault()` 读 `ListenPort`/`AutoOpenBrowser` 两个字段。

> **Rust 侧建议**：用 **SQLite**（`rusqlite`/`sqlx`）替换 LiteDB，理由：单文件、事务、崩溃安全、索引语义一一对应。
> 关键点：**不要复用 `data.db` 这个文件名**（LiteDB 格式），改用 `data.sqlite3`，避免把旧库当 SQLite 打开而损坏。
> 迁移旧数据需要一次性转换器（`litelib`/自写解析），属于独立决策，见附录 B。

### 1.4 数据目录（跨平台路径 + 环境变量覆盖）

`源文件: Services/AppPaths.cs:37-71`；默认数据库文件名 `data.db` 定义在 `Data/LiteDbContext.cs:25-38`。

**数据库位置解析（原实现，两处逻辑必须一致）**：

1. 读配置项 `FFmpegWebUI:DatabasePath`（来源：`appsettings.json` / 环境变量 / 命令行）；
2. 若为空 → `{DataDirectory}/data.db`；
3. 若非空 → `AppPaths.ToAbsolute(值)`，并 `EnsureDirectory(父目录)`。

`源文件: appsettings.json`（`FFmpegWebUI.DatabasePath` 默认 `""`）

**`DataDirectory` 解析优先级**：

| 顺序 | 条件 | 结果 |
| --- | --- | --- |
| 1 | 环境变量 `FFMPEGWEBUI_DATA_DIR` 非空 | `EnsureDirectory(ExpandHome(值))` ——**直接使用，不再追加 `FFmpegWebUI` 子目录** |
| 2 | Windows | `%LOCALAPPDATA%\FFmpegWebUI` |
| 3 | macOS | `~/Library/Application Support/FFmpegWebUI` |
| 4 | Linux | `$XDG_DATA_HOME/FFmpegWebUI`，`XDG_DATA_HOME` 为空则 `~/.local/share/FFmpegWebUI` |
| 5 | 上面 root 为空（极少见） | `{AppContext.BaseDirectory}/FFmpegWebUI`（即 .NET 可执行文件所在目录） |

- `EnsureDirectory` 会尝试 `mkdir -p`，**失败静默**（`AppPaths.cs:104-115`）；目录不可用时留到使用处报错。
- `ExpandHome`：`~`、`~/…`、`~\…` → `Path.GetFullPath(用户主目录 + 剩余部分)`（`AppPaths.cs:89-101`）。
- ⚠️ 注意顺序陷阱：**只要设了 `FFMPEGWEBUI_DATA_DIR`，就不会再有 `FFmpegWebUI` 这一层**。
  默认（不设环境变量）时数据落在 `…/FFmpegWebUI/data.db`。这一差异会影响用户预期的文件位置。

**各平台实际路径示例**：

| OS | 默认数据目录 | 默认数据库文件 |
| --- | --- | --- |
| Windows | `C:\Users\<u>\AppData\Local\FFmpegWebUI` | `…\FFmpegWebUI\data.db` |
| macOS | `/Users/<u>/Library/Application Support/FFmpegWebUI` | `…/FFmpegWebUI/data.db` |
| Linux (XDG 已设) | `$XDG_DATA_HOME/FFmpegWebUI` | `…/data.db` |
| Linux (默认) | `/home/<u>/.local/share/FFmpegWebUI` | `/home/<u>/.local/share/FFmpegWebUI/data.db` |
| 容器 | 同上（建议用 `FFMPEGWEBUI_DATA_DIR=/data` 显式挂载） | `/data/data.db` |

**默认输出目录**（不是数据目录，但一起解析，`AppPaths.cs:74-86`）：
`用户"视频"目录`（存在时）→ 否则 `用户主目录`（存在时）→ 否则系统临时目录。

**环境变量总表**：

| 变量 | 作用 | 读取位置 |
| --- | --- | --- |
| `FFMPEGWEBUI_DATA_DIR` | 覆盖数据目录（不再追加子目录） | `AppPaths.cs:41-45` |
| `FFMPEGWEBUI_BIND` | 覆盖监听地址；**未设置 = 仅回环** | `Program.cs:26-32` |
| `FFMPEGWEBUI_PORT` | 覆盖监听端口（优先于配置与数据库） | `Program.cs:238-239` |
| `FFMPEGWEBUI_NO_BROWSER=1` | 禁用启动后自动打开浏览器 | `Program.cs:252` |
| `XDG_DATA_HOME` | Linux 数据根目录 | `AppPaths.cs:58-61` |
| `DOTNET_RUNNING_IN_CONTAINER` | 容器探测（配合 `/.dockerenv`） | `AppPaths.cs:24-25` |
| `PATH` | 查找 ffmpeg/ffprobe/zenity/kdialog/powershell | `FFmpegLocator.cs:216-234`、`FileDialogService.cs:248-263` |

### 1.5 哪些字段必须持久化、哪些可重算

| 数据 | 必须持久化？ | 说明 |
| --- | --- | --- |
| `AppSettings` 全部字段 | ✅ 是 | 用户配置，唯一一条；缺失时用默认值并落库 |
| `ConversionTask` 全部字段（除下面标注） | ✅ 是 | 任务历史/重试/日志 |
| `ConversionTask.LogOutput` | ✅ 是 | 有行数上限，会被截断（§3.8） |
| `ConversionTask.EstimatedTimeRemaining` / `ProcessingSpeed` | ⚠️ 半持久化 | 运行中会写；**任务结束时被置 null**（`ConversionQueue.cs:457-458`、`799-800`） |
| `ConversionTask.Progress` / `CurrentTime` | ✅ 是 | 节流写入，见 §3.7/§3.8 |
| `ConversionTask.OutputFileSize` | ✅ 是 | 仅成功时填充 |
| `BatchTask.*` | ✅ 是 | 由子任务聚合重算（`ConversionQueue.cs:814-849`） |
| `HardwareEncoder.*` | ✅ 是（缓存） | 可用 `FFmpegVersion` 失效；不可用可重算但会耗时（试跑编码器） |
| `CommandTemplate.*` | ✅ 是 | 系统模板由 `TemplateService` 按版本同步 |
| `FFmpegInfo` / `MediaInfo` / `MediaStreamInfo` | ❌ 否 | 探测结果，内存缓存（`FFmpegInfo` 缓存 30 分钟，`FFmpegService.cs:153`） |
| `AppStatus` | ❌ 否 | 计算快照，10 秒缓存（§8） |
| 队列内容（待跑任务 id 列表） | ❌ 否 | 内存 `Channel`；崩溃后由 `Pending` 状态重建（⚠️ 见 §3.3 与 §3.13 的行为差异） |
| 正在运行的 ffmpeg 进程句柄 | ❌ 否 | 内存映射 `ObjectId → Process` |
| 日志节流缓冲 `_pendingLogs` | ❌ 否 | 内存；进程退出时强制落库（`finally`，`ConversionQueue.cs:340`） |

### 1.6 【Rust 侧建议】SQLite schema

```sql
-- 全局设置：单行（id 固定 'settings' 或任意 24hex）
CREATE TABLE settings (
  id                          TEXT PRIMARY KEY,
  ffmpeg_path                 TEXT    NOT NULL DEFAULT '',
  ffprobe_path                TEXT    NOT NULL DEFAULT '',
  default_output_directory    TEXT    NOT NULL DEFAULT '',
  output_naming               INTEGER NOT NULL DEFAULT 1,   -- OutputNamingRule
  output_suffix               TEXT    NOT NULL DEFAULT '_converted',
  file_exists_action          INTEGER NOT NULL DEFAULT 0,   -- FileExistsAction
  prefer_hardware_acceleration INTEGER NOT NULL DEFAULT 1,
  prefer_hardware_decoding    INTEGER NOT NULL DEFAULT 0,
  preferred_hardware_encoder  TEXT    NOT NULL DEFAULT '',
  auto_fallback_to_software   INTEGER NOT NULL DEFAULT 1,
  max_concurrent_tasks        INTEGER NOT NULL DEFAULT 1,
  max_log_lines               INTEGER NOT NULL DEFAULT 500,
  task_history_retention_days INTEGER NOT NULL DEFAULT 30,
  theme                       TEXT    NOT NULL DEFAULT 'System',
  global_ffmpeg_arguments     TEXT    NOT NULL DEFAULT '',
  notify_on_completion        INTEGER NOT NULL DEFAULT 1,
  auto_open_browser           INTEGER NOT NULL DEFAULT 1,
  listen_port                 INTEGER NOT NULL DEFAULT 5265
);

CREATE TABLE templates (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  description   TEXT NOT NULL DEFAULT '',
  command_args  TEXT NOT NULL DEFAULT '',
  type          INTEGER NOT NULL DEFAULT 1,
  category      TEXT NOT NULL DEFAULT '',
  supported_input_formats TEXT NOT NULL DEFAULT '[]',  -- JSON 数组
  output_extension        TEXT NOT NULL DEFAULT '',
  target_codec            TEXT NOT NULL DEFAULT '',
  requires_hardware_acceleration INTEGER NOT NULL DEFAULT 0,
  hardware_mode           INTEGER NOT NULL DEFAULT 0,
  required_encoder        TEXT,
  parameters              TEXT NOT NULL DEFAULT '[]',  -- JSON 数组
  required_filters        TEXT NOT NULL DEFAULT '[]',
  tags                    TEXT NOT NULL DEFAULT '[]',
  is_recommended          INTEGER NOT NULL DEFAULT 0,
  preset_version          INTEGER NOT NULL DEFAULT 0,
  created_at              INTEGER NOT NULL,             -- Unix ms UTC
  updated_at              INTEGER NOT NULL,
  sort_order              INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX ix_templates_name     ON templates(name);
CREATE INDEX        ix_templates_type     ON templates(type);
CREATE INDEX        ix_templates_category ON templates(category);

CREATE TABLE batch_tasks (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL DEFAULT '',
  template_id     TEXT NOT NULL,
  total_files     INTEGER NOT NULL DEFAULT 0,
  completed_files INTEGER NOT NULL DEFAULT 0,
  failed_files    INTEGER NOT NULL DEFAULT 0,
  status          INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER NOT NULL,
  completed_at    INTEGER
);
CREATE INDEX ix_batch_status     ON batch_tasks(status);
CREATE INDEX ix_batch_created_at ON batch_tasks(created_at);

CREATE TABLE tasks (
  id                    TEXT PRIMARY KEY,
  input_path            TEXT    NOT NULL,
  output_path           TEXT    NOT NULL,
  template_id           TEXT    NOT NULL,
  template_name         TEXT    NOT NULL DEFAULT '',
  actual_command        TEXT    NOT NULL DEFAULT '',
  status                INTEGER NOT NULL DEFAULT 0,
  progress              REAL    NOT NULL DEFAULT 0,
  total_duration        REAL    NOT NULL DEFAULT 0,
  current_time          REAL    NOT NULL DEFAULT 0,
  estimated_time_remaining REAL,
  processing_speed      REAL,
  log_output            TEXT    NOT NULL DEFAULT '',
  error_message         TEXT,
  created_at            INTEGER NOT NULL,
  started_at            INTEGER,
  completed_at          INTEGER,
  input_file_size       INTEGER NOT NULL DEFAULT 0,
  output_file_size      INTEGER,
  batch_id              TEXT,
  template_parameters   TEXT,             -- JSON 对象；NULL = 无
  retry_of_task_id      TEXT,
  output_path_resolved  INTEGER NOT NULL DEFAULT 0,
  overwrite_existing    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX ix_tasks_status     ON tasks(status);
CREATE INDEX ix_tasks_created_at ON tasks(created_at);
CREATE INDEX ix_tasks_batch_id   ON tasks(batch_id);

CREATE TABLE encoders (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  display_name  TEXT NOT NULL DEFAULT '',
  type          INTEGER NOT NULL DEFAULT 0,
  family        TEXT NOT NULL DEFAULT '',
  codec         TEXT NOT NULL DEFAULT '',
  is_available  INTEGER NOT NULL DEFAULT 0,
  supports_quality_options INTEGER NOT NULL DEFAULT 1,
  supported_codecs TEXT NOT NULL DEFAULT '[]',
  last_checked_at  INTEGER NOT NULL,
  unavailable_reason TEXT,
  missing_from_build INTEGER NOT NULL DEFAULT 0,
  ffmpeg_version TEXT NOT NULL DEFAULT ''
);
CREATE UNIQUE INDEX ix_encoders_name ON encoders(name);
CREATE INDEX        ix_encoders_type ON encoders(type);
```

> SQLite 用 TEXT 存 id 的 24 位十六进制（和原实现一致，前端无需改格式）；时间统一存
> **Unix 毫秒 UTC（INTEGER）**，可在 API 层转成 RFC3339。
> `tasks.created_at` 的 ms 精度与 LiteDB 的 BSON date 完全一致。

---

## 2. 领域模型：每个字段、类型、默认值与含义

> 通用约定：所有 `Id` 为 `ObjectId`（12 字节，24hex）。所有 `DateTime` 默认值都是 **`DateTime.UtcNow`**，
> 唯一的例外在 §2.5/§2.6 里逐条标注。C# `string` 非可空 → 存储层不允许 NULL（用 `''` 表示空）；
> `string?` → 允许 NULL/缺失。

### 2.1 枚举完整定义

`源文件: Models/Enums.cs:4-107`

```jsonc
// TemplateType
{ "System": 0, "User": 1 }                     // System 不可删除，随应用版本同步

// TaskStatus  —— 见 §3 的完整状态机
{ "Pending": 0, "Running": 1, "Completed": 2, "Failed": 3, "Cancelled": 4, "Skipped": 5 }

// EncoderType
{ "Software": 0, "Nvenc": 1, "Qsv": 2, "Amf": 3, "VideoToolbox": 4,
  "Vaapi": 5, "Vulkan": 6, "MediaFoundation": 7, "Other": 99 }

// HardwareAccelerationMode
{ "None": 0, "Auto": 1, "Required": 2 }        // Required：拿不到该族编码器 → 任务直接失败

// OutputNamingRule
{ "Original": 0, "Suffix": 1 }

// FileExistsAction （见 §4.1）
{ "Ask": 0, "Overwrite": 1, "Skip": 2, "Rename": 3 }

// TemplateParameterKind
{ "Select": 0, "Number": 1, "Text": 2, "Boolean": 3 }

// TemplateIssueSeverity
{ "Info": 0, "Warning": 1, "Error": 2 }
```

**语义要点**（逐条来自源码注释，必须保留）：

- `Skipped`：输出文件已存在且设置为跳过（**不是错误**，计入"完成"用于批次判定）。
- `HardwareAccelerationMode.Required`：模板指定族的硬件编码器不可用时**不允许**偷偷用别的编码器；
  必须报错（`ConversionQueue.cs:539-551`）。理由：用户按模板名预期输出特性，改名不如失败。
- `FileExistsAction.Ask`：单文件转换时在界面确认，**批量处理时自动降级为重命名**（`Enums.cs:75`）。

### 2.2 AppSettings（全局设置，单条）

`源文件: Models/AppSettings.cs:6-84`（字段 8-65，`Normalize()` 68-83）

| 字段（C#） | 类型 | 默认值 | 含义 | 约束（Normalize） |
| --- | --- | --- | --- | --- |
| `Id` | ObjectId | 新生成 | 主键 | — |
| `FFmpegPath` | string | `""` | ffmpeg 可执行文件路径；空 = 自动探测 | trim + 去掉首尾 `"` |
| `FFprobePath` | string | `""` | ffprobe 路径；空 = 自动探测 | trim + 去掉首尾 `"` |
| `DefaultOutputDirectory` | string | 系统"视频"目录（见 §1.4） | 默认输出目录 | trim |
| `OutputNaming` | OutputNamingRule | `Suffix`(1) | 命名规则 | — |
| `OutputSuffix` | string | `"_converted"` | 后缀（`OutputNaming=Suffix` 时使用） | 空白 → `_converted`；trim |
| `FileExistsAction` | FileExistsAction | `Ask`(0) | 输出已存在时的处理 | — |
| `PreferHardwareAcceleration` | bool | `true` | 优先硬件编码 | — |
| `PreferHardwareDecoding` | bool | `false` | 优先硬件解码（输入侧 `-hwaccel`） | — |
| `PreferredHardwareEncoder` | string | `""` | 用户指定的首选编码器名；空 = 自动 | trim |
| `AutoFallbackToSoftware` | bool | `true` | 硬件初始化失败时自动回退软件编码 | — |
| `MaxConcurrentTasks` | int | `1` | 同时执行的任务上限 | 越界 → `1`（合法 1..16） |
| `MaxLogLines` | int | `500` | 单任务保留日志行数 | 越界 → `500`（合法 50..20000） |
| `TaskHistoryRetentionDays` | int | `30` | 历史保留天数，0 = 不清理 | 越界 → `30`（合法 0..3650） |
| `Theme` | string | `"System"` | UI 主题 | 非 `Light/Dark/System` → `System` |
| `GlobalFFmpegArguments` | string | `""` | 追加到 ffmpeg 的全局参数（如 `-threads 4`） | trim |
| `NotifyOnCompletion` | bool | `true` | 完成后浏览器通知 | — |
| `AutoOpenBrowser` | bool | `true` | 启动时自动开浏览器（仅发布版） | — |
| `ListenPort` | int | `5265` | 监听端口，0 = 自动挑空闲端口 | 越界 → `5265`（合法 0..65535） |

⚠️ **两套并发上限**：模型 `Normalize()` 允许 `1..16`，而**设置页表单校验**只允许 `1..8`
（`Components/Pages/Settings.razor:1162`）。移植时需二选一并写清（建议统一为 1..16，或把 UI 上限与模型对齐）。

`Normalize()` 是幂等的原地修改，返回 `this`。所有读取路径都会调用它
（`SettingsService` 读取/保存、`ConversionQueue` 每次取设置、`FFmpegLocator`）。

### 2.3 ConversionTask（转换任务）

`源文件: Models/ConversionTask.cs:6-86`

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `Id` | ObjectId | 新生成 | 主键；前端用它做路由/事件键 |
| `InputPath` | string | `""` | 输入文件**绝对路径**（创建时 `AppPaths.ToAbsolute`） |
| `OutputPath` | string | `""` | 输出文件路径；执行期会被冲突解析结果覆盖 |
| `TemplateId` | ObjectId | `ObjectId.Empty` | 模板 ID（`000000000000000000000000`） |
| `ActualCommand` | string | `""` | 实际执行的 ffmpeg 参数串（**不含可执行文件名**，由 `CommandLineBuilder` 生成；回退重试时会重写） |
| `Status` | TaskStatus | `Pending` | 任务状态 |
| `Progress` | double | `0` | 进度百分比 0..100 |
| `TotalDuration` | double | `0` | 输入总时长（秒），来自 ffprobe |
| `CurrentTime` | double | `0` | 当前处理到的时间（秒） |
| `EstimatedTimeRemaining` | double? | null | 预估剩余秒数（结束时置 null） |
| `ProcessingSpeed` | double? | null | 倍速（结束时置 null） |
| `LogOutput` | string | `""` | 日志文本（多行；有行数上限，见 §3.8） |
| `ErrorMessage` | string? | null | 失败原因（中文，见 §3.4 的错误文案表） |
| `CreatedAt` | DateTime | `UtcNow` | 创建时间 |
| `StartedAt` | DateTime? | null | 开始执行时间 |
| `CompletedAt` | DateTime? | null | 结束时间（含 Skipped/Cancelled/Failed） |
| `InputFileSize` | long | `0` | 输入文件字节数 |
| `OutputFileSize` | long? | null | 输出字节数，**仅成功时**填充 |
| `BatchId` | ObjectId? | null | 所属批次 |
| `TemplateParameters` | Dictionary<string,string>? | null | 创建任务时用户选定的参数（重试时复用） |
| `TemplateName` | string | `""` | 模板名快照（模板被删后仍可展示） |
| `RetryOfTaskId` | ObjectId? | null | 本任务由哪个任务重试而来（UI 区分） |
| `OutputPathResolved` | bool | `false` | **输出路径已由界面完成冲突解析**；true 时执行层不再套用全局策略 |
| `OverwriteExisting` | bool | `false` | 是否允许覆盖（仅 `OutputPathResolved=true` 时有意义） |
| `DurationSeconds` | double?（**计算属性，不存储**） | — | `StartedAt` 与 `CompletedAt` 都存在时 = `(CompletedAt-StartedAt).TotalSeconds`，否则 null |

**`OutputPathResolved` / `OverwriteExisting` 的组合语义（移植必须照抄）**
`源文件: Services/TaskService.cs:73-86`、`Services/ConversionQueue.cs:172-194`

- 创建任务时若调用方传入 `overwriteExisting`（单文件转换路径总会传）：
  `OutputPathResolved = true`，`OverwriteExisting = 传入值`。
- 批量创建任务时**不传**：`OutputPathResolved = false`，`OverwriteExisting = true`（默认）→ 执行层重新按
  `AppSettings.FileExistsAction` 解析路径。
- 执行层：`OutputPathResolved=true` → 用任务自己的 `OutputPath`，是否覆盖看 `OverwriteExisting`，
  **但仍会做"输出 == 输入"安全检查**（命中则强制改名，见 §4.1）。

### 2.4 BatchTask（批量任务）

`源文件: Models/BatchTask.cs:6-33`

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `Id` | ObjectId | 新生成 | 主键 |
| `Name` | string | `""` | 批次名，创建时固定为 `批量任务 - {yyyy-MM-dd HH:mm}`（**本地时间**，`TaskService.cs:106`） |
| `TemplateId` | ObjectId | `Empty` | 批次内所有子任务共用的模板 |
| `TotalFiles` | int | `0` | 子任务总数；每次子任务状态变更后重算 |
| `CompletedFiles` | int | `0` | 状态为 `Completed` 的数量 |
| `FailedFiles` | int | `0` | 状态为 `Failed` 或 `Cancelled` 的数量（见 §3.11） |
| `Status` | TaskStatus | `Pending` | 批次状态（只出现 `Pending`/`Running`/`Completed`/`Failed`） |
| `CreatedAt` | DateTime | `UtcNow` | 创建时间 |
| `CompletedAt` | DateTime? | null | 全部子任务结束时写入（`??=` 只写一次） |

### 2.5 HardwareEncoder / HardwareDetectionReport（检测缓存）

`源文件: Models/HardwareEncoder.cs:6-87`

**`HardwareEncoder`**（落库到 `encoders`）：

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `Id` | ObjectId | 新生成 | 主键 |
| `Name` | string | `""` | 编码器名，如 `h264_nvenc`（唯一索引） |
| `DisplayName` | string | `""` | 展示名，如 `NVIDIA NVENC H.264` |
| `Type` | EncoderType | `Software` | 类型 |
| `Family` | string | `""` | 厂商标识：`nvenc/qsv/amf/videotoolbox/vaapi/vulkan/mf/software` |
| `Codec` | string | `""` | 目标编码格式：`h264/hevc/av1/vp9/…` |
| `IsAvailable` | bool | `false` | 是否可用（试跑通过） |
| `SupportsQualityOptions` | bool | `true` | 是否通过了带质量参数的完整试跑；false = 只能用最简参数 |
| `SupportedCodecs` | List\<string\> | `[]` | 兼容旧字段 |
| `LastCheckedAt` | DateTime | `UtcNow` | 最后检测时间 |
| `UnavailableReason` | string? | null | 不可用原因 |
| `MissingFromBuild` | bool | `false` | ffmpeg 构建里根本没这个编码器 |
| `FFmpegVersion` | string | `""` | 检测时用的 ffmpeg 版本，用于缓存失效判断 |
| `IsHardware` | bool（**计算**） | — | `Type != Software` |

**`HardwareDetectionReport`**（⚠️ **不落库**，内存对象）：

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `Encoders` | List\<HardwareEncoder\> | `[]` | 全部编码器（含不可用） |
| `DetectedAt` | DateTime | `UtcNow` | 检测时间 |
| `FFmpegVersion` | string | `""` | ffmpeg 版本 |
| `FFmpegPath` | string | `""` | ffmpeg 路径 |
| `Platform` | string | `""` | 平台名 |
| `Hwaccels` | List\<string\> | `[]` | ffmpeg 编译支持的硬件解码方式 |
| `SupportedFilters` | List\<string\> | `[]` | 可用滤镜（判断模板可执行性） |
| `Summary` | string | `""` | 整体提示文本 |
| `AvailableHardwareEncoders` | List（计算） | — | `IsAvailable && IsHardware` |
| `HasHardwareAcceleration` | bool（计算） | — | 是否存在任一可用硬件编码器 |
| `HasFilter(name)` | bool（方法） | — | `SupportedFilters` 为空视为"全部支持"，否则忽略大小写查找 |

### 2.6 CommandTemplate / TemplateParameter / TemplateParameterOption

`源文件: Models/CommandTemplate.cs:6-137`

> 本表虽然不是任务要求的必读文件之一，但 `ConversionQueue`（§3.6）与
> 输出命名（§4.8）都依赖它，必须一并移植。

**`CommandTemplate`**（`templates`）：

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `Id` | ObjectId | 新生成 | 主键 |
| `Name` | string | `""` | 名称（唯一） |
| `Description` | string | `""` | 描述 |
| `CommandArgs` | string | `""` | ffmpeg 参数模板（支持占位符，见 §4.10） |
| `Type` | TemplateType | `User` | 系统/用户 |
| `Category` | string | `""` | 分类（视频转码/音频提取/压缩/分辨率/硬件加速…） |
| `SupportedInputFormats` | List\<string\> | `[]` | 支持的输入格式（空 = 不限） |
| `OutputExtension` | string | `""` | 默认输出扩展名（**不含点**） |
| `TargetCodec` | string | `""` | 目标视频编码（h264/hevc/av1/vp9/vp8/prores），用于挑编码器 |
| `RequiresHardwareAcceleration` | bool | `false` | 是否需要硬件加速（旧字段） |
| `HardwareMode` | HardwareAccelerationMode | `None` | 硬件策略 |
| `RequiredEncoder` | string? | null | 需要的编码器：族名（`nvenc`…）或具体名（`h264_nvenc`） |
| `Parameters` | List\<TemplateParameter\> | `[]` | 可调参数定义 |
| `RequiredFilters` | List\<string\> | `[]` | 必须存在滤镜（如 `subtitles`/`drawtext`） |
| `Tags` | List\<string\> | `[]` | 搜索标签 |
| `IsRecommended` | bool | `false` | 推荐优先展示 |
| `PresetVersion` | int | `0` | 系统模板预设版本，用于随应用升级同步 |
| `CreatedAt` | DateTime | `UtcNow` | 创建时间 |
| `UpdatedAt` | DateTime | `UtcNow` | 最后修改时间 |
| `SortOrder` | int | `0` | 排序权重（越小越靠前） |
| `InputFormatsDisplay` | string（计算） | — | 空 → `任意格式`，否则 ` / ` 连接 |

**`TemplateParameter`**：`Name`（占位符名，不含花括号）、`Label`、`Description`、
`Kind`(默认 `Select`)、`DefaultValue`(`""`)、`Options`(`[]`)、`Min`/`Max`(double?，null)、
`TrueValue`(默认 `"1"`)、`FalseValue`(默认 `""` = 移除该片段)。

**`TemplateParameterOption`**：`Value` / `Label` / `Description`（各字段默认 `""`），另有一个
`(value, label, description = "")` 构造函数 —— 后者只是 C# 语法糖，JSON 里它就是普通对象
`{ "value": "…", "label": "…", "description": "…" }`。

### 2.7 DTO（不落库，仅进程内/接口用）

`源文件: Models/DTOs.cs:6-137`

| 类型 | 字段与类型 | 备注 |
| --- | --- | --- |
| `FFmpegInfo`（record） | `Version:string`, `Path:string`, `Configuration:string`, `SupportedFormats/Codecs/Encoders/Hwaccels/Filters:List<string>`, `MajorVersion:int`(init), `IsAutoDetected:bool`(init) | `IsAvailable` 计算属性：`Version` 非空且 != `"Unknown"` |
| `MediaStreamInfo`（record） | `Index:int`, `CodecType:string`, `CodecName:string?`, `CodecLongName:string?`, `Width:int`, `Height:int`, `FrameRate:double`, `Duration:double`, `Channels:int`, `SampleRate:int`, `BitRate:long`, `Rotation:int`, `PixelFormat:string?` | 来自 `ffprobe -show_streams` |
| `MediaInfo`（record） | 位置参数：`FilePath`, `Duration`, `Format`, `VideoCodec?`, `AudioCodec?`, `Width`, `Height`, `FrameRate`, `FileSize`；init：`FormatDisplay`(默认 `"unknown"`), `BitRate:long`, `AudioChannels:int`, `AudioSampleRate:int`, `Rotation:int`, `PixelFormat:string?`, `HasVideo:bool`, `HasAudio:bool`, `SubtitleStreamCount:int`, `Streams:List<MediaStreamInfo>` | `DurationDisplay` 计算：时长 ≤0 → `-`；≥1h → `h:mm:ss`；否则 `m:ss` |
| `ConversionProgress`（record） | `Percentage`, `CurrentTime`, `TotalDuration`, `Speed:double?`, `Eta:double?`, `RawOutput:string` | ffmpeg 进度块解析结果 |
| `TaskProgressEventArgs`（record） | `TaskId:ObjectId`, `Progress:double`, `Speed:double?`, `Eta:double?` | **事件负载 1**（§3.14） |
| `TaskStatusEventArgs`（record） | `TaskId`, `OldStatus:TaskStatus`, `NewStatus:TaskStatus`, `ErrorMessage:string?` | **事件负载 2** |
| `TaskLogEventArgs`（record） | `TaskId`, `Line:string` | **事件负载 3** |
| `TemplateIssue`（record） | `Severity`, `Message`, `Field:string?`（默认 null） | 模板校验问题 |
| `EncoderCheckResult`（record） | `EncoderName`, `IsAvailable`, `Reason:string?`, `Output:string?` | 编码器可用性检查 |
| `TemplateTestResult`（record） | `Success:bool`, `Message:string`, `Command:string`, `Output:string`, `Duration:TimeSpan` | 模板试跑结果 |

`TemplateTestResult.Duration` 是 `TimeSpan`：JSON 里 .NET 序列化成 `"00:00:12.3450000"`（`c` 格式）。
**Rust 侧建议**统一为毫秒数字（`durationMs`），见 §10。

### 2.8 存储 JSON 形状（【必须数据】的可读形式）

以 `tasks` 的一条文档为例（键名用本文的 snake_case 表示"数据字段"，**不等于** LiteDB 的 PascalCase / `_id`）：

```json
{
  "id": "66f1c0a2b3d4e5f60718293a",
  "input_path": "/Users/me/Movies/a.mp4",
  "output_path": "/Users/me/Movies/a_converted.mp4",
  "template_id": "66f1c0a2b3d4e5f607182930",
  "template_name": "推荐 · H.264 高质量",
  "actual_command": "-hide_banner -y -progress pipe:2 -i /Users/me/Movies/a.mp4 -c:v libx264 ... /Users/me/Movies/a_converted.mp4",
  "status": 2,
  "progress": 100,
  "total_duration": 123.456,
  "current_time": 123.456,
  "estimated_time_remaining": null,
  "processing_speed": null,
  "log_output": "提示：…\nframe=1234  bitrate=…  speed=2.1x  time=00:02:03.45",
  "error_message": null,
  "created_at": "2026-02-14T03:12:44.123Z",
  "started_at": "2026-02-14T03:12:45.001Z",
  "completed_at": "2026-02-14T03:13:20.456Z",
  "input_file_size": 104857600,
  "output_file_size": 52428800,
  "batch_id": null,
  "template_parameters": { "crf": "20", "preset": "medium" },
  "retry_of_task_id": null,
  "output_path_resolved": true,
  "overwrite_existing": false
}
```

> **LiteDB 实际落盘的差异**（仅兼容性相关）：键名是 PascalCase 且主键键名是 `_id`；
> `DateTime` 是 BSON date（无时区后缀，UTC 毫秒）；`ObjectId` 是 BSON ObjectId 类型。

---

## 3. ConversionQueue：执行引擎的完整生命周期

`源文件: Services/ConversionQueue.cs:1-884`（单例，实现 `IAsyncDisposable`）

### 3.1 内部状态容器

`源文件: ConversionQueue.cs:18-49, 69-90`

| 成员 | 类型 | 作用 |
| --- | --- | --- |
| `_queue` | `Channel<ObjectId>`（无界，`SingleReader=false, SingleWriter=false`） | 待执行任务 id 队列；`Reader.Count` 作为"排队中"数量 |
| `_running` | `ConcurrentDictionary<ObjectId, RunningTaskState>` | 正在执行的任务状态；`Count` = 运行中数量 |
| `_cancellations` | `ConcurrentDictionary<ObjectId, CancellationTokenSource>` | 取消令牌 |
| `_pendingLogs` | `ConcurrentDictionary<ObjectId, string>` | 未落库的日志缓冲 |
| `_lastPersisted` | `ConcurrentDictionary<ObjectId, DateTime>` | 上次日志落库时间（节流用） |
| `_workers` / `_workerCount` / `_workerGate` | `List<Task>` / `volatile int` / `Lock` | worker 池，**只增不减** |
| 公开事件 | `TaskProgressChanged` / `TaskStatusChanged` / `TaskLogAppended` | §3.14 |
| 公开属性 | `QueuedCount`（= `_queue.Reader.Count`）/ `ActiveCount`（= `_running.Count`） | 状态栏/健康检查用 |

```csharp
// RunningTaskState（ConversionQueue.cs:37-49）—— 每个运行中任务的附加内存态
class RunningTaskState {
    ConversionTask Task;              // 数据库实体引用（就地修改）
    CommandTemplate? Template;
    EncoderProfile?  Encoder;         // 回退到软件编码时会替换
    bool Overwrite;                   // 决定 ffmpeg 加 -y 还是 -n
    DateTime StartedAt;
    bool HardwareDecodeDisabled;      // 硬件解码失败后置 true，重试不再注入 -hwaccel
    bool ProducedAnyOutput;           // 本次尝试是否产出过数据（⚠️ 见 §3.6）
}
```

### 3.2 入队（enqueue）

`源文件: ConversionQueue.cs:96-106`（由 `TaskService.StartTaskAsync` 调用，`TaskService.cs:150-172`）

1. 调用方先**把任务重置为 `Pending`**：`Progress=0, CurrentTime=0, ETA=null, Speed=null,
   ErrorMessage=null, CompletedAt=null, LogOutput=""`，然后写库（`TaskService.cs:160-169`）。
   `StartTaskAsync` 只接受当前状态为 `Pending / Failed / Cancelled / Skipped` 的任务；`Running` 返回 false。
2. `Queue.Enqueue(taskId)`：`_queue.Writer.TryWrite(taskId)`；失败（通道已关闭）→ 记 warning 日志、返回 false。
   成功 → `EnsureWorkers()`，返回 true。
3. 入队**不阻塞**，立即返回。UI 收到 `true` 后提示"已开始转换"。

⚠️ **没有去重**：同一个 `taskId` 被入队两次会被两个 worker 各跑一次（`ProcessTaskAsync` 只检查
状态是否为 `Pending|Running`，`Running` 也会通过）。移植时必须自己维护 `in_flight: HashSet<Id>`。

### 3.3 并发控制与 worker 循环

`源文件: ConversionQueue.cs:108-143`

- `EnsureWorkers()`：`desired = clamp(settings.MaxConcurrentTasks, 1, 16)`；
  在 `_workerGate` 锁内 `while (_workerCount < desired) { _workerCount++; _workers.Add(Task.Run(WorkerLoopAsync)); }`。
  - **惰性**：worker 在第一次入队时才创建。
  - **只增不减**：之后把 `MaxConcurrentTasks` 调小**不会**销毁已有 worker，只是让它们抢同一个通道。
    （⚠️ 用户改小并发后实际并发仍可能更高，直到重启。）
  - ⚠️ 这里用了 `GetAwaiter().GetResult()` 同步等待异步设置读取（阻塞线程）。
- `WorkerLoopAsync()`：`await foreach (var taskId in _queue.Reader.ReadAllAsync())` →
  `ProcessTaskAsync(taskId)`；单任务抛异常 → 记 error 日志 + `TryMarkFailed(taskId, ex.Message)`。
  循环自身异常 → 记 error 并**退出该 worker**（不会重建，实际并发会衰减）。

> **移植建议**：用 `tokio::sync::mpsc` + 固定 `Semaphore`（或 `for` 起 N 个常驻 task）实现；
> 并发上限变化时**热调整**（推荐）或至少在 UI 明示需重启。

### 3.4 单任务执行生命周期

`源文件: ConversionQueue.cs:149-342`（`ProcessTaskAsync`）

按顺序（每一步的失败出口都标注了）：

| # | 步骤 | 失败/分支处理 |
| --- | --- | --- |
| 1 | 从库读任务（`FindById`）；不存在 → 直接返回 | — |
| 2 | 状态不是 `Pending`/`Running` → 返回（**已被取消/跳过的任务不执行**） | — |
| 3 | 读设置（`Normalize()`）与模板（`TemplateService.GetTemplateByIdAsync`） | 模板不存在 → `Failed`，错误文案 `模板不存在，可能已被删除。` |
| 4 | ffprobe 探测媒体信息（`GetMediaInfoAsync`），失败返回 null 不致命 | — |
| 5 | **输出冲突解析**（§3.5） | `resolution.Skip` → 状态置 `Skipped`，`CompletedAt=now`、`Progress=0`、`LogOutput=Note`、`OutputPath=resolution.OutputPath`，落库并发事件后返回 |
| 6 | 写回 `OutputPath`、`TotalDuration`（来自 ffprobe）、`InputFileSize` | — |
| 7 | 确保输出目录存在（`mkdir -p`） | 异常 → `Failed`，文案 `无法创建输出目录：{异常}` |
| 8 | 可写性探测（写临时文件，§4.4） | false → `Failed`，文案 `输出目录不可写：{目录}` |
| 9 | 解析编码器 `ResolveEncoderAsync`（§3.6） | `InvalidOperationException`（严格模式缺硬件）→ `Failed`，用异常消息 |
| 10 | 构建命令行 `BuildCommand`（§3.6，含 `-hwaccel` 注入） | — |
| 11 | 写库：`ActualCommand`、`Status=Running`、`StartedAt=now`、`ErrorMessage=null`、`Progress=0`、`LogOutput=""` | — |
| 12 | 若有命令构建警告 → `AppendLog("提示：" + 用 `；` 连接的 warnings)`；若有冲突说明 → 追加日志 | — |
| 13 | 发 `TaskStatusChanged(old=Pending → Running)` | ⚠️ old 是**硬编码的 `Pending`**，不是库里读到的旧值 |
| 14 | 建 `RunningTaskState`，登记 `_running[id]`、`_cancellations[id]=new CTS` | — |
| 15 | 第 1 次执行 `RunOnceAsync`（§3.7） | 见 §3.6 的回退链 |
| 16 | 回退链：先关硬件解码重试，仍失败再换软件编码（§3.6） | — |
| 17 | `FinalizeTask`（§3.4.1） | — |
| 18 | `finally`：从 `_running`/`_cancellations`/`_pendingLogs`/`_lastPersisted` 移除；`cts.Dispose()`；`FlushLogs(force:true)` | — |

#### 3.4.1 终态写入（`FinalizeTask`）

`源文件: ConversionQueue.cs:418-474`

| 结果 | `Status` | `Progress` | `ErrorMessage` | 追加日志 |
| --- | --- | --- | --- | --- |
| 被取消（`result.Cancelled`） | `Cancelled` | 保持当前值 | `用户取消` | `任务已被取消。` |
| 成功（`result.Success`） | `Completed` | `100` | `null` | — |
| 超时（`result.TimedOut`） | `Failed` | 保持 | `执行超时` | `❌ 转换失败。FFmpeg 输出：` + 诊断尾部 |
| 其它非 0 退出 | `Failed` | 保持 | `FFmpeg 退出码 {ExitCode}` | 同上 |

随后统一：`CompletedAt = now`；`OutputFileSize = 成功 ? 文件大小 : null`。
⚠️ **落库顺序很关键**（源码注释明确写了原因）：
`FlushLogs(force:true)` → **重新 `FindById` 读回最新文档** → 只覆盖上面这些字段并
`TruncateLog(persisted.LogOutput, MaxLogLines)` → `Update`。
（若直接用手里那份可能过期的对象 `Update`，会把刚 flush 进去的日志覆盖掉。）
最后：发 `TaskStatusChanged(oldStatus → newStatus)`，并 `UpdateBatchStatus(BatchId)`。

#### 3.4.2 异常出口

`TryMarkFailed(taskId, message)`（`ConversionQueue.cs:788-812`）：置 `Failed` + `ErrorMessage` +
`CompletedAt=now` + `ETA=null` + `Speed=null`，从 `_running`/`_cancellations` 移除，发状态事件，
更新批次。**日志不清空**（保留已产生的部分）。

### 3.5 输出冲突与"输出 == 输入"安全

`源文件: ConversionQueue.cs:169-230`

```
if task.OutputPathResolved:
    resolution = { OutputPath = task.OutputPath, Overwrite = task.OverwriteExisting,
                   Skip = false, Renamed = false }
    if PathsEqual(resolution.OutputPath, task.InputPath):        // §4.3
        resolution = Rename(resolution.OutputPath)
        resolution.Note = "输出路径与输入文件相同，已自动改名以避免覆盖源文件。"
else:
    resolution = OutputPathResolver.Resolve(task.OutputPath,
                                            settings.FileExistsAction,
                                            task.InputPath)
```

- `resolution.Overwrite` 决定 ffmpeg 命令行前部加 `-y`（true）还是 `-n`（false）——
  **这是防挂起的核心**（`FFmpegService.cs:561-562`）。
- 若 `resolution.Skip`：任务**不会启动 ffmpeg**，直接 `Skipped`（§3.4 步骤 5）。
- `resolution.Note` 一律写入 `LogOutput`（作为第一行提示）。

### 3.6 编码器解析与硬件→软件回退

#### 3.6.1 编码器解析（何时需要编码器）

`源文件: ConversionQueue.cs:519-567`

```
usesEncoderPlaceholder = template.CommandArgs 含 "{encoder}"（忽略大小写）
needsEncoder = usesEncoderPlaceholder || template.HardwareMode != None
if (!needsEncoder) return null            // 模板自带编码器，不做任何注入
codec     = DetermineTargetCodec(template)                 // §3.6.2
required  = template.RequiredEncoder
strict    = template.HardwareMode == Required
name      = HardwareDetection.GetRecommendedEncoderAsync(codec, required, strict)
if (name == null) throw InvalidOperationException(
   $"该模板要求使用 {family} 硬件编码，但本机不可用。" +
   "请改用「推荐」分类下会自动挑选编码器的模板，或在「设置 → 硬件加速能力」中确认本机支持的编码器。")
profile = EncoderCatalog.Find(name) ?? EncoderCatalog.SoftwareFallback(codec) ?? EncoderCatalog.Find("libx264")
if (最近检测报告里该编码器 SupportsQualityOptions == false)
    profile = profile with { QualityArgs = null, PresetArgs = null }   // 退化为最简参数
```
`family` 的取值：`RequiredEncoder` 为空白时用字面量 `"指定的硬件编码器"`，否则用
`EncoderCatalog.FamilyDisplayName(...)` 映射（`nvenc→NVIDIA NVENC`、`qsv→Intel Quick Sync`、
`amf→AMD AMF`、`videotoolbox→Apple VideoToolbox`、`vaapi→Linux VA-API`、`vulkan→Vulkan Video`、
`mf→Windows Media Foundation`、`software→软件编码`）。

#### 3.6.2 目标 codec 推断（`DetermineTargetCodec`）

`源文件: ConversionQueue.cs:569-606`，优先级：

1. `template.TargetCodec` 非空 → `HardwareDetectionService.NormalizeCodec(...)`；
2. 否则 `template.RequiredEncoder` → 查 `EncoderCatalog.Find(name).Codec`，再退到该族第一个编码器的 codec；
3. 否则从 `template.OutputExtension` 归一化，若落在 `h264/hevc/av1/vp9` 则用它；
4. 否则扫描 `CommandArgs` 里是否出现 `libx265, libx264, libsvtav1, libvpx-vp9, libvpx`（**按此顺序**）取对应 codec；
5. 否则按扩展名兜底：`webm→vp9`、`mkv→h264`、`mov→h264`、其它 `→h264`。

#### 3.6.3 硬件解码参数注入

`源文件: ConversionQueue.cs:624-638`

仅当 `settings.PreferHardwareDecoding == true` 且本次尝试未禁用硬件解码 且 ffprobe 探测到视频流时注入
（即 `hardwareDecodeEnabled`）：

| 平台 | `-hwaccel` 值 |
| --- | --- |
| macOS | `videotoolbox` |
| Windows | `d3d11va` |
| Linux/其它 | `vaapi` |

只有当输入视频 codec ∈ `h264, hevc, av1, vp9, mpeg2video, vc1` 时才注入；
否则返回空串。（不注入 `-hwaccel_output_format`，也不做 `hwupload` —— 上传滤镜由模板/编码器
`UploadFilter` 描述，属 `CommandLineBuilder` 职责。）

#### 3.6.4 回退链（三步）

`源文件: ConversionQueue.cs:279-324` 与判定函数 `380-416`

```
# 第 1 步：关掉硬件解码，保留硬件编码
if (!result.Success && !result.Cancelled && settings.AutoFallbackToSoftware &&
    hardwareDecodeEnabled && ShouldFallbackToSoftware(result, state.ProducedAnyOutput)) {
      日志: "⚠️ 硬件解码失败，正在改用软件解码重试……"
      日志: "失败原因：" + FirstLine(result.Diagnostic)          // 首行、最长 300 字符
      state.HardwareDecodeDisabled = true
      retryBuilt = BuildCommand(..., disableHardwareDecode: true)   // 不再注入 -hwaccel
      task.ActualCommand = retryBuilt.CommandLine ; 落库
      result = await RunOnceAsync(retryBuilt)
}
# 第 2 步：仍在用硬件编码器 → 换软件编码
if (!result.Success && !result.Cancelled && settings.AutoFallbackToSoftware &&
    encoder is { IsHardware: true } && ShouldFallbackToSoftware(result, state.ProducedAnyOutput)) {
      software = EncoderCatalog.SoftwareFallback(encoder.Codec)     // 同 codec 的软件编码器，按 SortPreference 取第一
      if (software != null && software.Name != encoder.Name) {
          日志: $"⚠️ 硬件编码器 {encoder.Name} 执行失败，正在改用软件编码 {software.Name} 重试……"
          日志: "失败原因：" + FirstLine(result.Diagnostic)
          state.Encoder = software ; _running[task.Id] = state
          fallbackBuilt = BuildCommand(..., state.HardwareDecodeDisabled)
          task.ActualCommand = fallbackBuilt.CommandLine ; 落库
          result = await RunOnceAsync(fallbackBuilt)
      }
}
```

`ShouldFallbackToSoftware(result, producedAnyOutput)` 判定规则（`ConversionQueue.cs:380-416`）：

1. `result.TimedOut == true` → **不**回退（超时不是硬件问题）。
2. `producedAnyOutput == false` → **回退**（一帧都没编出来，基本可确定是硬件初始化失败）。
3. 否则只在 `result.Diagnostic` 命中以下关键字之一（**忽略大小写**）时回退：

```
Cannot load / Device creation failed / No capable devices found / Error initializing /
Cannot open device / Failed to initialise / Failed to initialize / No device available /
not available / Function not implemented / Operation not permitted / Invalid argument /
Unrecognized option / Option not found / Error while opening encoder / Could not open encoder /
Generic error in an external library / OpenEncodeSessionEx failed / No NVENC capable devices found /
Error creating an internal MFX session / unsupported / MFX_ERR
```

⚠️ **已知实现缺陷（移植必须显式决策）**：第 1 次 `RunOnceAsync` 之后、做回退判定**之前**，
代码把 `state.ProducedAnyOutput = false`（第 286 行；第 2 步前第 321 行同理）。
因此判定实际上**永远走"未产出任何数据"分支**（除非超时），关键字白名单形同虚设——
即"跑到一半才失败的硬件编码任务也会被重跑一次"。
**移植建议**：修正为"每轮执行前重置、执行后判定"，并在文档/变更日志里注明这与原实现不同
（否则用户可以观察到"失败后重复执行且日志出现两次 ⚠️ 行"的差异）。

**回退如何记录**：全部写进任务的 `LogOutput`（`⚠️ 硬件解码失败…` / `⚠️ 硬件编码器 X 执行失败…`
+ `失败原因：{诊断首行}`），并**改写 `ActualCommand`** 为最终实际执行的那条命令；
`state.Encoder` 也被替换（供后续逻辑/展示）。**没有独立的字段记录"用了几次回退/最终编码器"**
（如需展示，Rust 侧建议新增 `effective_encoder` 字段，或从日志解析）。

### 3.7 进度上报：来源、频率、内容

`源文件: ConversionQueue.cs:344-374`（`RunOnceAsync`）、`FFmpegService.cs:555-750, 947-1041`

**ffmpeg 调用形态（固定三段前缀）**：

```
{ffmpeg} -hide_banner {-y|-n} -progress pipe:2 {request.Arguments…}
```

- `-progress pipe:2` 让 ffmpeg 把进度**块**写到 stderr；解析器（`ProgressAccumulator`）按
  `key=value` 行累积，遇到 `progress=continue|end` 收尾成一次进度事件。
- 认识的 key：`frame, fps, bitrate, total_size, out_time_us, out_time_ms, out_time,
  dup_frames, drop_frames, speed, progress`；其它行**不算进度**，走 `OnLogLine`。
- **频率由 ffmpeg 决定**（默认为其统计周期，约每 0.5 秒一块）；Rust 侧不要自己造节流，
  但**要自己造 UI 侧的上限**（见下）。
- `out_time` 取值优先级：`out_time_us`（微秒 / 1e6）> `out_time_ms`（**注意：也按 /1e6 处理**，
  因为新版 ffmpeg 的 `out_time_ms` 实际是微秒）> `out_time` 文本（`TimeSpan.Parse` 风格 `HH:MM:SS.fff`）。
- 百分比：`total_duration > 0 ? clamp(current/total*100, 0, 100) : 0`；
  且当块以 `progress=end` 收尾时 **`currentTime` 强制 = `totalDuration`**（保证最后到 100%）。
- ETA：`speed > 0.01 && total > 0 && (total-current) > 0` → `(total-current)/speed`（秒），否则 null。
- speed 解析：去掉尾部 `x/X`，`N/A` → null。
- `RawOutput`（给日志用的一行）：`frame={f}  bitrate={b}  speed={s:0.##}x  time={hh:mm:ss.ff}`，
  空项直接省略（双空格分隔）。

**进度写回任务与事件**（每次进度回调）：

```
task.Progress = percentage ; task.CurrentTime = currentTime
task.ProcessingSpeed = speed ; task.EstimatedTimeRemaining = eta
state.ProducedAnyOutput = true                 // ⚠️ 见 §3.6.4
NotifyProgress(taskId, percentage, speed, eta)  // 事件 → UI
AppendLog(taskId, progress.RawOutput, throttle: true)   // 节流落库（§3.8）
```

> ⚠️ **进度字段几乎不落库（重要且反直觉）**：`RunOnceAsync` 只更新**内存里的任务对象**；
> 而 `FlushLogs` 会重新 `FindById` 出一份新文档、**只写 `LogOutput`**。
> 因此运行期间数据库里的 `Progress / CurrentTime / ProcessingSpeed / EstimatedTimeRemaining`
> 一直停留在开始时的 `0 / 0 / null / null`，**只有 `FinalizeTask`（或 `TryMarkFailed`）才把最终值写回**。
> 后果：(1) 其它页面/进程读数据库看不到实时进度，只能靠事件；(2) 崩溃后无法知道跑到哪里。
> **移植决策**：Rust 侧建议每 1 秒把进度字段一起落库（行为增强，需在变更说明里标注）。

**订阅方（UI）**：页面在内存里维护一份任务列表，事件到达时**原地更新内存对象的字段**再
`InvokeAsync(StateHasChanged)` 触发渲染（`Convert.razor:1219-1228` 只更新
`Progress/ProcessingSpeed/EstimatedTimeRemaining`）。日志事件同理追加到最后若干行。
状态事件走另一条路径：**回读数据库**后提示 Toast（`Convert.razor:1234-1260`）。
**Rust/React 侧必须把这三类事件通过 WebSocket/SSE 推给前端**（§3.14 表、§10.4）。

### 3.8 日志：追加、节流、上限

`源文件: ConversionQueue.cs:733-786`

- `AppendLog(taskId, line, throttle)`：
  1. 追加到内存缓冲 `_pendingLogs`，多行用 **`Environment.NewLine`** 连接
     （Linux/macOS = `\n`，Windows = `\r\n`；⚠️ 跨平台可见差异，移植建议统一 `\n`）；
  2. **落库判定**：`throttle == false` 时**每次都落库**；`throttle == true` 时仅在
     "距上次落库 ≥ 700 ms"或"从未落库"时落库。进度行用 throttle=true，普通 stderr 行不节流。
  3. **无论是否落库**，都立即 `TaskLogAppended` 事件推给 UI（"界面实时可见"）。
- `FlushLogs(taskId, force)`：把缓冲取走（`TryRemove`），读回任务文档，
  `combined = 原 LogOutput + NewLine + pending` → `TruncateLog(combined, settings.MaxLogLines)` → `Update`，
  记录 `_lastPersisted = now`。读设置时同样用 `GetAwaiter().GetResult()`。
- `TruncateLog(log, maxLines)`：按 `\n` 切分；行数 ≤ 上限则原样返回（**注意保留 `\r`**）；
  超出时丢弃最早的行，并在最前面加一行
  `…（已省略早期 {跳过行数} 行日志）` + `Environment.NewLine`。
  `maxLines <= 0` → 不截断。**行数是"上限行数"，不是字符数。**
- 失败诊断：`FFmpegRunResult.Diagnostic` 是 **stderr 尾部最近 200 行**（`BoundedLineBuffer(200)`），
  再被 `Truncate(…, 8000)` 限制为最多 8000 字符（超出时保留尾部，前面加 `…（已截断前部输出）\n`）。

### 3.9 取消：优雅 `q` vs 强制杀（以及 pending 取消）

`源文件: ConversionQueue.cs:645-698`、`FFmpegService.cs:774-838`

| 场景 | 行为 |
| --- | --- |
| 任务 `Pending` | **不碰进程**：直接置 `Cancelled`，`CompletedAt=now`，`ErrorMessage="用户取消"`，落库 + 事件 + 更新批次，返回 true。（该 id 仍在队列里，worker 取到后因状态不是 Pending/Running 而跳过） |
| 任务 `Running` | ① `cts.CancelAsync()`；② `FFmpegService.KillAsync(taskId, graceful)`；③ 若 `graceful` 追加日志 `已发送优雅退出指令（q），FFmpeg 会写完文件尾再退出。`；返回 true |
| 其它状态 | 返回 false（不做事） |

`KillAsync(taskId, graceful)`：

- `graceful=true` → `TryGracefulStop`：向 **stdin 写 `"q"` 并 flush**，然后在后台任务里
  `await process.WaitForExitAsync(5s)`，**5 秒内没退出就 `Kill(entireProcessTree: true)`**。
- `graceful=false` → `TryKillProcess`：直接 `Kill(entireProcessTree: true)`。

⚠️ **交叉影响（务必注意）**：`cts.CancelAsync()` 会触发 `RunAsync` 里注册的回调
（`FFmpegService.cs:613-618`），该回调**无条件调用 `TryGracefulStop`**。所以
`CancelAsync(taskId, graceful:false)` 实际序列是：先发一次 `q`，紧接着立刻强杀。
结果符合"强制取消"的预期，但日志/管道行为会比预期多一次 `q` 写入。移植时应明确：
**取消语义 = {graceful: 只发 q 并等 5s→强杀；force: 立即 kill 进程树}**，不要复制"先 q 再 kill"的偶然顺序。

**退出码语义**（`FFmpegService.cs:713-731`）：

- `success = !cancelled && !timedOut && exitCode == 0`；且"`cancelled` 为真但 exitCode==0"**仍视为失败**。
- `TimedOut` 只在"外部 `FFmpegRunRequest.Timeout` 超时"时置位（$\neq$ 取消）。
  `ConversionQueue` **从不设置 `Timeout`**（null = 不限制），所以超时路径只在被其它调用者使用时出现。
- `skippedExisting`：stderr/stdout 出现 `already exists. Overwrite?` 或 `Not overwriting - exiting` 时置位；
  若同时退出码非 0，会在诊断前加一行
  `输出文件已存在，且当前策略为「不覆盖」。请启用覆盖或使用自动重命名。`
- 进程启动失败（可执行文件不存在/不可执行）：返回 `ExitCode = -1`，
  `Diagnostic = "无法启动 FFmpeg（{路径}）：{异常消息}"`，`Success=false`。

### 3.10 部分输出的清理（partial output cleanup）

`源文件: FFmpegService.cs:732-736, 840-858`

- `ConversionQueue` 每次执行都传 `CleanupPartialOutput = true`。
- 触发条件：`!success && CleanupPartialOutput && !skippedExisting && OutputPath 非空`。
- 动作 `TryDeletePartialOutput(path)`：文件存在且 **大小 < 64 MiB**（`1024*1024*64`）→ 删除；否则**保留**。
  （阈值的含义：只删"明显是残片"的文件，不去删一个可能已经完整但退出码异常的几十 GB 输出。）
- 删除失败静默忽略；**跳过（skip）情形不删**（`skippedExisting` 显式排除，避免删掉用户已有文件）。
- ✅ 没有临时文件/`.part` 命名：ffmpeg 直接写目标路径，靠"删掉它"回收。

### 3.11 批次状态维护

`源文件: ConversionQueue.cs:814-849`

每次任务进入终态（`FinalizeTask` / `TryMarkFailed` / pending 取消 / 跳过）都会调用：

```
if (batchId == null) return
batch = 读批次；若不存在 return
tasks = 该批次的全部子任务；若为空 return
batch.TotalFiles     = tasks.Count
batch.CompletedFiles = count(status == Completed)          // Skipped 不计入 completed，也不计入 failed
batch.FailedFiles    = count(status in {Failed, Cancelled})
finished = 所有子任务状态 ∈ {Completed, Failed, Cancelled, Skipped}
if (finished)   batch.Status = (FailedFiles == 0) ? Completed : Failed ; batch.CompletedAt ??= now
else if (任一子任务 Running) batch.Status = Running
（未 finished 且无 Running → 保持原状态，通常是 Pending）
```
⚠️ `Skipped` 子任务既不加 `CompletedFiles` 也不加 `FailedFiles`，因此一批"全部跳过"会得到
`Status=Completed` 但 `CompletedFiles=0`。移植时按此保留，或显式修正并在迁移说明里标注。

### 3.12 重试语义（Retry）

`源文件: TaskService.cs:258-284`

- 重试**不修改原任务**，而是**新建一条任务**并立即入队：
  复制 `InputPath, OutputPath, TemplateId, TemplateName, TemplateParameters(深拷贝), TotalDuration,
  InputFileSize, BatchId`；设置 `Status=Pending`、`RetryOfTaskId=源任务 Id`、`CreatedAt=now`；
  `OutputPath` 沿用旧值。
- **不复制** `OutputPathResolved` / `OverwriteExisting`（保持默认 false/false）→
  **重试会重新按当前 `FileExistsAction` 解析输出路径**：典型情况是上次已有的输出会被
  "重命名/覆盖/跳过"策略重新处理（例如策略=Rename 时会生成 `a_converted (1).mp4`）。
- ⚠️ 源码里有一行注释写着"同一批次里已有等待/运行中的同名任务时不重复创建"，
  但**没有任何去重代码**——重复点重试会真的建多条任务。
- `Progress/CurrentTime/LogOutput/ErrorMessage/StartedAt/CompletedAt/OutputFileSize` 都不继承（新任务默认值）。

### 3.13 孤儿任务恢复（启动时）

`源文件: ConversionQueue.cs:701-727`（经 `TaskService.RecoverOrphanedTasks`，`TaskService.cs:343`）

- 启动初始化时执行一次（`Program.cs:145-151`）。
- 查询 `Status == Running` 的全部任务 → 逐个置：
  `Status=Failed`、`ErrorMessage="应用退出导致任务中断"`、`CompletedAt=now`、
  `EstimatedTimeRemaining=null`、`ProcessingSpeed=null`，落库。
- 返回处理条数（>0 时记 info 日志 `已清理 {n} 个中断的任务`）；异常 → 记 warning 返回 0。
- ⚠️ **`Pending` 任务不会在启动时自动重新入队**：崩溃后队列内存内容丢失，
  这些任务会永远停在 `Pending`，需要用户手动"开始/重试"。
  **移植决策点**：Rust 版可以选择在启动时把 `Pending` 重新入队（更友好）——
  若这样做，必须在新文档/发布说明里写明行为差异。

### 3.14 事件/回调面（UI 订阅的东西 —— 移植成 WebSocket/SSE）

`源文件: ConversionQueue.cs:77-90, 855-859`；转发链见 `TaskService.cs:30-32`（`TaskService` 在构造函数里
把 `Queue` 的三个事件原样重新抛出，所以 UI 只注入 `ITaskService`）。

| 事件 | 负载 | 触发点 | 频率 |
| --- | --- | --- | --- |
| `TaskProgressChanged` | `{ TaskId, Progress:double, Speed:double?, Eta:double? }` | 每个 ffmpeg 进度块 | 约 2 次/秒（由 ffmpeg 决定） |
| `TaskStatusChanged` | `{ TaskId, OldStatus, NewStatus, ErrorMessage:string? }` | 入队后转 Running、跳过、终态、失败、取消 | 每任务 2–3 次 |
| `TaskLogAppended` | `{ TaskId, Line:string }` | 每条 stderr 行 + 每条进度摘要行 | 高（可能上百行/秒） |

**订阅/退订时机**：Blazor 页面 `OnInitializedAsync` 里 `+=`，`Dispose` 里 `-=`
（`Convert.razor:638-640,1342-1344`；`Tasks.razor:598-600,615-617`；`BatchProcess.razor:202-203,519-520`）。
**没有任何事件用于"队列长度变化"**：`QueuedCount`/`ActiveCount` 是轮询读的
（`Tasks.razor:565,638` 用 **2 秒** `PeriodicTimer` 刷新运行中列表；`Tasks.razor:568,770`
用 **200 ms** 延迟做状态刷新合并，避免事件风暴触发多次查询）。
`Convert.razor:1236-1240` 收到状态事件后会**回读数据库**（`GetTaskByIdAsync`）拿最新任务再提示 Toast。

> **移植建议（WebSocket）**：连接建立时先推一份快照（`queue` + 运行中/等待中任务列表），
> 之后推增量；日志事件在服务端做**批量合并**（例如 100–200ms 合并一次、每帧最多 N 行），
> 否则高频 stderr 会把前端打满。事件类型见 §10.4。

### 3.15 关闭与 Dispose

`源文件: ConversionQueue.cs:868-883`

1. `_queue.Writer.TryComplete()`（停止接受新任务）；
2. 对所有 `_cancellations` 调 `CancelAsync()` → 触发优雅 `q`（每个进程还有 5s 兜底强杀）；
3. `await Task.WhenAny(Task.WhenAll(_workers), Task.Delay(3s))` —— **最多等 3 秒**；
4. 异常一律吞掉。

> 移植建议：Rust 侧用 `CancellationToken` + `tokio::signal`，`SIGINT/SIGTERM` 时：
> 停止新任务 → 所有子进程发 `q`（stdin）→ 等 5s → `kill` → 再退出。

### 3.16 日志/DB 写入的线程安全与性能注意

- LiteDB 的 `Update` 在多 worker 下会并发写同一集合；原实现依赖 LiteDB 内部锁。
- 每个任务结束都会 `FlushLogs` + `Update` ×2（flush 一次、finalize 一次），
  再加"每 700 ms 一次"的进度日志 flush → **SQLite 移植时请用单写连接 + `WAL`**，
  或干脆把日志行异步批量落库（但注意 §3.8 的语义：UI 必须实时看到）。
- `_lastPersisted` 只在节流判定里用，**不区分"哪次 flush"**。

---

## 4. 输出路径解析与文件名占位符

> 参考实现把这件事拆成两处：
> - **冲突策略/路径解析** = `Services/OutputPathResolver.cs`（§4.1–4.4）；
> - **文件名模板（占位符）** = `Services/FileService.cs:85-144`（§4.5–4.7）。
> Rust 侧建议合成一个 `output_path` 模块，但**语义必须两个都保留**：占位符在"用户命名"阶段展开，
> 冲突解析在"任务执行前"再做一次（因为排队等待期间目标可能被别人创建）。

### 4.1 `OutputPathResolver.Resolve` —— 决策表

`源文件: Services/OutputPathResolver.cs:41-112`

输入：`requestedPath`（用户/模板期望的路径）、`action`（`FileExistsAction`）、
`inputPath`（**可选，但必须传**，用于"输出==输入"检查）。

返回 `OutputPathResolution`：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `OutputPath` | string（required） | 最终使用的输出路径 |
| `Overwrite` | bool（required） | 决定 ffmpeg 加 `-y`(true)/`-n`(false) |
| `Skip` | bool（required） | 是否应当跳过本次转换 |
| `Renamed` | bool（required） | 相对请求路径是否发生了自动改名 |
| `Note` | string? | 给用户看的说明（写入 `LogOutput` / UI 提示） |

决策顺序（**严格按此顺序，短路返回**）：

| # | 条件 | 结果 | `Note` |
| --- | --- | --- | --- |
| 1 | `requestedPath` 空白 | `OutputPath=原样`，`Overwrite=true`，`Skip=false`，`Renamed=false` | 无 |
| 2 | `PathsEqual(ToAbsolute(requested), inputPath)` | **强制** `Rename(...)`（**与 action 无关**） | `输出路径与输入文件相同，已自动改名以避免覆盖源文件。` |
| 3 | 文件不存在（`safe exists`） | `OutputPath=绝对路径`，`Overwrite=true`，`Skip=false`，`Renamed=false` | 无 |
| 4 | 存在 && `Skip` | 原路径，`Overwrite=false`，`Skip=true`，`Renamed=false` | `输出文件已存在，按设置跳过本次转换。` |
| 5 | 存在 && `Overwrite` | 原路径，`Overwrite=true`，`Skip=false`，`Renamed=false` | `输出文件已存在，将按设置覆盖。` |
| 6 | 存在 && `Rename` | `Rename(绝对路径)`（下面 §4.2） | `目标文件已存在，已自动重命名为 {新文件名}。` |
| 7 | 存在 && `Ask`（默认分支） | **`Rename` + 覆盖 Note** | `输出文件已存在，已自动重命名以避免覆盖原文件。` |

**关键点**：
- 两条 `Note` 文案会出现在任务日志里（执行层 `resolution.Note` → `AppendLog`），要照抄。
- 第 2 条（输出==输入）**优先于一切策略**，包括 `Overwrite` 和 `Ask`。这是防"边读边写同一文件导致源文件损坏"的硬规则。
- 执行层（§3.5）对 `OutputPathResolved=true` 的任务**跳过整个 Resolve**，只保留第 2 条检查。

### 4.2 自动改名算法（`Rename`）

`源文件: OutputPathResolver.cs:114-155`

```
dir  = 目录部分（为空 → 原样返回，Overwrite=true, Renamed=false）
name = 去掉扩展名的文件名
ext  = 扩展名（含点）
for i in 1..=9999:                       // MaxRenameAttempts = 9999
    candidate = dir / "{name} ({i}){ext}"    // 注意：圆括号，数字前有一个空格
    if not exists(candidate): return { candidate, Overwrite=true, Skip=false,
                                      Renamed=true, Note="目标文件已存在，已自动重命名为 {文件名}。" }
// 全部占用 → 时间戳兜底（本地时间！）
stamped = dir / "{name}_{yyyyMMddHHmmss}{ext}"
return { stamped, true, false, true, Note="目标文件已存在，已自动重命名为 {文件名}。" }
```

- 命名规律：`a.mp4` → `a (1).mp4` → `a (2).mp4` …（**一个空格** + 半角括号）。
- 时间戳兜底用 **`DateTime.Now`（本地时间）**，格式 `yyyyMMddHHmmss`。
- `FileService.GenerateUniqueOutputPath`（`FileService.cs:67-83`）是**另一份等价实现**：
  同样 `(i)` 递增（`i` 从 1 到 9999，循环条件 `< 10000`），兜底同样是
  `{name}_{yyyyMMddHHmmss}{ext}`（本地时间）—— 移植时合并成一份即可，但**行为要一致**。

### 4.3 `PathsEqual`（判断同一文件）

`源文件: OutputPathResolver.cs:195-212`

```
比较 = (Linux ? Ordinal : OrdinalIgnoreCase)      // macOS/Windows 默认大小写不敏感
返回 string.Equals(TrimEndingDirectorySeparator(GetFullPath(a)),
                   TrimEndingDirectorySeparator(GetFullPath(b)), 比较)
异常 → false
```
注意：macOS 被当作**大小写不敏感**（默认 APFS 确实如此）。
Rust 侧等价：规范化路径（绝对化 + 去掉尾部分隔符），Linux 用字节精确比较，其它平台用小写化后比较。

### 4.4 `IsOutputPathWritable`（可写性探测）

`源文件: OutputPathResolver.cs:158-177`

```
dir = dirname(ToAbsolute(outputPath))；为空 → false
若 dir 不存在 → mkdir -p
probe = dir / ".ffmpegwebui-write-test-{Guid:N}"     // 32 位十六进制随机名
以 CreateNew + DeleteOnClose 打开并写 1 字节 → true；任何异常 → false
```
⚠️ 副作用：探测文件用 `DeleteOnClose`，但崩溃时可能残留 `.ffmpegwebui-write-test-*`。
移植建议：用 `probe.tmp` + `RAII` 删除，或直接尝试 `OpenOptions::create_new` 后立刻 remove。

### 4.5 文件名占位符语法（`FileService.FormatFileName`）

`源文件: Services/FileService.cs:85-144`；系统提示见 `IFileService.cs:21-25`。

**签名**：`FormatFileName(template, inputPath, extension) -> 文件名（不含扩展名）`
—— 调用方负责拼 `.{extension}`（`Convert.razor:962` 处即如此）。

替换顺序：**固定占位符 → `{now:FMT}` → `{random:N}` → `{random}` → `{counter:N}` → 清洗**。
所有名字匹配都**忽略大小写**。未知占位符原样保留。

| 占位符 | 展开为 | 备注 |
| --- | --- | --- |
| `{filename}` | 输入文件名（不含扩展名） | |
| `{ext}` | 输入扩展名（**不含点**） | `mp4` |
| `{outputext}` | 输出扩展名（归一化；空 → `mp4`，去前导点） | |
| `{dir}` | 输入文件的**父目录名**（最后一段，不含路径） | |
| `{datetime}` | 本地 `yyyyMMdd_HHmmss` | |
| `{date}` | 本地 `yyyyMMdd` | |
| `{time}` | 本地 `HHmmss` | |
| `{year}` | 本地 `yyyy` | |
| `{month}` | 本地 `MM` | |
| `{day}` | 本地 `dd` | |
| `{hour}` | 本地 `HH`（24 小时制） | |
| `{minute}` | 本地 `mm` | |
| `{second}` | 本地 `ss` | |
| `{now:FMT}` | 本地时间按 .NET 自定义日期格式 `FMT` 格式化 | 正则 `\{now:([^}]+)\}`（忽略大小写）；格式非法 → **保留原文** |
| `{random}` | `Guid.NewGuid().ToString("N")` 的前 8 个字符（小写十六进制） | |
| `{random:N}` | 同上前 `N` 字符，`N` 被 clamp 到 `1..32` | 正则 `\{random:(\d+)\}`；解析失败 → 8 位 |
| `{counter:N}` | `(now.Ticks % 10^N)` 补零到 `N` 位十进制 | 正则 `\{counter:(\d+)\}`；`N` clamp `1..10`；解析失败 → `"0"` |

⚠️ **`{now:FMT}` 的正则 `[^}]+` 会吞掉 `}` 之前的一切**，所以 `{now:yyyy-MM-dd_HH-mm-ss}` 可用，
但不支持嵌套花括号。
⚠️ 模板为空/空白 → 直接返回输入文件名（不含扩展名）。
⚠️ `{random:N}` 与 `{random}` 是**两次独立的随机生成**：若模板同时含二者，得到两个不同值。

**`{counter:N}` 的 .NET Ticks 定义**：`DateTime.Ticks` = 自 **0001-01-01T00:00:00 UTC** 起的
100 纳秒数（`DateTime.Now.Ticks`，用本地"墙上时间"值配 UTC 纪元）。Rust 等价计算：

```rust
// ticks = 从 0001-01-01T00:00:00Z 起的 100ns 单位数（对 now 用本地时间值）
let ticks: i64 = (unix_nanos_since_epoch + 62135596800_i64 * 1_000_000_000) / 100;
let counter = ticks % 10_i64.pow(digits);
format!("{:0width$}", counter, width = digits as usize)
```
（`62135596800` = 0001-01-01 → 1970-01-01 的秒数。）
实际用途是"高频下不易重复的短编号"，**不要求与原实现逐位相同**，但必须是 N 位十进制补零。
若想完全避免实现 Ticks，可退化为 `(unix_nanos/100) % 10^N` 并在文档注明差异。

### 4.6 .NET 自定义日期格式 → Rust 必须产出什么

`{now:FMT}` 里的 `FMT` 是 .NET 的自定义格式串（**不是** strftime）。Rust（chrono）的格式串语法不同，
必须做一次翻译或自己实现一个 .NET 子集解析器。常用对照：

| .NET | 含义 | chrono（`time`/`chrono`） | 示例 |
| --- | --- | --- | --- |
| `yyyy` | 4 位年 | `%Y` | 2026 |
| `yy` | 2 位年 | `%y` | 26 |
| `MM` | 2 位月 | `%m` | 02 |
| `M` | 月（不补零） | `%-m` | 2 |
| `MMM` | 月缩写 | `%b` | Feb |
| `MMMM` | 月全名 | `%B` | February |
| `dd` | 2 位日 | `%d` | 14 |
| `d` | 日（不补零） | `%-d` | 14 |
| `HH` | 24 小时制小时（2 位） | `%H` | 09 |
| `H` | 24 小时制小时（不补零） | `%-H` | 9 |
| `hh` | 12 小时制小时（2 位） | `%I` | 09 |
| `mm` | 分（2 位） | `%M` | 05 |
| `ss` | 秒（2 位） | `%S` | 07 |
| `fff` | 毫秒（3 位） | `%3f`（chrono）/ `%f`（time） | 123 |
| `tt` | AM/PM | `%p` | AM |
| `zzz` | 与 UTC 的偏移 `+08:00` | `%:z` | +08:00 |
| `dddd` / `ddd` | 星期全名/缩写 | `%A` / `%a` | Thursday / Thu |
| `\x` 或 `'text'` | 字面量 | 直接写字面量 | `_` |

**行为细节（必须复刻）**：

- 字面量字符（如 `-`、`_`、`:`、空格）原样输出；.NET 里 `:` 在自定义格式中也是字面量。
- 格式非法（如 `{now:zzz:yy}`、含未转义的特殊字符）→ 原实现会**抛异常被 catch**，
  结果**原样返回 `{now:...}` 字面量**。Rust 侧遇到无法解析的格式也应**原样保留**，
  而不是 panic 或返回空串。
- `FMT` 用的是**本地时间**（`DateTime.Now`）。
- 由于随后会 `SanitizeFileName`，`: / \` 等会被替换成 `_`（见 §4.7），所以
  `{now:HH:mm:ss}` 最终得到 `09_05_07`，**不需要**在格式里回避冒号。

### 4.7 文件名清洗（`SanitizeFileName`）

`源文件: FileService.cs:147-169`

1. 把 `Path.GetInvalidFileNameChars()` 里的字符替换成 `_`。
   - Linux/macOS：非法字符 = `\0` 和 `/`；
   - Windows：还包含 `< > : " / \ | ? *` 以及 `0x00–0x1F` 控制字符。
   → **同一模板在不同平台得到不同结果**，移植时按目标平台取对应集合（不要统一成 Windows 集合，
   否则在 macOS 上会把合法的 `:` 也改掉）。
2. `Trim()`，再 `TrimEnd('.', ' ', ...)`（去尾部点和空格 —— Windows 不允许文件名以点/空格结尾）。
3. **Windows 才做**保留设备名检查：`CON, PRN, AUX, NUL, COM1..COM9, LPT1..LPT9`（忽略大小写）
   → 前面加 `_`。
4. 清洗后为空 → 返回 `"output"`。

**安全收益**：因为清洗在最后一步，用户模板里的 `/`（路径分隔符）也会被替换成 `_`，
所以**占位符/用户命名无法逃出输出目录**。移植时必须保留这个"先展开、后清洗"的顺序。

### 4.8 命名规则（Original / Suffix）与调用方

`源文件: TaskService.cs:114-141`（批量）、`Components/Pages/Convert.razor:946-966`（单文件）

**批量路径**（`CreateBatchTaskAsync`）：

```
extension = template.OutputExtension 去前导点；空 → "mp4"
suffix    = (settings.OutputNaming == Suffix) ? settings.OutputSuffix : null
每个输入文件: outputPath = FileService.GenerateOutputPath(input, outputDirectory, extension, suffix)
```
`GenerateOutputPath`（`FileService.cs:57-65`）：`{basename}{suffix}.{ext}`，用 `Path.Combine(outputDirectory, …)`。
（`outputDirectory` 为空时 `Path.Combine` 只返回文件名 → 后续 `ToAbsolute` 会把它落到**进程 CWD**，
⚠️ 这是一个隐患，建议 Rust 侧对空输出目录直接报错。）

**单文件路径**（UI 侧）：用户可填"输出文件名模板"；非空 → `FormatFileName(模板, 输入, 扩展名)`；
空 → 输入 basename + （`OutputNaming=Suffix` 时）`settings.OutputSuffix`；最后 `Path.Combine(输出目录, "{name}.{ext}")`。

### 4.9 `Ask` 策略到底"问"了什么（UI 侧语义，必须照搬）

`源文件: Convert.razor:216-226, 825-880, 976-985, 1032-1060`；`Enums.cs:75`；`Settings.razor:1555-1561`

- `Ask` **不是**执行期的交互提问（执行期 `Resolve` 把 `Ask` 当 `Rename` 处理，见 §4.1 第 7 行）。
  真正发生的是：
  1. 单文件页面在**生成命令预览时**调用 `Resolve(requested, settings.FileExistsAction, inputPath)`，
     把结果存进 `_resolvedOutputPath`，并根据 `Skip/Renamed/Overwrite` 显示提示条；
  2. 若发生冲突，提示条上出现两个按钮：
     **「覆盖已有文件」**（`_forceOverwrite = true` → 重算预览，用 `Overwrite` 策略）、
     **「保留两者（自动改名）」**（`_forceOverwrite = false` → 回到设置里的策略）；
  3. 点"开始转换"时：
     `overwrite = _forceOverwrite || !File.Exists(outputPath)`，
     然后 `CreateTaskAsync(input, outputPath, templateId, parameters, overwrite)` →
     `OutputPathResolved = true`，`OverwriteExisting = overwrite`。
- 也就是说 **`Ask` 的"询问"= 预览阶段的一次界面确认**，一旦用户点开始，执行层不再改路径。
- 批量处理：不传 `overwriteExisting` → `OutputPathResolved = false` → 执行层按全局策略；
  若全局是 `Ask` → 落到 §4.1 第 7 行 → **自动重命名**。（设置页帮助文案：
  "转换前在界面询问你；批量处理时自动改为重命名，避免卡住等你确认。"）
- 设置页帮助文案原文（`Settings.razor:1557-1560`），Rust 前端应复用：
  - Ask：`转换前在界面询问你；批量处理时自动改为重命名，避免卡住等你确认。`
  - Overwrite：`直接覆盖同名文件（已有输出的内容会丢失）。命令里会自动加 -y，不会等待确认。`
  - Skip：`保留已有文件、跳过该文件的转换，任务会标记为「已跳过」。`
  - Rename：`自动换成不冲突的新名字（如 video (1).mp4），原文件和已有输出都保留。`

### 4.10 附：命令参数占位符（`{encoder}` 等）是另一套语法

`源文件: Services/CommandLineBuilder.cs:384-478`

`CommandTemplate.CommandArgs` 用的占位符与文件名模板**不是同一套**（这容易混）：
`{input} {output} {input_dir} {input_name} {input_filename} {input_ext} {output_dir} {output_name}
{output_ext} {output_pattern} {duration} {width} {height} {fps} {vcodec} {acodec} {sample_rate}
{channels} {encoder} {audio_encoder} {threads} {hwaccel} {crf} {quality} {q} {quality_args}
{preset_args} {hwupload_args} {device_args} {muxer_args} {date} {time} {datetime} {random}`
＋模板自定义参数名（`TemplateParameter.Name`）。
- 正则匹配 `\{([^}]+)\}`；未识别的占位符**保留原文**，同时记入 `Warnings`（以 `提示：` 前缀进日志）。
- 展开顺序要点：内置占位符 → 用户参数（`context.Parameters`，**空值不覆盖内置**，`input`/`output` 永不被覆盖）
  → 用最终质量值重新展开 `{quality_args}/{preset_args}/{hwupload_args}`（`{q}` 在内层再替换）。
- `{encoder}` 的默认值（当解析不到编码器时）是 `libx264`；`{audio_encoder}` 默认 `aac`。
- `{output_pattern}` = `{output_dir}/{output_name}_%04d{output_ext}`（图片序列用）。
- 这些占位符**不是本次文档的重点**，但 `{encoder}` 的存在会决定 §3.6.1 是否去解析编码器。

---

## 5. SettingsService

`源文件: Services/SettingsService.cs:1-214`、`Services/ISettingsService.cs:3-25`

### 5.1 接口面

```csharp
interface ISettingsService {
    Task<AppSettings> GetSettingsAsync();          // 永远返回已 Normalize 的对象（内存缓存）
    Task SaveSettingsAsync(AppSettings settings);
    Task ResetToDefaultAsync();
    Task<bool> ValidateFFmpegPathAsync(string path);      // 跑 `ffmpeg -hide_banner -version`
    Task<string?> GetFFmpegVersionAsync(string path);
    event EventHandler? SettingsChanged;                  // 保存/重置后触发
}
```

### 5.2 读取与缓存

`源文件: SettingsService.cs:25-57`

- 进程内**单份缓存** `_cached`（`lock (_gate)` 保护）；命中直接返回**同一个对象引用**
  （⚠️ 调用方若就地修改它会污染全局设置 —— 这是原实现的隐患，Rust 侧应返回不可变快照/`Arc<Settings>`）。
- 未命中：`Db.Settings.FindAll().FirstOrDefault()`（**取第一条**）；异常 → 视为没有。
- 数据库里没有记录 → `CreateDefaultSettings()` 并尝试 `Insert`（失败也只在内存里用默认值，**不影响 UI 可用**）。
- 返回前 `Normalize()`。

### 5.3 默认值（首次运行落库的内容）

`源文件: SettingsService.cs:194-213`

```json
{
  "FFmpegPath": "",
  "FFprobePath": "",
  "DefaultOutputDirectory": "{AppPaths.DefaultOutputDirectory}",   // 运行时求值：视频目录→主目录→临时目录
  "OutputNaming": "Suffix",
  "OutputSuffix": "_converted",
  "FileExistsAction": "Ask",
  "PreferHardwareAcceleration": true,
  "PreferHardwareDecoding": false,
  "AutoFallbackToSoftware": true,
  "PreferredHardwareEncoder": "",
  "MaxConcurrentTasks": 1,
  "MaxLogLines": 500,
  "TaskHistoryRetentionDays": 30,
  "Theme": "System",
  "NotifyOnCompletion": true,
  "AutoOpenBrowser": true,
  "ListenPort": 5265
}
```
注意：`GlobalFFmpegArguments` 的默认值 `""` 来自模型初始化器（`AppSettings.cs:56`），
`CreateDefaultSettings` 里**没有显式列出**它 —— 结果相同，但移植时别漏字段（漏了会给 `null` 而不是 `""`）。

### 5.4 保存（持久化时机）与副作用

`源文件: SettingsService.cs:59-85`

1. `Normalize()`（原地修正非法值）；
2. 锁内：读出旧的单条记录，**沿用其 `Id`**，然后 `Upsert(settings)`；异常**吞掉**（内存状态保留）；
3. `_cached = settings`；
4. **锁外**：`Locator.Invalidate()`（清 ffmpeg 路径缓存）＋ `SettingsChanged` 事件
   （`FFmpegService` 订阅它来清 `FFmpegInfo` 缓存，见 `FFmpegService.cs:106-118`）。

**持久化时机**：只有一个入口 —— 设置页点"保存"（`Settings.razor:1204`）；没有自动保存、没有 debounce。
表单在保存前后会 `GetSettingsAsync` 回读并**用保存后的实际值刷新表单**（因为 `Normalize` 可能改值）。

**保存前的前置校验（UI 层，`Settings.razor:1155-1180`）**：

| 校验 | 规则 | 错误文案 |
| --- | --- | --- |
| 后缀 | `OutputNaming=Suffix` 时不能为空 | `输出文件后缀：选择「添加后缀」时不能为空。` |
| 后缀字符 | 不能含非法文件名字符 | `输出文件后缀：包含文件名不允许的字符「{c}」。` |
| 并发 | `1..8`（⚠️ 与模型 `1..16` 不一致） | `同时执行的转换任务数：必须在 1 ~ 8 之间。` |
| 日志行数 | `50..20000` | `单个任务保留的日志行数：必须在 50 ~ 20000 之间。` |
| 历史天数 | `0..3650` | `任务历史保留天数：必须在 0 ~ 3650 之间。` |
| 端口 | `0..65535` | `监听端口：必须在 0 ~ 65535 之间（0 表示自动选择空闲端口）。` |
| 主题 | `System/Light/Dark` | `主题：取值无效，请重新选择。` |
| 默认输出目录 | 保存时实际探测可写（写临时文件） | `默认输出目录：不可写或无法创建：{dir}。请换一个目录，或先创建它并授予写权限。` |

### 5.5 恢复默认

`源文件: SettingsService.cs:87-107`
`DeleteAll()` → `Insert(CreateDefaultSettings())` → 更新缓存 → `Locator.Invalidate()` + 事件。
异常时只把内存缓存换成默认值（数据库保持原样）。

### 5.6 FFmpeg 路径验证 / 版本读取

`源文件: SettingsService.cs:109-180`

- `ValidateFFmpegPathAsync(path)`：以 10 秒超时执行 `{exe} -hide_banner -version`
  （`path` 为空则用字面量 `ffmpeg`，走 PATH；非空则 `ToAbsolute`），
  输出（stdout 为空时用 stderr）**包含 `ffmpeg version`**（忽略大小写）→ true。超时/异常 → false/null。
- `GetFFmpegVersionAsync(path)`：取第一个以 `ffmpeg version` 开头的行，
  截断到 `Copyright` 之前（避免把编译参数整段塞进 UI）；找不到 → null。
- 子进程统一：不重定向 stdin 之外还**主动关闭 stdin**、UTF-8 编码、不建窗口、`Kill(entireProcessTree)`
  超时兜底、读流各限 3 秒。

### 5.7 `ListenPort` / `AutoOpenBrowser`：**在 Web 主机启动之前**如何被读到

`源文件: Program.cs:230-264, 296-324`（详见 §9.2、§9.3）
这与 `SettingsService` 无关：启动早期 DI 还没建，代码**直接打开 LiteDB 文件**读 `settings` 集合第一条，
用 `BsonValue.ToString()` 取字段文本，再用 `int.TryParse` / `bool.TryParse` 解析。
- 读不到（文件不存在、库损坏、字段缺失）→ 返回 null，调用方用默认值（5265 / true）。
- ⚠️ 这意味着**改端口必须重启**；UI 也明确提示：`修改后需要重启应用才会生效；0 表示自动挑选一个空闲端口。`
  （`Settings.razor:611`）
- 优先级里 `appsettings.json` 的 `FFmpegWebUI:ListenPort` **排在数据库偏好之前**（§9.2）。

---

## 6. AppPaths：跨平台路径与平台判定

`源文件: Services/AppPaths.cs:1-143`（静态类；数据目录细节见 §1.4）

### 6.1 平台判定与显示名

`源文件: AppPaths.cs:9-34`

| 成员 | 实现 | 值 |
| --- | --- | --- |
| `Platform` | 依次判断 Windows → OSX → 否则 Linux（**没有第四个分支**） | `OSPlatform` |
| `IsWindows` / `IsMacOS` / `IsLinux` | `OperatingSystem.IsXxx()` | bool |
| `IsContainer` | `IsLinux && (File.Exists("/.dockerenv") \|\| Env("DOTNET_RUNNING_IN_CONTAINER") == "true")` | bool |
| `ExeSuffix` | Windows → `".exe"`，其它 → `""` | string |
| `PlatformDisplayName` | Windows → `"Windows"`；macOS → `"macOS"`；Linux → `"Linux"`；其它 → `"未知系统"` | string |

> Rust 等价：`cfg!(target_os = "windows" | "macos" | "linux")`；
> ⚠️ 原实现把 FreeBSD 等也归入 Linux 分支 —— 移植时若要支持其它 Unix，
> 需要显式决定"数据目录"和"文件对话框后端"的归属。
> `ExeSuffix` 只在拼可执行名时用（`ffmpeg.exe`）。

### 6.2 绝对路径规整（`ToAbsolute`）

`源文件: AppPaths.cs:118-130`

```
ToAbsolute(path):
  path 空白 → 原样返回
  → Trim() → Trim('"')（去掉用户从终端粘贴时带的引号）
  → ExpandHome(~ 展开)
  → Path.GetFullPath(...)（**不要求文件存在**；相对路径基于进程 CWD）
  → 异常时返回展开后的字符串（不抛）
```
**使用点**：数据库路径、`FFmpegPath/FFprobePath`、任务的 `InputPath/OutputPath`、
`OutputPathResolver.Resolve/Rename/IsOutputPathWritable`、文件对话框的起始目录。
Rust 等价：`path.trim_matches('"')` → 展开 `~` → `std::path::absolute`/`fs::canonicalize(parent)`+
`join(file_name)`（**注意 `canonicalize` 要求存在且会解析符号链接**，语义不同，需自己实现
"纯词法绝对化"以避免把不存在的输出路径弄错）。

### 6.3 其它辅助

| 函数 | 行为 | 行号 |
| --- | --- | --- |
| `ExpandHome(path)` | `~` / `~/…` / `~\…` → `GetFullPath(用户主目录 + 其余部分)`；主目录取不到则原样返回 | `89-101` |
| `EnsureDirectory(path)` | 尝试 `CreateDirectory`（递归），**失败静默**，总是返回入参 | `104-115` |
| `DefaultOutputDirectory` | 视频目录（存在）→ 主目录（存在）→ 临时目录 | `74-86` |
| `SafeSpecialFolder(folder)` | 包一层 try/catch，失败返回 `""` | `132-142` |

### 6.4 ffmpeg 目录（"随应用内置"的位置）

⚠️ 原实现的 `AppPaths` **没有** ffmpeg 目录常量；内置目录逻辑在 `Services/FFmpegLocator.cs:113-145`。

`FindExecutable(name)` 的搜索顺序（`name` ∈ {`ffmpeg`,`ffprobe`}，`exe = name + ExeSuffix`）：

1. **随应用内置**（相对 `AppContext.BaseDirectory`，即可执行文件所在目录）——按此顺序：
   ```
   {exe}
   ffmpeg/{exe}
   tools/{exe}
   tools/ffmpeg/{exe}
   runtimes/ffmpeg/{exe}
   ```
2. **系统 PATH**：遍历 `PATH` 的每一项（`Trim('"')`），`File.Exists(dir/{exe})`。
3. **平台常见安装位置**（`FFmpegLocator.cs:147-214`）：
   - Windows：`C:\ffmpeg\bin\{exe}`、`C:\ffmpeg\{exe}`、`%ProgramFiles%\ffmpeg\bin\{exe}`、
     `%ProgramFiles(x86)%\ffmpeg\bin\{exe}`、`%LOCALAPPDATA%\Microsoft\WinGet\Links\{exe}`、
     `C:\ProgramData\chocolatey\bin\{exe}`、`%LOCALAPPDATA%\Microsoft\WindowsApps\{exe}`、
     `%APPDATA%\ffmpeg\bin\{exe}`、`~/scoop/shims/{exe}`、`~/scoop/apps/ffmpeg/current/bin/{exe}`
   - macOS：`/opt/homebrew/bin/`、`/usr/local/bin/`、`/opt/local/bin/`、`/usr/bin/`、
     `/nix/var/nix/profiles/default/bin/`、`/run/current-system/sw/bin/`、`~/.local/bin/`、`~/bin/`
   - Linux（**优先 jellyfin-ffmpeg**，因其对 QSV/VAAPI 支持更完整）：
     `/usr/lib/jellyfin-ffmpeg/`、`/usr/local/bin/`、`/usr/bin/`、`/bin/`、`/snap/bin/`、
     `/var/lib/flatpak/exports/bin/`、`/nix/var/nix/profiles/default/bin/`、
     `/run/current-system/sw/bin/`、`~/.local/bin/`、`~/bin/`、`~/.nix-profile/bin/`

**整体解析顺序**（`FFmpegLocator.ResolveCore`，`56-111`）：

1. `settings.FFmpegPath` 非空 且（文件存在 或 它是"不含分隔符的裸名字"→交由 PATH 解析）：
   使用它；ffprobe = `settings.FFprobePath`（若有）→ 否则**ffmpeg 同目录的 ffprobe** →
   否则平台常见位置里的 ffprobe → 否则裸名 `ffprobe`。标记 `IsAutoDetected=false`、
   `Hint="使用设置中指定的路径"`。
2. 否则自动探测：`ffmpeg = FindExecutable("ffmpeg")`；`ffprobe = 显式配置的ffprobe ?? FindExecutable("ffprobe")`；
   找不到时**回退成裸名 `ffmpeg`/`ffprobe`**（让子进程靠 PATH 报错）。
   `Hint` = `"自动探测到 FFmpeg"` 或 `"未找到 FFmpeg，请在设置中指定路径"`。
- 结果**带缓存**，保存设置时通过 `Invalidate()` 清除。
- ⚠️ `ffmpeg` 与 `ffprobe` 版本**不要求一致**，也不做校验。

---

## 7. FileDialogService：原生文件对话框（浏览器做不到的事）

`源文件: Services/FileDialogService.cs:1-460`

**为什么存在**：浏览器 `<input type=file>` 给不出"本机绝对路径"，而 ffmpeg 需要路径。
接口（`FileDialogService.cs:7-20`）：

```csharp
interface IFileDialogService {
    bool IsSupported { get; }                 // 当前环境能否弹出原生对话框
    string BackendDescription { get; }        // 后端描述，展示给用户
    Task<string?> PickFileAsync(string title = "选择文件", string? filter = null, string? initialDirectory = null);
    Task<string?> PickFolderAsync(string title = "选择文件夹", string? initialDirectory = null);
}
```
- **统一超时 5 分钟**（`DialogTimeout`，`FileDialogService.cs:30`）：超时 → `Kill(entireProcessTree)` → 返回 null。
- 进程通用：UTF-8 输出、关闭 stdin、非零退出码 → null、异常 → null（回退到"手动粘贴路径"）。
- `NormalizeResult` 会 `Trim()`；空/空白 → `null`；**包含子串 `User canceled`**（忽略大小写）→ `null`。

### 7.1 可用性判定与后端描述

`源文件: FileDialogService.cs:41-80`（结果缓存，`_supported` / `_backend`）

| 平台 | `IsSupported` | `BackendDescription` |
| --- | --- | --- |
| Windows | 恒 `true`（依赖系统自带 PowerShell） | `Windows 文件对话框（PowerShell）` |
| macOS | 恒 `true`（依赖系统自带 osascript） | `macOS 原生选择器（osascript）` |
| Linux | `CommandExists("zenity") \|\| CommandExists("kdialog")` | `zenity` / `kdialog` / `不可用（未安装 zenity 或 kdialog）` |

`CommandExists(cmd)`（`248-263`）：自己遍历 `PATH`，`File.Exists(Path.Combine(dir, cmd))`。
⚠️ **不带 `.exe` 后缀**、**不检查可执行位**，所以"文件存在但不可执行"也会被判为可用。

**"不可用"时前端展示的文案**（`Components/Shared/FileSelector.razor:291-294`，原文）：

> `当前环境不支持系统文件对话框（{BackendDescription}），请在输入框中手动粘贴路径。`

其它相关文案（`BatchFileList.razor:286-290, 308-312`；`FileSelector.razor:310-312`）：

> `没有选择文件。如果对话框没有弹出，可以直接把路径粘贴到上方输入框。（当前后端：{BackendDescription}）`
> `没有选择文件夹。如果对话框没有弹出，可以直接把文件夹路径粘贴到上方输入框。（当前后端：{BackendDescription}）`
> `没有选择文件夹：可能已取消，或系统对话框（{BackendDescription}）不可用。可直接把路径粘贴到输入框。`
> `打开文件对话框失败：{ex.Message}（对话框后端：{BackendDescription}）。可直接把路径粘贴到输入框。`

> ⚠️ 语义陷阱：**返回 null 既表示"用户取消"也表示"后端不可用"**，两者不可区分，UI 只能都用
> "可能已取消或不可用"的措辞。Rust 侧建议区分（返回 `Cancelled` vs `Unsupported` 枚举），
> 但文案保持不变。

### 7.2 macOS：osascript + AppleScript 源文本（逐字）

`源文件: FileDialogService.cs:135-189`

调用方式：
```
/usr/bin/osascript -e "<完整脚本>"
```
- `verb = "choose folder"`（选目录）或 `"choose file"`（选文件）。
- 转义规则 `EscapeAppleScript`：`\` → `\\`，`"` → `\"`（**不转义换行/其他字符**）。
- `start`（起始目录）来自 `ResolveStartDirectory`（§7.5），可能为空。

**带起始目录时的脚本（逐字，`{title}`/`{start}` 为已转义值）**：

```applescript
set theResult to ""
try
    tell application "System Events" to set frontmost of first process whose frontmost is true to true
end try
try
    set theResult to POSIX path of (choose file with prompt "{title}" default location POSIX file "{start}")
on error errMsg number errNum
    if errNum is -128 then return ""
    try
        set theResult to POSIX path of (choose file with prompt "{title}")
    on error
        return ""
    end try
end try
return theResult
```

**起始目录为空时（逐字）**：

```applescript
set theResult to ""
try
    tell application "System Events" to set frontmost of first process whose frontmost is true to true
end try
try
    set theResult to POSIX path of (choose file with prompt "{title}")
on error
    return ""
end try
return theResult
```

**要点（源码注释里解释过，移植必须保留）**：

1. 结果**必须**用 `POSIX path of (...)` 包住。否则得到的是 AppleScript alias 字符串
   （形如 `alias Macintosh HD:Users:me:Movies:a.mp4`），传回程序后会被判为"文件不存在"。
2. 起始目录不可用不能让整个选择器失败：**AppleScript 的一个 `try` 只能有一个 `on error` 分支**，
   所以"用户取消"（错误号 `-128`）与"起始目录无效"必须在同一个错误块里用 `errNum` 区分：
   `-128` → 直接返回空串；其它错误 → 退化为**不带起始目录**再试一次。
3. 第一段 `System Events` 的 `frontmost` 调用是把选择器窗口带到前台（从后台启动的进程弹出的
   窗口会藏在其它窗口后面）。
4. 末尾 `TrimEnd()` 去掉尾部换行；每行由 `AppendLine` 加 `\n`（Windows 上是 `\r\n`，但不影响）。

**返回值处理**：`NormalizeResult`（`381-403`）：
- 依次检查是否以 `alias ` / `file ` / `folder ` / `disk ` / `document file ` 开头（**忽略大小写**），
  是则把剩余部分按 §7.4 的 HFS→POSIX 规则转换（这是"兜底"路径，正常不会走到）。
- 去掉末尾目录分隔符（长度 >1 时；根目录 `/` 保持）。
- 空串 → null。

### 7.3 Linux：zenity / kdialog

`源文件: FileDialogService.cs:195-246`

媒体过滤器（逐字，`FileDialogService.cs:197-198`）：

```
*.mp4 *.mkv *.avi *.mov *.wmv *.flv *.webm *.m4v *.ts *.mpg *.mp3 *.wav *.flac *.aac *.ogg *.m4a *.opus *.gif *.png *.jpg
```

**选文件**（先 zenity，再 kdialog）：

```bash
zenity --file-selection \
       --title={title} \
       --filename={start}/ \
       --file-filter=媒体文件 | *.mp4 *.mkv *.avi *.mov *.wmv *.flv *.webm *.m4v *.ts *.mpg *.mp3 *.wav *.flac *.aac *.ogg *.m4a *.opus *.gif *.png *.jpg \
       --file-filter=所有文件 | *
```
```bash
kdialog --getopenfilename {start}/ "媒体文件 | *.mp4 ... *.jpg" --title {title}
```
（`{start}/` 由 `EnsureTrailingSeparator` 保证以目录分隔符结尾；`start` 为空串时该参数为 `""`。）

**选目录**：

```bash
zenity --file-selection --directory --title={title} --filename={start}/
```
```bash
kdialog --getexistingdirectory {start}/ --title {title}
```

⚠️ **顺序与取消语义**：先试 zenity；`NormalizeResult` 为 null（**包含"用户取消"**）时**仍会继续试 kdialog**。
即"用户在 zenity 里点了取消"会再弹一次 kdialog（若安装了）。移植时建议改为
"退出码/取消语义可区分"的实现，但需在变更说明里标注行为差异。

### 7.4 AppleScript alias（HFS 路径）→ POSIX 路径

`源文件: FileDialogService.cs:406-434`

```csharp
前缀集合 = ["alias ", "file ", "folder ", "disk ", "document file "]   // 忽略大小写匹配
ConvertAppleScriptAliasToPosix(hfsPath):
    hfsPath 空白 → null
    parts = hfsPath.Split(':')            // HFS 用 ':' 分隔，因此段内不会再有 ':'
    parts.Length <= 1 → null
    volume = parts[0]
    rest   = parts[1..] 过滤掉空段
    isBootVolume = (volume 忽略大小写 == "Macintosh HD")
    segments = isBootVolume ? rest : ["Volumes", volume] + rest
    segments 为空 → "/"
    return "/" + segments.join("/")
```

示例：`Macintosh HD:Users:me:Movies:25 4K.mp4` → `/Users/me/Movies/25 4K.mp4`；
外接卷 `MySSD:Movies:a.mp4` → `/Volumes/MySSD/Movies/a.mp4`。

> 注意：**启动卷名硬编码为 `Macintosh HD`**。若系统盘被改名（如 `Macintosh HD - Data` 不会出现在
> 这条路径，但用户改过卷名会），会错误地拼成 `/Volumes/<卷名>/…`。Rust 侧更稳妥的做法是
> 用卷名与 `/` 的挂载名对比（`/Volumes` 下是否存在该名），或直接依赖 `POSIX path of`（首选路径）。

### 7.5 起始目录与"带不带尾斜杠"

- `ResolveStartDirectory(initialDirectory)`（`446-453`）：
  传入非空**且目录存在** → `ToAbsolute(它)`；否则 → `AppPaths.DefaultOutputDirectory`（§6.3）。
- `EnsureTrailingSeparator(path)`（`455-459`）：空则原样；已以目录分隔符结尾则原样；否则追加。
  ⚠️ 只判断**当前平台的分隔符**（`Path.DirectorySeparatorChar`），不判断 `/`（Windows 上 `C:/x` 会被再补 `\`）。
- 起始目录仅在 macOS/Linux 的 zenity/kdialog 与 Windows 脚本里使用；**它们都不校验目录是否可读**。

### 7.6 Windows：PowerShell + WinForms

`源文件: FileDialogService.cs:269-315`

**调用形态**（按顺序尝试 `powershell.exe` → `pwsh.exe` → `pwsh`）：

```
{shell} -NoProfile -NonInteractive -STA -EncodedCommand {base64}
```
其中 `base64` = **UTF-16LE** 编码的 `前导语句 + 脚本`：

```
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
{脚本}
```
（用 `-EncodedCommand` 的理由：避免引号转义问题、避免中文乱码。
`PowerShellLiteral(text)` 把文本包成 PowerShell 单引号字面量，内部 `'` 翻倍：`'` → `''`。）

**选文件脚本（逐字）**：

```powershell
Add-Type -AssemblyName System.Windows.Forms
$dialog = New-Object System.Windows.Forms.OpenFileDialog
$dialog.Title = '{title}'
$dialog.Filter = '媒体文件|*.mp4;*.mkv;*.avi;*.mov;*.wmv;*.flv;*.webm;*.m4v;*.ts;*.mpg;*.mp3;*.wav;*.flac;*.aac;*.ogg;*.m4a;*.opus;*.gif;*.png;*.jpg|所有文件|*.*'
$dialog.CheckFileExists = $true
$dialog.CheckPathExists = $true
if (Test-Path -LiteralPath '{start}') { $dialog.InitialDirectory = '{start}' }
if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $dialog.FileName }
```

**选目录脚本（逐字）**：

```powershell
Add-Type -AssemblyName System.Windows.Forms
$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = '{title}'
$dialog.ShowNewFolderButton = $true
if (Test-Path -LiteralPath '{start}') { $dialog.SelectedPath = '{start}' }
if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $dialog.SelectedPath }
```

**流程细节**：

- 若某个 shell 的 `RunProcessAsync` 返回 **非 null**（即成功启动且退出码 0，即使 stdout 为空）→
  `return NormalizeResult(stdout)`（用户取消 → stdout 为空 → null）。
- 只有**启动失败或退出码非 0** 才继续尝试下一个 shell。
- ⚠️ PowerShell 里 `$dialog.Filter` 使用 `;` 分隔扩展名、`|` 分隔"描述|模式"两组；保持逐字一致。
- ⚠️ Windows 上"文件不存在"的输入也会被 `CheckFileExists` 拦住（不可能选到不存在的文件）。

**Rust 侧建议**：Windows 用原生 `IFileOpenDialog`（如 `rfd` crate）替代 PowerShell+WinForms，
抛弃进程启动延迟与乱码风险；但**接口语义（返回绝对路径、取消→None、5 分钟超时）必须一致**，
并且仍应保留"不可用时提示手动粘贴路径"的降级路径。

---

## 8. AppStatusService：启动检查与对 UI 暴露的状态

`源文件: Services/AppStatusService.cs:1-164`

### 8.1 暴露的数据结构

`AppStatus`（record，全部 `init` 属性，`AppStatusService.cs:6-55`）：

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `AppVersion` | string | `AppInfo.Version` | 应用版本（`Services/AppInfo.cs`，编译期常量） |
| `Platform` | string | `AppPaths.PlatformDisplayName` | 平台显示名（§6.1） |
| `Runtime` | string | `Environment.Version` | 运行时版本（**record 有默认值但实现里没赋值**，所以对 UI 永远是 .NET 版本字符串；Rust 侧建议填 Rust/构建版本） |
| `DataDirectory` | string | `AppPaths.DataDirectory` | 数据目录 |
| `FFmpegAvailable` | bool | — | ffmpeg 是否可用 |
| `FFmpegVersion` | string | `""` | 可用时是版本号，否则 `"未检测到"` |
| `FFmpegPath` | string | `""` | 可用时是探测到的路径，否则是定位器给的兜底值 |
| `FFprobePath` | string | `""` | ffprobe 路径（来自定位器，**不做可用性判断**） |
| `AutoDetected` | bool | — | 路径是否自动探测得到 |
| `Hwaccels` | List\<string\> | `[]` | ffmpeg 支持的硬件解码方式 |
| `MajorVersion` | int | `0` | ffmpeg 主版本；不可用为 0 |
| `AvailableHardwareEncoderCount` | int | `0` | 可用硬件编码器数量 |
| `TemplateCount` | int | `0` | 模板总数 |
| `TaskCount` | int | `0` | **`QueuedCount + ActiveCount`**（⚠️ 名字像"任务总数"，实际是"排队+运行中"） |
| `Notices` | List\<string\> | `[]` | 提示/警告文案列表 |
| `IsHealthy` | bool（计算） | — | `= FFmpegAvailable` |

接口：

```csharp
interface IAppStatusService {
    Task<AppStatus> GetStatusAsync(bool forceRefresh = false);   // 10 秒缓存
    bool IsDataDirectoryWritable { get; }                        // 数据目录写入探测
}
```

### 8.2 `GetStatusAsync` 执行的检查（顺序与容错）

`源文件: AppStatusService.cs:81-163`

1. 缓存判定：非强制 且 有缓存 且 距今 < **10 秒** → 直接返回缓存。
2. `locator.Resolve()` 拿 ffmpeg/ffprobe 路径。
3. `ffmpegService.GetFFmpegInfoAsync(forceRefresh)`（内部 30 分钟缓存；失败/异常 → `info=null` +
   notice `检测 FFmpeg 时出错：{message}`）。
4. 若不可用 → notice：
   - Windows：`未找到可用的 FFmpeg。请安装 FFmpeg 并加入 PATH，或在「设置」中指定 ffmpeg.exe 的完整路径。`
   - 其它平台：`未找到可用的 FFmpeg。请安装 FFmpeg（macOS: brew install ffmpeg；Linux: apt install ffmpeg），或在「设置」中指定路径。`
5. 否则（可用）若 `MajorVersion in 1..5` → notice：
   `当前 FFmpeg 主版本为 {n}，较旧。建议升级到 9.x 以获得完整的硬件加速与格式支持。`
6. 仅在 ffmpeg 可用时做硬件检测 `hardwareDetection.DetectAsync(forceRefresh)`：
   - `AvailableHardwareEncoders.Count` 作为 `AvailableHardwareEncoderCount`；
   - 数量为 0 → notice：`未检测到可用的硬件编码器，转换将使用 CPU 软件编码（速度较慢但结果稳定）。`
   - 异常 → notice：`硬件编码器检测失败：{message}`。
7. `templateService.GetAllTemplatesAsync()`（异常 → notice `读取模板失败：{message}`）→ `TemplateCount`。
8. `TaskCount = taskService.QueuedCount + taskService.ActiveCount`。
9. 写缓存（对象 + `_cachedAt = UtcNow`）。

`IsDataDirectoryWritable`（`79`）= `fileService.IsDirectoryWritable(AppPaths.DataDirectory)`
= `OutputPathResolver.IsOutputPathWritable({DataDir}/probe.tmp)`（§4.4，会在数据目录里写一个临时文件）。

### 8.3 启动期（`Program.cs`）与 AppStatusService 的分工

`Program.cs:129-204` 的启动初始化**不等同于** `AppStatusService`，两者做的是不同的事：

| 启动初始化（`InitializeApplicationAsync`） | AppStatusService |
| --- | --- |
| 打开数据库读一次 settings（校验库可用）并记录数据目录 | 按需（UI 打开首页/设置时）生成快照 |
| `RecoverOrphanedTasks()`（§3.13） | — |
| `templateService.InitializeSystemTemplatesAsync()`（系统模板同步） | 只统计模板数量 |
| 按 `TaskHistoryRetentionDays` 清理历史（>0 时） | — |
| 后台任务：`GetFFmpegInfoAsync(forceRefresh:true)` → 记日志/版本过旧警告 → `DetectAsync` | 同样信息，但对 UI 结构化暴露 |

启动初始化**失败不阻止启动**（各步独立 try/catch，`Program.cs:167-170`），
因为"用户至少要能打开设置页去修 FFmpeg 路径"（源码注释）。

---

## 9. Program.cs：启动、端口、绑定与 HTTP 表面

`源文件: Program.cs:1-347`

### 9.1 阶段总览

| 顺序 | 动作 | 行号 |
| --- | --- | --- |
| 1 | 解析 ContentRoot（见下） | `14-18`（实现 `222-228`） |
| 2 | 解析绑定地址与端口 → `UseUrls` | `26-32` |
| 3 | 仅回环时设置 `AllowedHosts` | `34-37` |
| 4 | 日志格式：单行控制台 + 时间戳 `HH:mm:ss ` | `39-43` |
| 5 | DI 注册（**全部单例**，见 §9.5） | `48-71` |
| 6 | `builder.Build()` | `73` |
| 7 | `await InitializeApplicationAsync(app.Services)`（§8.3） | `78` |
| 8 | 中间件：异常处理页 `/Error`、状态码页 `/not-found`、Antiforgery、静态资源、Razor 组件 | `83-89` |
| 9 | `GET /api/health` | `92-106` |
| 10 | 注册 `ApplicationStarted` 回调自动开浏览器 | `109-121` |
| 11 | `app.Run()` | `123` |

**ContentRoot 解析规则**（`ResolveContentRoot`，`222-228`）：
若 `{AppContext.BaseDirectory}/wwwroot` 存在 → 用可执行文件目录；否则用 `Directory.GetCurrentDirectory()`。
理由（源码长注释）：发布产物的静态资源清单只记相对路径，工作目录不对会导致
"每个静态资源 200 + 空响应体"，浏览器 integrity 校验失败、Blazor 电路起不来。
> Rust 侧等价：把静态资源**嵌入二进制**（`rust-embed`）或强制在可执行文件同目录查找 `dist/`，
> 从根上避免这个坑。

### 9.2 绑定地址与端口解析（优先级链）

`源文件: Program.cs:26-37, 230-288`

```
bindAddress = env FFMPEGWEBUI_BIND                       // 空/未设 → 仅回环
listenPort  = ResolvePort(configuration, args)
isLoopbackOnly = (bindAddress 为空)

if (isLoopbackOnly && listenPort == 0) listenPort = FindFreePort(5265)   // 见下
UseUrls($"http://{isLoopbackOnly ? "127.0.0.1" : bindAddress}:{listenPort}")
if (isLoopbackOnly) Configuration["AllowedHosts"] = "localhost;127.0.0.1;[::1]"
```

**端口优先级（严格按此顺序，第一个成功即返回）**：

| # | 来源 | 说明 |
| --- | --- | --- |
| 1 | 命令行 `--port <n>` 或 `-p <n>` | 只检查 `args[i]` 与下一个参数（`i` 遍历到 `len-2`） |
| 2 | 环境变量 `FFMPEGWEBUI_PORT` | `int.TryParse` |
| 3 | 配置 `FFmpegWebUI:ListenPort` | `appsettings.json` / 环境变量 / 命令行配置提供者（`GetValue<int?>`） |
| 4 | 数据库里保存的 `settings.ListenPort` | 见 §9.3（直接读 LiteDB 文件） |
| 5 | 默认 `5265` | |

**`ListenPort == 0` 的语义**（仅回环时）：`FindFreePort(5265)` 从 5265 起**逐个向上试 100 个端口**
（5265..5364），在 `127.0.0.1` 上"bind 成功再立刻关闭"以此判定空闲；**全部占用 → 返回 0，
交给操作系统随机分配**（此时实际端口只能从服务器地址特性里读，见 §9.4）。

⚠️ 非回环（`FFMPEGWEBUI_BIND=0.0.0.0`）时**不会**做空闲端口探测：`port=0` 直接交给 OS 随机分配。

**AllowedHosts**：只有"仅回环"模式才强制 `localhost;127.0.0.1;[::1]`；
`appsettings.json` 里默认也是这个值（`AllowedHosts`），注释说明"应用也只会监听 127.0.0.1"。
`FFMPEGWEBUI_BIND` 非空时**不改** AllowedHosts（用户自行承担）。

### 9.3 启动前直接读数据库偏好（`ReadSavedPreference`）

`源文件: Program.cs:296-324`

```
dbPath = config["FFmpegWebUI:DatabasePath"]；空 → {DataDirectory}/data.db；非空 → ToAbsolute
if (!File.Exists(dbPath)) return null
using db = new LiteDatabase(dbPath)                   // 第二次打开同一个文件（此时 DI 尚未建立）
doc = db.GetCollection("settings").FindAll().FirstOrDefault()
if (doc == null) return null
value = doc[fieldName]                                // BsonValue 动态取字段
return value.IsNull ? null : value.ToString()
```
- 被调用两次：`ReadSavedPreference(configuration, "ListenPort")` 与 `..., "AutoOpenBrowser"`。
- 任何异常（库损坏/被占用）→ 返回 null，调用方用默认值。
- ⚠️ 这是**唯一**在 DI 之前访问持久层的地方。Rust 侧：在启动早期用只读方式打开 SQLite 读
  `settings.listen_port` / `auto_open_browser` 即可，但要处理"库不存在"（用默认）与"库被锁"（用默认）。

### 9.4 自动打开浏览器

`源文件: Program.cs:250-264, 109-121, 326-347`

**是否打开**（`ShouldAutoOpenBrowser`，优先级从高到低）：

1. `FFMPEGWEBUI_NO_BROWSER == "1"` → **false**（精确匹配字符串 `"1"`）；
2. `app.Environment.IsDevelopment()` → **false**（开发时 launchSettings 已负责打开，避免开两个窗口）；
3. 配置 `FFmpegWebUI:AutoOpenBrowser` 有值 → 用它；
4. 数据库 `settings.AutoOpenBrowser` 能解析成 bool → 用它；
5. 否则 **true**。

**打开什么 URL / 何时**：注册 `ApplicationStarted` 回调 →
从 `IServer.Features.Get<IServerAddressesFeature>().Addresses.FirstOrDefault()` 取**第一个地址**，
把其中的 `127.0.0.1` **替换成 `localhost`**，然后调 `OpenBrowser(url)`。
（替换的理由：部分环境对 `127.0.0.1` 的 cookie/HTTPS 处理不一致，且更友好。）

**各平台命令**：

| 平台 | 命令 |
| --- | --- |
| Windows | `cmd /c start {url}`（`CreateNoWindow = true`） |
| macOS | `open {url}` |
| 其它（Linux） | `xdg-open {url}` |

失败静默忽略（无桌面环境：容器/SSH 场景）。

### 9.5 DI 注册表（生命周期即"状态归属"）

`源文件: Program.cs:48-71`（**全部 Singleton**）

| 接口 | 实现 | 备注 |
| --- | --- | --- |
| `ILiteDbContext` | `LiteDbContext` | 数据访问（`IDisposable`） |
| `IFFmpegLocator` | `FFmpegLocator` | 带缓存 |
| `ISettingsService` | `SettingsService` | 内存缓存设置 |
| `IFileService` | `FileService` | 无状态 |
| `IFileDialogService` | `FileDialogService` | 缓存后端探测结果 |
| `IFFmpegService` | `FFmpegService` | **进程登记表必须单例**，才能从任意页面取消任务 |
| `IHardwareDetectionService` | `HardwareDetectionService` | 检测报告缓存 |
| `ITemplateService` | `TemplateService` | 系统模板同步 |
| `ConversionQueue` | 自注册 | 后台转换引擎 |
| `ITaskService` | `TaskService` | CRUD + 事件转发 |
| `IAppStatusService` | `AppStatusService` | 状态快照（10s 缓存） |
| `IToastService` | `ToastService` | 全局轻提示（纯 UI） |

> 唯一"非单例"的生命周期在 Blazor 组件里（`Scoped`/瞬时），但**承载任务状态的都在单例里**，
> 这正是"刷新页面不中断转换"的实现方式。Rust 侧：`AppState` 放进 `Arc<...>` 共享。

### 9.6 `GET /api/health` 响应（唯一已实现的 HTTP API）

`源文件: Program.cs:92-106`

```json
{
  "status": "ok",
  "version": "{AppInfo.Version}",
  "platform": "{AppPaths.PlatformDisplayName}",
  "ffmpeg": "{locator.Resolve().FFmpeg}",
  "ffmpegResolved": true,
  "ffprobe": "{locator.Resolve().FFprobe}",
  "ffprobeResolved": true
}
```
- 由 `Results.Json(...)` 序列化 → **camelCase 键名**（ASP.NET 默认策略），字段名逐字为
  `status/version/platform/ffmpeg/ffmpegResolved/ffprobe/ffprobeResolved`。
- `ffmpeg`/`ffprobe` 即使未解析成功也会返回（裸名 `ffmpeg`/`ffprobe` 或空），
  用 `ffmpegResolved`/`ffprobeResolved` 表示"确实存在"。
- `status` **恒为 `"ok"`**（与 FFmpeg 是否可用无关）。
- 用途（源码注释）：让"内嵌脚本"（前端）拿到运行期信息（版本、已解析路径）。
- Rust 侧必须保留这个端点与字段名（前端/健康检查/文档已依赖它）。

### 9.7 这些行为对应的 Rust 侧硬性要求

1. 绑定：默认 `127.0.0.1`；只有显式 `FFMPEGWEBUI_BIND` 才监听其它地址（安全默认）。
2. 端口：命令行 > 环境变量 > 配置文件 > 持久化偏好 > 5265；`0` 表示"挑一个空闲端口"，
   并**把最终端口写回日志/UI**（否则用户找不到地址）。
3. 自动开浏览器：默认开，但（a）开发模式默认不开，（b）`FFMPEGWEBUI_NO_BROWSER=1` 强制不开。
4. 静态资源：优先"可执行文件同目录"，避免 CWD 依赖。
5. 启动初始化必须**分步容错**，任何一步失败都要能进 UI。

---

## 10. 【Proposed】REST + WebSocket API 草图

> **本节全部是提案，不是参考实现的行为**。参考实现只有 `GET /api/health`（§9.6），
> 其余能力都通过 Blazor Server 的 SignalR circuit 直接调 C# 服务实现。
> 下面的 API 覆盖"Blazor UI 做过的**每一件事**"，目的是让 React 前端能 1:1 复刻。
>
> **为什么必须有服务端 push**：转换在后台跑，UI 只是观察者（§0 不变式 1）。
> Blazor 用 `TaskService` 的三个事件 + `StateHasChanged` 做到这一点；React 侧等价的机制
> 就是 WebSocket（或 SSE）。

### 10.1 全局约定（Proposed）

| 约定 | 决定 | 理由 |
| --- | --- | --- |
| 前缀 | `/api`（健康检查例外：保持不变） | 与现有 `/api/health` 一致 |
| JSON 键名 | **camelCase** | 与 ASP.NET 默认序列化一致，前端不变 |
| id | 24 位小写十六进制字符串 | 与 ObjectId 字符串形式一致 |
| 时间 | RFC3339 UTC，毫秒精度，结尾 `Z`（如 `2026-02-14T03:13:20.456Z`） | 存储是 UTC ms |
| 枚举 | **字符串**（`"Running"`）；请求体也接受数字 | 自解释、可调试；数量接口用 `status=Pending` 查询 |
| 时区 | 服务端不做本地化；`{date}` 类占位符在**服务端**按服务器本地时区展开（与原实现一致） | 保持 `FormatFileName` 语义 |
| 空值 | `null` 明确出现在响应里（不用"字段缺失"表示 null） | 前端类型稳定 |
| 错误 | 非 2xx：`{"error": {"code": "TEMPLATE_NOT_FOUND", "message": "模板不存在或已被删除"}}` | 原文错误文案见 §3.4/§5.4 表 |
| 分页 | `?limit=&offset=` + 响应 `{"items": [...], "total": n, "limit":, "offset":}` | 任务历史需要 |
| 幂等 | `POST /api/tasks/{id}/start` 可重复调用；已终态返回 409 + 明确 message | 防重复执行（§3.2 警告） |
| 超时 | 长操作（模板试跑/硬件检测）返回 202 + 轮询作业接口，或直接在 WS 上发进度事件 | 检测可能几十秒 |

**统一的任务对象（`TaskDto`）**——就是 §2.8 的存储形状换成 camelCase + 字符串枚举 + `durationSeconds`：

```json
{
  "id": "66f1c0a2b3d4e5f60718293a",
  "inputPath": "/Users/me/Movies/a.mp4",
  "outputPath": "/Users/me/Movies/a_converted.mp4",
  "templateId": "66f1c0a2b3d4e5f607182930",
  "templateName": "推荐 · H.264 高质量",
  "actualCommand": "-hide_banner -y -progress pipe:2 -i ... /Users/me/Movies/a_converted.mp4",
  "status": "Completed",
  "progress": 100,
  "totalDuration": 123.456,
  "currentTime": 123.456,
  "estimatedTimeRemaining": null,
  "processingSpeed": null,
  "errorMessage": null,
  "createdAt": "2026-02-14T03:12:44.123Z",
  "startedAt": "2026-02-14T03:12:45.001Z",
  "completedAt": "2026-02-14T03:13:20.456Z",
  "durationSeconds": 35.455,
  "inputFileSize": 104857600,
  "outputFileSize": 52428800,
  "batchId": null,
  "templateParameters": { "crf": "20" },
  "retryOfTaskId": null,
  "outputPathResolved": true,
  "overwriteExisting": false
}
```
> `logOutput` 默认**不出现在列表接口**里（可能很大），只在 `GET /api/tasks/{id}`、
> `GET /api/tasks/{id}/log` 和 WS 的 `task.log` 事件里给。

### 10.2 端点清单（Proposed）

**系统 / 环境**

| 方法 | 路径 | 请求 | 响应 | 对应原实现 |
| --- | --- | --- | --- | --- |
| GET | `/api/health` | — | §9.6 形状 | `Program.cs:92-106` |
| GET | `/api/system/status?force=false` | — | `AppStatusDto` | `IAppStatusService.GetStatusAsync` |
| GET | `/api/system/ffmpeg?force=false` | — | `FFmpegInfoDto` + `paths` | `IFFmpegService.GetFFmpegInfoAsync` |
| POST | `/api/system/ffmpeg/validate` | `{"path":"/opt/homebrew/bin/ffmpeg"}` | `{"valid":true,"version":"ffmpeg version 7.1.1 …"}` | `ISettingsService.ValidateFFmpegPathAsync` / `GetFFmpegVersionAsync` |
| GET | `/api/system/encoders?force=false` | — | `HardwareDetectionReportDto` | `IHardwareDetectionService.DetectAsync`（结果会落 `encoders` 表） |
| GET | `/api/system/encoders/recommend?codec=h264&required=nvenc&strict=false` | — | `{"encoder":"h264_nvenc"}` 或 `{"encoder":null,"reason":"…"}` | `GetRecommendedEncoderAsync` |
| GET | `/api/system/paths` | — | `{"dataDirectory":"…","defaultOutputDirectory":"…","platform":"macOS","isContainer":false,"containerWritable":true,"databasePath":"…","logPath":null}` | `AppPaths` + `IAppStatusService.IsDataDirectoryWritable` |

**设置**

| 方法 | 路径 | 请求/响应 |
| --- | --- | --- |
| GET | `/api/settings` | → `SettingsDto`（§2.2 字段 camelCase，字符串枚举） |
| PUT | `/api/settings` | body = 完整 `SettingsDto`（**部分更新不支持**，与原实现一致：整表替换 + `Normalize`）→ 返回 `{settings, warnings:[...]}`，`warnings` 报告被 `Normalize`/校验修正的地方 |
| POST | `/api/settings/reset` | → `SettingsDto`（新默认值） |
| GET | `/api/settings/port` | → `{"savedListenPort":5265,"effectiveListenPort":5265,"restartRequired":false}` |

`SettingsDto` 示例（字符串枚举）：

```json
{
  "ffmpegPath": "", "ffprobePath": "",
  "defaultOutputDirectory": "/Users/me/Movies",
  "outputNaming": "Suffix", "outputSuffix": "_converted",
  "fileExistsAction": "Ask",
  "preferHardwareAcceleration": true, "preferHardwareDecoding": false,
  "preferredHardwareEncoder": "", "autoFallbackToSoftware": true,
  "maxConcurrentTasks": 1, "maxLogLines": 500, "taskHistoryRetentionDays": 30,
  "theme": "System", "globalFfmpegArguments": "",
  "notifyOnCompletion": true, "autoOpenBrowser": true, "listenPort": 5265
}
```
> ⚠️ 服务端校验必须复刻 §5.4 的整张表（并发 1..8 vs 模型 1..16 的冲突需先拍板，附录 B）。

**模板**

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/templates?includeSystem=true&category=&q=` | 排序：推荐 → sortOrder → name |
| GET | `/api/templates/categories` | 分类列表（含预设顺序） |
| GET | `/api/templates/{id}` | 单模板 |
| POST | `/api/templates` | 创建用户模板（name 唯一冲突 → 409） |
| PUT | `/api/templates/{id}` | 更新（系统模板保持 `System` 类型） |
| DELETE | `/api/templates/{id}` | 只能删用户模板；系统模板 → 403 |
| POST | `/api/templates/{id}/copy` | body `{"newName":"…"}` → 新用户模板 |
| POST | `/api/templates/{id}/validate` | → `{"issues":[{"severity":"Warning","message":"…","field":"commandArgs"}]}`（同时把"本机缺少必需滤镜"合并进 issues） |
| POST | `/api/templates/{id}/test` | body `{"parameterValues":{"crf":"20"}}` → `202 {"jobId":"…"}`；完成后 WS 发 `template.test.result`；同步版本返回 `{"success":true,"message":"…","command":"…","output":"…","durationMs":12345}` |
| GET | `/api/templates/export` | → JSON 文本（可分享/备份） |
| POST | `/api/templates/import` | body `{"json":"…","asSystemTemplate":false}` → `{"imported":3}` |
| POST | `/api/templates/system/reset` | 重置系统预设 |
| POST | `/api/templates/system/sync` | 同步系统模板 → `{"added":1,"updated":2,"unchanged":8,"presetVersion":4,"summary":"系统模板已更新：新增 1 个，更新 2 个（预设 v4）"}` |

**媒体与文件**

| 方法 | 路径 | 请求/响应 | 说明 |
| --- | --- | --- | --- |
| POST | `/api/media/info` | `{"path":"/Users/me/Movies/a.mp4"}` → `MediaInfoDto`（§2.7）或 `{"info":null,"reason":"ffprobe 无输出"}` | ffprobe 探测 |
| POST | `/api/media/scan` | `{"directory":"/x","extensions":["mp4","mkv"],"recursive":false}` → `{"files":[{"path":"…","size":123,"name":"a.mp4"}]}` | `FileService.ScanMediaFiles`；排序按路径忽略大小写 |
| GET | `/api/files/extensions` | → `{"extensions":[".mp4",…]}`（26 个默认媒体扩展名） | |
| POST | `/api/files/dialog` | `{"kind":"file"\|"folder","title":"选择媒体文件","initialDirectory":"/x"}` → `{"path":"/Users/me/Movies/a.mp4"}` / `{"path":null,"reason":"cancelled"\|"unsupported"\|"timeout"}` | ⚠️ **必须在服务端执行**（§7），前提是服务端与用户同一台机器 |
| GET | `/api/files/dialog/backend` | → `{"isSupported":true,"backend":"macOS 原生选择器（osascript）"}` | 用于 UI 提前禁用"浏览"按钮 |
| POST | `/api/files/check` | `{"path":"/x","kind":"file"\|"dir"}` → `{"exists":true,"accessible":true,"writable":true,"size":123,"diskFree":123456789}` | `IsFileAccessible`/`IsDirectoryWritable`/`GetAvailableDiskSpace` |

**任务（核心）**

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/tasks/preview` | **替代 `RebuildPreviewAsync`**（关键端点，见下） |
| POST | `/api/tasks` | 创建任务（可选立即开始） |
| POST | `/api/tasks/{id}/start` | 重置为 Pending 并入队 |
| POST | `/api/tasks/{id}/cancel` | body `{"graceful":true}`（默认 false） |
| POST | `/api/tasks/{id}/retry` | 新建 + 入队（§3.12） |
| GET | `/api/tasks?status=&batchId=&limit=&offset=&order=createdAt:desc` | 列表（不含 logOutput） |
| GET | `/api/tasks/{id}` | 详情（含 logOutput） |
| GET | `/api/tasks/{id}/log?tailLines=200` | `{"taskId":"…","logOutput":"…","maxLogLines":500}` |
| GET | `/api/tasks/running` | 运行中（按 createdAt 倒序） |
| GET | `/api/tasks/pending` | 等待中（按 createdAt 正序） |
| POST | `/api/tasks/delete` | `{"ids":["…","…"]}` → `{"deleted":2,"skippedRunning":1}` |
| DELETE | `/api/tasks?status=Failed` | 按状态批量删除 → `{"deleted":5}` |
| GET | `/api/queue` | `{"queued":2,"active":1,"maxConcurrentTasks":1,"workers":1}` |

`POST /api/tasks/preview` —— 请求与响应：

```jsonc
// 请求
{
  "inputPath": "/Users/me/Movies/a.mp4",
  "outputDirectory": "/Users/me/Movies",
  "outputFileNameTemplate": "{filename}_4k",   // 可空：走命名规则
  "outputExtension": "mp4",                    // 可空：取模板 OutputExtension
  "templateId": "66f1…",
  "parameterValues": { "crf": "20" },
  "forceOverwrite": false                      // 对应 UI 的「覆盖已有文件」按钮
}
```
```jsonc
// 响应
{
  "media": { /* MediaInfoDto 或 null */ },
  "requestedOutputPath": "/Users/me/Movies/a_4k.mp4",
  "resolvedOutputPath": "/Users/me/Movies/a_4k (1).mp4",
  "overwrite": true,
  "skip": false,
  "renamed": true,
  "note": "目标文件已存在，已自动重命名为 a_4k (1).mp4。",
  "conflictNotice": "输出文件已存在，将自动改名后保存，避免覆盖原文件。",
  "pathsEqualWarning": null,
  "encoder": { "name": "h264_nvenc", "type": "Nvenc", "family": "nvenc", "codec": "h264", "qualityArgs": "-c:v h264_nvenc -preset p5 -cq {q}" },
  "commandPreview": "-hide_banner -y -progress pipe:2 -i /… -c:v h264_nvenc … /…/a_4k (1).mp4",
  "warnings": ["提示：{unknown_placeholder} 未被识别"],
  "templateRunnable": true,
  "blockingError": null
}
```
> `conflictNotice` / `pathsEqualWarning` 的文案直接取自 §4.9；
> `blockingError` 对应"严格模式缺硬件"（`该模板要求使用 {family} 硬件编码，但本机不可用。…`）与
> "输出目录不可写"。

`POST /api/tasks`：

```jsonc
// 请求
{
  "inputPath": "/Users/me/Movies/a.mp4",
  "outputPath": "/Users/me/Movies/a_4k (1).mp4",
  "templateId": "66f1…",
  "parameterValues": { "crf": "20" },
  "overwriteExisting": true,       // 非 null → 服务端置 outputPathResolved=true（§2.3）
  "startImmediately": true
}
// 响应 201
{ "task": { /* TaskDto */ }, "enqueued": true }
```

**批次**

| 方法 | 路径 | 请求/响应 |
| --- | --- | --- |
| POST | `/api/batches` | `{"inputPaths":["…","…"],"outputDirectory":"/out","templateId":"…","parameterValues":{…},"startImmediately":true}` → `{"batch":BatchDto,"tasks":[TaskDto,…]}` |
| GET | `/api/batches?limit=&offset=` | 批次列表 |
| GET | `/api/batches/{id}` | `BatchDto`（含 `totalFiles/completedFiles/failedFiles/status`） |
| GET | `/api/batches/{id}/tasks` | 子任务列表（按 createdAt 正序） |
| POST | `/api/batches/{id}/cancel` | 取消批次内 Pending+Running（Running 用非优雅） |
| POST | `/api/batches/{id}/retry-failed` | 对所有 Failed 子任务逐个 retry → `{"retried":3,"queued":3}`（UI 的"重试全部失败"） |
| DELETE | `/api/batches/{id}` | 删除批次记录（子任务另行处理） |

**历史维护**

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/history/cleanup` | `{"retentionDays":30}` 或 `{"olderThan":"2026-01-01T00:00:00Z"}` → `{"deletedTasks":12,"deletedBatches":2}` |
| POST | `/api/history/clear` | 清空全部终态任务 + 全部批次记录 → `{"deletedTasks":100}` |

### 10.3 Blazor UI → API 对照表
（确保"没有遗漏能力"的检查表）

| Blazor 页面/动作 | 原实现调用 | 提案端点 |
| --- | --- | --- |
| 首页状态卡 | `IAppStatusService.GetStatusAsync` | `GET /api/system/status` |
| 首页硬件能力 | `IHardwareDetectionService.DetectAsync` | `GET /api/system/encoders` |
| 首页最近任务 | `GetTaskHistoryAsync(5)` | `GET /api/tasks?limit=5` |
| 状态栏计数 | `TaskService.ActiveCount/QueuedCount` | `GET /api/queue` + WS `queue.changed` |
| 转换页选择输入文件 | `IFileDialogService.PickFileAsync` | `POST /api/files/dialog` |
| 转换页媒体信息 | `IFFmpegService.GetMediaInfoAsync` | `POST /api/media/info` |
| 转换页模板/参数 | `ITemplateService.*` | `GET /api/templates*` |
| 转换页命令预览 + 冲突提示 | `RebuildPreviewAsync` | `POST /api/tasks/preview` |
| "覆盖已有文件"/"保留两者" | `_forceOverwrite` + `Resolve` | `POST /api/tasks/preview` with `forceOverwrite` |
| 开始转换 | `CreateTaskAsync` + `StartTaskAsync` | `POST /api/tasks` |
| 进度/状态/日志实时 | 三个 C# 事件 | WS `task.progress` / `task.status` / `task.log` |
| 优雅取消 / 强制取消 | `CancelTaskAsync(id, graceful)` | `POST /api/tasks/{id}/cancel {"graceful":…}` |
| 重试 | `RetryTaskAsync` | `POST /api/tasks/{id}/retry` |
| 任务列表筛选/分页 | `GetTaskHistoryAsync(limit,status)` | `GET /api/tasks?...` |
| 批量：扫描目录 | `FileService.ScanMediaFiles` | `POST /api/media/scan` |
| 批量：选择文件夹 | `PickFolderAsync` | `POST /api/files/dialog {"kind":"folder"}` |
| 批量：开始/取消/重试 | `StartBatchTaskAsync`/`CancelBatchAsync`/retry | `/api/batches*` |
| 设置：读取/保存/重置 | `ISettingsService.*` | `/api/settings*` |
| 设置：验证 ffmpeg 路径 | `ValidateFFmpegPathAsync` | `POST /api/system/ffmpeg/validate` |
| 设置：重新检测硬件 | `DetectAsync(forceRefresh:true)` | `GET /api/system/encoders?force=true` |
| 设置：清理/清空历史 | `CleanupHistoryAsync`/`ClearHistoryAsync` | `/api/history/*` |
| 设置：数据目录可写 | `IAppStatusService.IsDataDirectoryWritable` | `GET /api/system/paths` |
| 模板页：校验/试跑/导入导出/复制 | `ITemplateService.*` | `/api/templates*` |
| 全局轻提示 | `IToastService` | 纯前端，不进 API；服务端错误交给 `error.message` |

### 10.4 WebSocket 协议（Proposed）

- 端点：`GET /api/ws`（Upgrade）。建议**单连接、订阅式**：
  客户端连接后可发 `{"action":"subscribe","topics":["tasks","logs","queue"]}`；
  默认订阅全部。
- 服务器在连接建立后**立即推一份快照**（避免首屏竞态）：
  ```jsonc
  { "type": "snapshot", "ts": "2026-02-14T03:13:20.456Z",
    "data": { "queue": {"queued":1,"active":1,"maxConcurrentTasks":1},
              "runningTasks": [ /* TaskDto */ ], "pendingTasks": [ /* TaskDto */ ] } }
  ```
- 事件信封统一为 `{"type","ts","data"}`。事件类型与负载（**第三个是高频事件，必须做合并**）：

```jsonc
// 进度：等价 TaskProgressEventArgs（ConversionQueue.cs:77-90）
{ "type": "task.progress", "ts": "…",
  "data": { "taskId": "66f1…", "progress": 42.5, "speed": 2.13, "eta": 33.2,
            "currentTime": 52.4, "totalDuration": 123.456 } }

// 状态：等价 TaskStatusEventArgs（oldStatus 用真实旧值，不要复制原实现的硬编码 Pending）
{ "type": "task.status", "ts": "…",
  "data": { "taskId": "66f1…", "oldStatus": "Running", "newStatus": "Completed",
            "errorMessage": null, "task": { /* TaskDto 全量，便于前端一次更新 */ } } }

// 日志：等价 TaskLogEventArgs；服务端合并（建议 100–200ms 一批、每批最多 50 行）
{ "type": "task.log", "ts": "…",
  "data": { "taskId": "66f1…", "lines": ["frame=123 … time=00:00:52.40"],
            "dropped": 0, "truncated": false } }

// 队列变化
{ "type": "queue.changed", "ts": "…", "data": { "queued": 0, "active": 2 } }

// 批次聚合变化（等价 UpdateBatchStatus 的结果）
{ "type": "batch.status", "ts": "…",
  "data": { "batchId": "66f1…", "status": "Running", "totalFiles": 10,
            "completedFiles": 4, "failedFiles": 1 } }

// 长作业（模板试跑、硬件检测）结果
{ "type": "template.test.result", "ts": "…",
  "data": { "jobId": "…", "success": false, "message": "…", "command": "…",
            "output": "…", "durationMs": 12345 } }
{ "type": "encoders.detected", "ts": "…", "data": { /* HardwareDetectionReportDto */ } }

// 服务端主动结束（例如进程即将退出）
{ "type": "server.shutdown", "ts": "…", "data": { "reason": "app exit" } }
```

- 心跳：客户端每 20s 发 `{"action":"ping"}`，服务端回 `{"type":"pong"}`；
  服务端 60s 无 ping → 关闭连接（前端自动重连 + 重放 `snapshot`）。
- 断线重连：**必须用快照恢复**（不要试图补发丢失的日志行）；`task.log` 事件里的行是"增量"，
  重连后前端用 `GET /api/tasks/{id}/log?tailLines=200` 补齐当前视图。
- 多客户端：原实现是 Blazor Server，多标签页各自有 circuit 但共享事件源；
  WS 侧建议广播给所有订阅者（无状态、不区分会话）。

### 10.5 与参考实现的关键差异（必须写进迁移说明）

| 项 | 参考实现 | 提案 | 影响 |
| --- | --- | --- | --- |
| 传输 | Blazor circuit（SignalR） | REST + WS | 前端完全重写 |
| 文件对话框 | 服务端进程调 osascript/zenity/PowerShell | `POST /api/files/dialog` 由**服务端**执行 | 若部署成"远程服务器 + 本地浏览器"则不可用；必须以"本机单用户工具"为前提（与 `AllowedHosts` 的默认仅回环一致） |
| 状态订阅 | C# 事件 → `StateHasChanged` | WS 事件 | 需处理重连与日志合并 |
| 队列长度 | 属性轮询（1s 定时器） | WS `queue.changed` | 更实时 |
| 进度持久化 | 只在日志 flush/finalize 时写库（§3.7 警告） | 建议每 1s 落一次进度字段 | 崩溃后可显示更准确的最后进度（**行为增强，需标注**） |
| 启动时 `Pending` | 不自动重跑（§3.13） | 可选自动重跑 | 若实现，需 UI 提示 |
| 回退判定 | `ProducedAnyOutput` 被提前清空（§3.6.4） | 修正 | 日志中 ⚠️ 行出现次数、是否重跑会变 |
| 数字 id | ObjectId | 建议保持 24hex 字符串 | 前端无需改；若要换 UUIDv7 需同步改所有接口 |

---

## 附录 A：源文件 → 章节索引（便于对照原文）

| 源文件 | 行数 | 本文相关章节 |
| --- | --- | --- |
| `Models/AppSettings.cs` | 1-84 | §2.2、§5.3、§5.4 |
| `Models/ConversionTask.cs` | 1-86 | §2.3、§1.5、§2.8 |
| `Models/BatchTask.cs` | 1-33 | §2.4、§3.11 |
| `Models/DTOs.cs` | 1-137 | §2.7、§3.7、§3.14 |
| `Models/Enums.cs` | 1-107 | §0.1、§2.1 |
| `Models/HardwareEncoder.cs` | 1-87 | §2.5、§3.6 |
| `Models/CommandTemplate.cs` | 1-137 | §2.6、§3.6、§4.10 |
| `Data/LiteDbContext.cs` | 1-80 | §1.1、§1.2、§1.3 |
| `Services/ConversionQueue.cs` | 1-884 | §3（全节） |
| `Services/TaskService.cs` | 1-344 | §3.2、§3.12、§3.13、§2.3 |
| `Services/ITaskService.cs` | 1-92 | §3.14、§10.3 |
| `Services/SettingsService.cs` | 1-214 | §5（全节） |
| `Services/ISettingsService.cs` | 1-25 | §5.1 |
| `Services/AppPaths.cs` | 1-143 | §6、§1.4 |
| `Services/OutputPathResolver.cs` | 1-213 | §4.1-4.4 |
| `Services/FileService.cs` | 1-256 | §4.5-4.8、§10.2 |
| `Services/IFileService.cs` | 1-41 | §4.5、§10.2 |
| `Services/FileDialogService.cs` | 1-460 | §7（全节） |
| `Services/AppStatusService.cs` | 1-164 | §8 |
| `Services/FFmpegService.cs` | 555-1042 | §3.7、§3.9、§3.10 |
| `Services/FFmpegLocator.cs` | 1-261 | §6.4 |
| `Services/EncoderCatalog.cs` | 351-377 | §3.6.1、§3.6.4 |
| `Services/CommandLineBuilder.cs` | 384-478 | §4.10 |
| `Program.cs` | 1-347 | §9（全节） |
| `appsettings.json` | — | §1.4、§9.2、§9.3 |
| `Components/Pages/Convert.razor` | 825-880、946-1060、216-226 | §4.9、§10.2 |
| `Components/Pages/Settings.razor` | 1155-1180、1555-1561、609-614 | §5.4、§4.9、§5.7 |
| `Components/Shared/FileSelector.razor` / `BatchFileList.razor` | 291-325 / 275-315 | §7.1 |

## 附录 B：Rust 侧必须拍板的决策清单

> 这些点参考实现里**要么没做、要么做得可疑**，移植前必须逐条决定并在发布说明里写清。

| # | 决策 | 参考实现的事实 | 建议 |
| --- | --- | --- | --- |
| 1 | 存储引擎 | LiteDB 单文件 `data.db` | SQLite（新文件名 `data.sqlite3`，勿复用旧文件）；是否需要一次性 LiteDB→SQLite 迁移器（默认：不自动迁移，只提示路径） |
| 2 | id 形态 | ObjectId（24hex，含时间戳前缀） | 保持 24hex（前端/日志不变）；若换 UUIDv7 需同步 API/前端 |
| 3 | 时间精度/时区 | 持久化 UTC ms；`{date}` 类占位符用**本地时间** | 持久化 UTC ms；占位符保持本地时间（否则用户命名会变） |
| 4 | 日志分隔符 | `Environment.NewLine`（Windows 为 `\r\n`） | 统一 `\n`，并在解析/展示处兼容 `\r\n` |
| 5 | 日志行上限 | `MaxLogLines`（只限行数） | 同时加字节上限（例如 2 MiB），防单行超长把库撑爆 |
| 6 | 并发上限 | 模型 1..16，UI 1..8；worker 只增不减 | 统一 1..16 或 1..8；并发上限**热调整**（`Semaphore`），并在 UI 说明 |
| 7 | 入队去重 | 无（同一 id 可被跑两次） | 必须加 `in_flight` 集合；重复 start 返回 409 或忽略 |
| 8 | `Pending` 重启 | 不自动重跑 | 建议自动重跑（需 UI 提示"已恢复 N 个排队任务"） |
| 9 | 回退判定 | `ProducedAnyOutput` 被提前清空 → 总是回退 | 修正为"每轮前置位、轮后判定"；文档标注行为差异 |
| 10 | 优雅取消 | 先发 `q`，5s 后强杀；force 时也会先发一次 `q`（副作用） | 明确三态：`graceful` / `force` / `pending`；不要复制"先 q 再 kill" |
| 11 | 部分输出清理 | < 64 MiB 才删 | 保持阈值语义；建议记录"已删除残片"到任务日志 |
| 12 | 进度落库 | 只在日志 flush 与终态时写 | 每 1s 落一次（崩溃可恢复） |
| 13 | 文件对话框 | 服务端调系统命令 | Windows 用原生对话框（`rfd`）；Linux 保持 zenity/kdialog（可选原生 GTK）；macOS 保持 osascript（或 `rfd`），返回"取消/不支持"可区分 |
| 14 | Linux 对话框回退 | zenity 取消后仍试 kdialog | 取消即返回，不再试下一个 |
| 15 | `Ask` 语义 | 预览阶段确认，执行层按 Rename | 保持；批量永不询问 |
| 16 | 端口/监听 | 默认 127.0.0.1；`FFMPEGWEBUI_BIND` 才放开；端口需重启生效 | 保持；建议额外把"实际监听地址"打在启动日志与 UI 上 |
| 17 | 静态资源 | CWD 依赖的坑（§9.1） | 嵌入二进制或固定可执行文件目录 |
| 18 | 批次统计 | `Skipped` 既不算 completed 也不算 failed | 保持，或在 API 里新增 `skippedFiles` 字段（前端展示更准确） |
| 19 | `TaskCount` 命名 | 实际是"排队+运行中" | 改名 `activeTaskCount`，并新增真正的 `taskTotalCount`（需前端配合） |
| 20 | 密码/鉴权 | 无（仅回环 + AllowedHosts） | 保持"本机工具"前提；若允许 `FFMPEGWEBUI_BIND`，必须加 token，否则任何人可执行任意 ffmpeg/读任意文件（**安全红线**） |



