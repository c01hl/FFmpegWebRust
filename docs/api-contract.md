# HTTP / WebSocket 接口契约（已冻结）

本文件是 **Rust 服务端与 React 前端之间的唯一契约**。任何一方需要改接口，先改本文件。

详细字段语义、逐字文案与参考实现的对应关系见
[`backend-data.md`](reference/backend-data.md) §10（提案原文）与
[`ui-spec.md`](reference/ui-spec.md) §11。本文件在其基础上**定稿**，差异以本文件为准。

## 0. 全局约定

| 项 | 约定 |
| --- | --- |
| 前缀 | `/api` |
| JSON 键名 | camelCase |
| id | 24 位小写十六进制（与参考实现 ObjectId 的字符串形式一致） |
| 时间 | RFC3339 UTC，毫秒精度，以 `Z` 结尾（如 `2026-02-14T03:13:20.456Z`） |
| 枚举 | **字符串**（`"Running"`、`"Nvenc"`、`"Suffix"`…），取值名与参考实现枚举名一致；请求体也接受数字 |
| null | 显式输出 `null`，不用「字段缺失」表示 |
| 错误 | 非 2xx：`{"error":{"code":"TEMPLATE_NOT_FOUND","message":"模板不存在或已被删除"}}` |
| 分页 | `?limit=&offset=` → `{"items":[…],"total":n,"limit":l,"offset":o}` |
| 端口 | 默认 **5266**（与参考实现的 5265 错开，方便两个应用同时运行） |
| 静态资源 | 前端构建产物内嵌在二进制里；除 `/api/*` 外全部回落到 `index.html`（SPA 路由） |

### 0.1 本期对参考实现的**有意修正**（需在 README 的「与参考实现的差异」中列出）

1. **硬件→软件回退判定**：参考实现的 `ProducedAnyOutput` 在判定前被清空，导致「已经产出文件就不回退」形同虚设。本实现按注释表达的意图修正：只有**没有产出任何输出文件**时才回退重试。
2. **运行期进度落库**：参考实现只在结束时写进度。本实现每 1 秒落一次 `progress/current_time`（日志仍是 700ms 节流），崩溃后能显示更准确的中断位置。
3. **入队去重**：同一 `taskId` 不会有两个 worker 同时执行。
4. **并发上限**：统一取模型层的 `1..16`（参考实现是模型 1..16 / 表单 1..8 两套）。
5. **日志推送合并**：WS 的 `task.log` 在服务端合并（100–200ms 一批，每批最多 50 行），避免高频 stderr 打满前端。

## 1. 端点清单

### 1.1 系统与环境

| 方法 | 路径 | 请求 | 响应 |
| --- | --- | --- | --- |
| GET | `/api/health` | — | `{"status":"ok","version","platform","ffmpeg","ffmpegResolved","ffprobe","ffprobeResolved"}` |
| GET | `/api/system/status?force=false` | — | `AppStatusDto` |
| GET | `/api/system/ffmpeg?force=false` | — | `FFmpegInfoDto`（含 `paths`） |
| POST | `/api/system/ffmpeg/validate` | `{"path"}` | `{"valid":bool,"version":"…"}` |
| GET | `/api/system/encoders?force=false` | — | `HardwareReportDto`（同步返回，首次可能耗时数十秒） |
| GET | `/api/system/encoders/recommend?codec=h264&required=nvenc` | — | `{"encoder":"h264_nvenc"}` 或 `{"encoder":null,"reason":"…"}` |
| GET | `/api/system/paths` | — | `{"dataDirectory","defaultOutputDirectory","databasePath","platform","isContainer","dataDirectoryWritable"}` |
| GET | `/api/system/queue` | — | `{"queued","active","maxConcurrentTasks"}` |

`AppStatusDto`：

```jsonc
{
  "ffmpegAvailable": true, "ffmpegVersion": "9.0.2", "ffmpegPath": "/opt/homebrew/bin/ffmpeg",
  "ffprobePath": "/opt/homebrew/bin/ffprobe", "platform": "macOS",
  "dataDirectory": "/Users/me/Library/Application Support/FFmpegWebRust",
  "defaultOutputDirectory": "/Users/me/Movies",
  "availableHardwareEncoderCount": 3, "templateCount": 52, "taskCount": 12,
  "hwaccels": ["videotoolbox"], "notices": ["…"],
  "lastHardwareDetection": "2026-02-14T03:12:44.123Z"
}
```

### 1.2 设置

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/settings` | → `SettingsDto` |
| PUT | `/api/settings` | body = 完整 `SettingsDto`（整表替换 + normalize）→ `{"settings":SettingsDto,"warnings":["…"]}` |
| POST | `/api/settings/reset` | → `SettingsDto`（并重新同步系统模板/硬件缓存失效） |
| GET | `/api/settings/port` | → `{"savedListenPort":5266,"effectiveListenPort":5266,"restartRequired":false}` |

`SettingsDto` 字段名 = `models::AppSettings` 的 camelCase 序列化结果（见
[`src/models/settings.rs`](../src/models/settings.rs)）。枚举字段 `outputNaming` / `fileExistsAction`
序列化为 `"Suffix"` / `"Ask"` 等字符串。

### 1.3 模板

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/templates?includeSystem=true&category=&q=` | 排序：推荐 → sortOrder → name；每项附 `available` + `unavailableReason` |
| GET | `/api/templates/categories` | `[{"name":"视频转码","count":12},…]` |
| GET | `/api/templates/{id}` | 单模板 |
| POST | `/api/templates` | 创建用户模板；重名 → 409 |
| PUT | `/api/templates/{id}` | 更新 |
| DELETE | `/api/templates/{id}` | 系统模板 → 403 |
| POST | `/api/templates/{id}/copy` | `{"newName"}` → 新用户模板 |
| POST | `/api/templates/validate` | body = 模板对象 → `{"issues":[{"severity":"Error","message":"…","field":"commandArgs"}]}` |
| POST | `/api/templates/{id}/test` | `{"parameterValues":{…}}` → `TemplateTestResult`（**同步**，最长约 30s） |
| GET | `/api/templates/export?ids=a,b` | → `{"json":"…","fileName":"ffmpeg-templates.json"}` |
| POST | `/api/templates/import` | `{"json":"…","asSystemTemplate":false}` → `{"imported":3,"skipped":1,"errors":[]}` |
| POST | `/api/templates/system/reset` | → `{"presetVersion":4,"summary":"…"}` |
| POST | `/api/templates/system/sync` | → `{"added":1,"updated":2,"unchanged":8,"presetVersion":4,"summary":"…"}` |

模板对象 = `models::CommandTemplate` 的 camelCase 序列化结果。**唯一例外**：`type` 字段
（`System`/`User`）在 API 中叫 `type`，在 Rust 里是 `r#type`。

模板列表项附加字段（服务端计算，前端不自己算）：

```jsonc
{
  "available": true,
  "unavailableReason": null,   // 例如 "需要 NVIDIA NVENC，本机不可用"
  "missingFilters": [],        // 例如 ["libass"]
  "inputFormatsDisplay": "任意格式"
}
```

### 1.4 媒体与文件

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/media/info` | `{"path"}` → `{"info":MediaInfoDto\|null,"reason":null\|"ffprobe 无输出"}` |
| POST | `/api/media/scan` | `{"directory","extensions":[],"recursive":false}` → `{"files":[{"path","name","size"}]}` |
| GET | `/api/files/extensions` | → `{"extensions":["mp4","mkv",…]}` |
| POST | `/api/files/dialog` | `{"kind":"file"\|"folder","title","initialDirectory"}` → `{"path":"…"}` 或 `{"path":null,"reason":"cancelled"\|"unsupported"\|"timeout"}` |
| GET | `/api/files/dialog/backend` | → `{"isSupported":true,"backend":"macOS 原生选择器（osascript）"}` |
| POST | `/api/files/check` | `{"path","kind"}` → `{"exists","accessible","writable","size","diskFree"}` |
| POST | `/api/files/reveal` | `{"path"}` → 在系统文件管理器中打开（失败时 `{"ok":false,"reason":"…"}`） |

### 1.5 任务

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/tasks/preview` | 命令预览 + 输出路径冲突解析（**关键端点**，前端不做任何本地推断） |
| POST | `/api/tasks` | 创建任务 → `201 {"task":TaskDto,"enqueued":bool}` |
| POST | `/api/tasks/{id}/start` | 入队；已终态 → 409 |
| POST | `/api/tasks/{id}/cancel` | `{"graceful":false}` |
| POST | `/api/tasks/{id}/retry` | 新建 + 入队 → `{"task":TaskDto}` |
| GET | `/api/tasks?status=&batchId=&limit=&offset=` | `{"items":[TaskDto…],"total"}`（**不含** `logOutput`） |
| GET | `/api/tasks/{id}` | `TaskDto`（含 `logOutput`） |
| GET | `/api/tasks/{id}/log?tailLines=200` | `{"taskId","logOutput","maxLogLines"}` |
| GET | `/api/tasks/running` | `[TaskDto…]` |
| GET | `/api/tasks/pending` | `[TaskDto…]` |
| POST | `/api/tasks/delete` | `{"ids":[…]}` → `{"deleted":2,"skippedRunning":1}` |
| DELETE | `/api/tasks?status=Failed` | → `{"deleted":5}` |

`POST /api/tasks/preview` 请求/响应（逐字段，前端直接渲染）：

```jsonc
// 请求
{
  "inputPath": "/Users/me/Movies/a.mp4",
  "outputDirectory": "/Users/me/Movies",
  "outputFileNameTemplate": null,     // 可空：走设置里的命名规则
  "outputExtension": null,            // 可空：取模板 outputExtension
  "templateId": "66f1…",
  "parameterValues": { "crf": "20" },
  "overwriteExisting": false,         // 界面上的「覆盖已有文件」
  "allowRename": true
}
// 响应
{
  "media": { /* MediaInfoDto 或 null */ },
  "requestedOutputPath": "/Users/me/Movies/a_converted.mp4",
  "resolvedOutputPath": "/Users/me/Movies/a_converted (1).mp4",
  "renamed": true, "skip": false, "overwrite": false,
  "conflictNotice": "输出文件已存在，将自动改名后保存，避免覆盖原文件。",
  "pathsEqualWarning": null,
  "outputDirectoryWritable": true,
  "encoder": { "name": "h264_videotoolbox", "displayName": "…", "type": "VideoToolbox", "family": "videotoolbox", "codec": "h264", "qualityArgs": "-q:v {q}", "qualityHint": "数值越小画质越好", "isHardware": true },
  "commandPreview": "-hide_banner -y -progress pipe:2 -i … -c:v h264_videotoolbox … /Users/me/Movies/a_converted (1).mp4",
  "warnings": ["以下占位符没有对应取值，将原样传给 FFmpeg，可能导致失败：{foo}"],
  "blockingError": null,              // 例如「该模板要求使用 nvenc 硬件编码，但本机不可用。」
  "templateRunnable": true,
  "usesHardwareAcceleration": true,
  "templateName": "推荐 · H.264 高质量",
  "issues": [ { "severity": "Warning", "message": "…", "field": null } ]
}
```

### 1.6 批次

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/batches` | `{"inputPaths":[…],"outputDirectory","templateId","parameterValues":{},"startImmediately":true}` → `{"batch":BatchDto,"tasks":[TaskDto…]}` |
| GET | `/api/batches?limit=&offset=` | `{"items":[BatchDto…],"total"}` |
| GET | `/api/batches/{id}` | `BatchDto` |
| GET | `/api/batches/{id}/tasks` | `[TaskDto…]`（按 createdAt 正序） |
| POST | `/api/batches/{id}/cancel` | → `{"cancelled":3}` |
| POST | `/api/batches/{id}/retry-failed` | → `{"retried":3,"queued":3}` |
| DELETE | `/api/batches/{id}` | → `{"deletedTasks":10}` |

### 1.7 历史

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/history/cleanup` | `{"retentionDays":30}` → `{"deletedTasks":12,"deletedBatches":2}` |
| POST | `/api/history/clear` | → `{"deletedTasks":100}` |

## 2. WebSocket 协议

端点：`GET /api/ws`（Upgrade）。**单连接、广播式**，服务端不区分会话。

客户端可发：

```jsonc
{ "action": "subscribe", "topics": ["tasks","logs","queue"] }  // 默认全部
{ "action": "ping" }
{ "action": "task.cancel", "taskId": "…", "graceful": true }   // 可选便捷通道
```

服务端连接建立后**立即推快照**（避免首屏竞态）：

```jsonc
{ "type": "snapshot", "ts": "…", "data": {
    "queue": { "queued": 1, "active": 1, "maxConcurrentTasks": 1 },
    "runningTasks": [ /* TaskDto */ ], "pendingTasks": [ /* TaskDto */ ] } }
```

事件信封统一为 `{"type","ts","data"}`：

| type | data |
| --- | --- |
| `task.progress` | `{taskId, progress, speed, eta, currentTime, totalDuration}` |
| `task.status` | `{taskId, oldStatus, newStatus, errorMessage, task: TaskDto}` |
| `task.log` | `{taskId, lines: [String], dropped: n, truncated: bool}`（服务端已合并） |
| `queue.changed` | `{queued, active}` |
| `batch.status` | `{batchId, status, totalFiles, completedFiles, failedFiles}` |
| `encoders.detected` | `HardwareReportDto` |
| `settings.changed` | `SettingsDto` |
| `templates.changed` | `{reason: "import"\|"sync"\|"crud", count}` |
| `pong` | `{}` |
| `server.shutdown` | `{reason}` |

心跳：客户端 20s 一次 `ping`；服务端 90s 未收到任何消息则关闭。前端断线后自动重连并**依赖快照恢复**，
日志用 `GET /api/tasks/{id}/log?tailLines=200` 补齐。

## 3. DTO 与 Rust 类型的对应

| DTO | Rust 类型 |
| --- | --- |
| `TaskDto` | `models::ConversionTask`（+ 计算字段 `durationSeconds`、`logLines`） |
| `BatchDto` | `models::BatchTask` |
| `SettingsDto` | `models::AppSettings` |
| 模板 | `models::CommandTemplate` |
| `HardwareReportDto` | `ffmpeg::hardware::HardwareReport` |
| `MediaInfoDto` | `ffmpeg::service::MediaInfo` |
| `FFmpegInfoDto` | `ffmpeg::service::FFmpegInfo` + `paths: ffmpeg::locator::FFmpegPaths` |
| `TemplateTestResult` | `models::dto::TemplateTestResult` |

所有 DTO 都带 `#[serde(rename_all = "camelCase")]`。**新增字段时必须同步本文件与
`web/src/api/types.ts`。**
