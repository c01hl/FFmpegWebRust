/**
 * REST / WebSocket 契约对应的 TypeScript 类型。
 *
 * 来源：`docs/api-contract.md`（冻结契约）+ `src/models/*.rs`（Rust 侧的 serde 结构）。
 * 约定（与契约 §0 一致）：
 * - 前缀 `/api`，JSON 键名 camelCase；
 * - id 是 24 位小写十六进制字符串；
 * - 时间是 RFC3339 UTC 毫秒精度、以 `Z` 结尾（如 `2026-02-14T03:13:20.456Z`），前端用 `lib/format` 转本地时间；
 * - 枚举是**字符串**（`"Running"`、`"Nvenc"`、`"Suffix"`…），取值名与 Rust 枚举名一致；请求体也接受数字；
 * - null 是显式输出，不用「字段缺失」表示。
 *
 * ⚠️ 新增字段时必须同步 `docs/api-contract.md` 与本文件。
 */

/* ══════════════════════════════════════════════════════════════════════════
   基础类型
   ══════════════════════════════════════════════════════════════════════════ */

/** 24 位小写十六进制 id */
export type Id = string
/** RFC3339 UTC 时间串 */
export type Timestamp = string

/** 分页响应（契约 §0：`{"items":[…],"total":n,"limit":l,"offset":o}`） */
export interface Paged<T> {
  items: T[]
  total: number
  limit: number
  offset: number
}

/** 非 2xx 的错误体：`{"error":{"code":"TEMPLATE_NOT_FOUND","message":"模板不存在或已被删除"}}` */
export interface ApiErrorBody {
  error: {
    code: string
    message: string
  }
}

/* ══════════════════════════════════════════════════════════════════════════
   枚举（字符串字面量联合；取值名与 Rust 枚举名一致）
   ══════════════════════════════════════════════════════════════════════════ */

/** 任务状态（models::enums::TaskStatus） */
export type TaskStatus = 'Pending' | 'Running' | 'Completed' | 'Failed' | 'Cancelled' | 'Skipped'

/** 模板类型（`type` 字段；Rust 里是 `r#type`） */
export type TemplateType = 'System' | 'User'

/** 模板硬件加速策略 */
export type HardwareAccelerationMode = 'None' | 'Auto' | 'Required'

/** 输出命名规则 */
export type OutputNamingRule = 'Original' | 'Suffix'

/** 输出文件已存在时的处理方式 */
export type FileExistsAction = 'Ask' | 'Overwrite' | 'Skip' | 'Rename'

/** 模板参数控件类型 */
export type TemplateParameterKind = 'Select' | 'Number' | 'Text' | 'Boolean'

/** 模板校验问题严重程度 */
export type TemplateIssueSeverity = 'Info' | 'Warning' | 'Error'

/** 编码器厂商/类型（models::ffmpeg::encoder_catalog::EncoderType） */
export type EncoderType =
  | 'Software'
  | 'Nvenc'
  | 'Qsv'
  | 'Amf'
  | 'VideoToolbox'
  | 'Vaapi'
  | 'Vulkan'
  | 'MediaFoundation'
  | 'Other'

/** 数据库里主题的三档取值（`SettingsDto.theme`，**首字母大写**；前端 store 用的小写见 store/theme.ts） */
export type ApiThemeMode = 'Light' | 'Dark' | 'System'

/** 全部任务状态，便于页面遍历渲染 */
export const ALL_TASK_STATUSES: readonly TaskStatus[] = [
  'Pending',
  'Running',
  'Completed',
  'Failed',
  'Cancelled',
  'Skipped',
]

/* ══════════════════════════════════════════════════════════════════════════
   1.1 系统与环境
   ══════════════════════════════════════════════════════════════════════════ */

/** `GET /api/health` */
export interface HealthDto {
  status: string
  version: string
  platform: string
  ffmpeg: string
  ffmpegResolved: boolean
  ffprobe: string
  ffprobeResolved: boolean
}

/** `GET /api/system/status` */
export interface AppStatusDto {
  /** 服务端版本（`Cargo.toml`，形如 `0.1.0`） */
  version?: string
  ffmpegAvailable: boolean
  ffmpegVersion: string
  ffmpegPath: string
  ffprobePath: string
  /** `Windows` / `macOS` / `Linux` */
  platform: string
  dataDirectory: string
  defaultOutputDirectory: string
  availableHardwareEncoderCount: number
  templateCount: number
  taskCount: number
  /** 例如 `["videotoolbox"]` */
  hwaccels: string[]
  /** 需要提醒用户的中文提示（可能为空数组） */
  notices: string[]
  lastHardwareDetection: Timestamp | null
}

/** FFmpeg 可执行文件解析结果（`ffmpeg::locator::FFmpegPaths` 的 camelCase 投影） */
export interface FFmpegPathsDto {
  ffmpeg: string
  ffprobe: string
  isAutoDetected: boolean
  /** 定位失败时的中文提示（可选字段，服务端未实现时缺失） */
  hint?: string
}

/** `GET /api/system/ffmpeg` = FFmpegInfo + paths */
export interface FFmpegInfoDto {
  /** `ffmpeg version` 后首个 token；解析不到是 `Unknown` */
  version: string
  path: string
  configuration: string
  supportedFormats: string[]
  supportedCodecs: string[]
  supportedEncoders: string[]
  supportedHwaccels: string[]
  supportedFilters: string[]
  /** 从 version 里取出的主版本号，解析失败为 0 */
  majorVersion: number
  isAutoDetected: boolean
  /** `version` 非空且 != `Unknown` */
  isAvailable: boolean
  paths: FFmpegPathsDto
}

/** `POST /api/system/ffmpeg/validate` */
export interface FFmpegValidateResultDto {
  valid: boolean
  version: string
}

/** 一条编码器检测结果（`models::hardware::HardwareEncoder`） */
export interface HardwareEncoderDto {
  id: Id
  /** FFmpeg 编码器名，如 `h264_nvenc` */
  name: string
  /** 展示名，如 `NVIDIA NVENC H.264` */
  displayName: string
  type: EncoderType
  /** 厂商标识：`nvenc/qsv/amf/videotoolbox/vaapi/vulkan/mf/software` */
  family: string
  /** 目标编码格式：`h264/hevc/av1/vp9/…` */
  codec: string
  isAvailable: boolean
  /** 是否通过了带质量参数的完整试跑；false = 只能用最简参数 */
  supportsQualityOptions: boolean
  lastCheckedAt: Timestamp
  /** 不可用原因（可用时为 null） */
  unavailableReason: string | null
  /** FFmpeg 构建里根本没有这个编码器 */
  missingFromBuild: boolean
  /** 检测时使用的 FFmpeg 版本 */
  ffmpegVersion: string
  /** 是否硬件编码器（计算属性，服务端可能不给，用 isHardwareEncoder() 兜底） */
  isHardware?: boolean
  /** 兼容旧字段：恒为 `[codec]` */
  supportedCodecs?: string[]
}

/** `GET /api/system/encoders` → HardwareReportDto（`ffmpeg::hardware::HardwareReport`） */
export interface HardwareReportDto {
  /** 全部编码器（含不可用），已按「硬件在前 + family + codec + name」排序 */
  encoders: HardwareEncoderDto[]
  detectedAt: Timestamp
  ffmpegVersion: string
  ffmpegPath: string
  platform: string
  /** FFmpeg 支持的 `-hwaccel` 方法 */
  hwaccels: string[]
  /** FFmpeg 支持的滤镜名（空数组 = 未知，放行） */
  supportedFilters: string[]
  /** 整体提示文案（中文） */
  summary: string
}

/** 推荐的编码器：`{"encoder":"h264_nvenc"}` 或 `{"encoder":null,"reason":"…"}` */
export interface RecommendedEncoderDto {
  encoder: string | null
  reason?: string | null
}

/** `GET /api/system/paths` */
export interface SystemPathsDto {
  dataDirectory: string
  defaultOutputDirectory: string
  databasePath: string
  platform: string
  isContainer: boolean
  dataDirectoryWritable: boolean
}

/** `GET /api/system/queue` */
export interface QueueStatusDto {
  queued: number
  active: number
  maxConcurrentTasks: number
}

/* ══════════════════════════════════════════════════════════════════════════
   1.2 设置
   ══════════════════════════════════════════════════════════════════════════ */

/** `GET/PUT /api/settings`（models::AppSettings 的 camelCase 序列化结果，整表读写） */
export interface SettingsDto {
  id: Id
  /** 空 = 自动探测 */
  ffmpegPath: string
  /** 空 = 自动探测 */
  ffprobePath: string
  defaultOutputDirectory: string
  outputNaming: OutputNamingRule
  outputSuffix: string
  fileExistsAction: FileExistsAction
  preferHardwareAcceleration: boolean
  preferHardwareDecoding: boolean
  /** 首选硬件编码器名（具体名，不是族名）；空 = 自动 */
  preferredHardwareEncoder: string
  autoFallbackToSoftware: boolean
  /** 1..16 */
  maxConcurrentTasks: number
  /** 50..20000 */
  maxLogLines: number
  /** 0..3650，0 = 不清理 */
  taskHistoryRetentionDays: number
  /** `Light` / `Dark` / `System` */
  theme: ApiThemeMode
  /** 追加到所有 ffmpeg 命令的全局参数 */
  globalFfmpegArguments: string
  notifyOnCompletion: boolean
  autoOpenBrowser: boolean
  /** 0 = 自动挑一个空闲端口；0..65535 */
  listenPort: number
}

/** `PUT /api/settings` 响应 */
export interface SettingsUpdateResultDto {
  settings: SettingsDto
  /** 规范化时产生的中文提示 */
  warnings: string[]
}

/** `GET /api/settings/port` */
export interface SettingsPortDto {
  savedListenPort: number
  effectiveListenPort: number
  restartRequired: boolean
}

/* ══════════════════════════════════════════════════════════════════════════
   1.3 模板
   ══════════════════════════════════════════════════════════════════════════ */

/** 模板参数选项 */
export interface TemplateParameterOption {
  /** 提交给 FFmpeg 的值 */
  value: string
  /** 界面显示文本 */
  label: string
  /** 补充说明 */
  description: string
}

/** 模板可调参数定义 */
export interface TemplateParameter {
  /** 占位符名称（不含花括号），如 `crf` */
  name: string
  label: string
  description: string
  kind: TemplateParameterKind
  defaultValue: string
  /** `kind === 'Select'` 时使用 */
  options: TemplateParameterOption[]
  min: number | null
  max: number | null
  trueValue: string
  /** 空字符串表示移除该片段 */
  falseValue: string
}

/** 模板校验/预览问题 */
export interface TemplateIssue {
  severity: TemplateIssueSeverity
  /** 用户可见的中文说明 */
  message: string
  /** 关联字段（可选，界面用来高亮） */
  field?: string | null
}

/** `models::CommandTemplate` 的 camelCase 序列化结果 */
export interface CommandTemplate {
  id: Id
  name: string
  description: string
  /** 支持占位符的 FFmpeg 参数模板 */
  commandArgs: string
  /** 系统预设 / 用户模板 */
  type: TemplateType
  /** 视频转码 / 音频提取 / 压缩 / 分辨率 / 硬件加速 … */
  category: string
  /** 空数组 = 不限 */
  supportedInputFormats: string[]
  /** 不含点 */
  outputExtension: string
  /** 目标视频编码格式（h264/hevc/av1/vp9/vp8/prores） */
  targetCodec: string
  /** 旧字段，保留兼容 */
  requiresHardwareAcceleration: boolean
  hardwareMode: HardwareAccelerationMode
  /** 族名（`nvenc`…）或具体名（`h264_nvenc`） */
  requiredEncoder: string | null
  parameters: TemplateParameter[]
  requiredFilters: string[]
  tags: string[]
  isRecommended: boolean
  presetVersion: number
  createdAt: Timestamp
  updatedAt: Timestamp
  /** 越小越靠前 */
  sortOrder: number
}

/** 模板列表项 = CommandTemplate + 服务端计算出的可用性字段（前端不自己算） */
export interface TemplateListItem extends CommandTemplate {
  available: boolean
  /** 例如「需要 NVIDIA NVENC，本机不可用」 */
  unavailableReason: string | null
  /** 例如 `["libass"]` */
  missingFilters: string[]
  /** 例如「任意格式」 */
  inputFormatsDisplay: string
}

/** `GET /api/templates/categories` */
export interface TemplateCategoryDto {
  name: string
  count: number
}

/** `GET /api/templates?includeSystem=&category=&q=` 的查询参数 */
export interface TemplateListQuery {
  includeSystem?: boolean
  category?: string
  q?: string
}

/** `POST /api/templates/validate` */
export interface TemplateValidateResultDto {
  issues: TemplateIssue[]
}

/** `POST /api/templates/{id}/test` 请求体 */
export interface TemplateTestRequest {
  parameterValues: Record<string, string>
}

/** 模板试运行结果（models::dto::TemplateTestResult；耗时单位是**毫秒**） */
export interface TemplateTestResultDto {
  success: boolean
  /** 用户可见的中文说明 */
  message: string
  /** 实际执行的完整命令 */
  command: string
  /** ffmpeg 输出（尾部截断） */
  output: string
  durationMs: number
}

/** `GET /api/templates/export` */
export interface TemplateExportDto {
  json: string
  fileName: string
}

/** `POST /api/templates/import` 请求体 */
export interface TemplateImportRequest {
  json: string
  asSystemTemplate: boolean
}

/** `POST /api/templates/import` 响应 */
export interface TemplateImportResultDto {
  imported: number
  skipped: number
  errors: string[]
}

/** `POST /api/templates/system/reset` 响应 */
export interface TemplateSystemResetResultDto {
  presetVersion: number
  summary: string
}

/** `POST /api/templates/system/sync` 响应 */
export interface TemplateSystemSyncResultDto {
  added: number
  updated: number
  unchanged: number
  presetVersion: number
  summary: string
}

/* ══════════════════════════════════════════════════════════════════════════
   1.4 媒体与文件
   ══════════════════════════════════════════════════════════════════════════ */

/** 媒体流信息（models::ffmpeg::service::MediaStreamInfo） */
export interface MediaStreamInfoDto {
  index: number
  /** `video` / `audio` / `subtitle` … */
  codecType: string
  codecName: string | null
  codecLongName: string | null
  width: number
  height: number
  frameRate: number
  duration: number
  channels: number
  sampleRate: number
  bitRate: number
  rotation: number
  pixelFormat: string | null
}

/** 媒体文件信息（MediaInfoDto） */
export interface MediaInfoDto {
  filePath: string
  duration: number
  format: string
  videoCodec: string | null
  audioCodec: string | null
  width: number
  height: number
  frameRate: number
  fileSize: number
  /** 友好格式名，如 `MP4` */
  formatDisplay: string
  bitRate: number
  audioChannels: number
  audioSampleRate: number
  rotation: number
  pixelFormat: string | null
  hasVideo: boolean
  hasAudio: boolean
  subtitleStreamCount: number
  streams: MediaStreamInfoDto[]
}

/** `POST /api/media/info` → `{"info":…|null,"reason":null|"ffprobe 无输出"}` */
export interface MediaInfoResultDto {
  info: MediaInfoDto | null
  reason: string | null
}

/** `POST /api/media/scan` 请求体 */
export interface MediaScanRequest {
  directory: string
  /** 空数组 = 用默认媒体扩展名 */
  extensions: string[]
  recursive: boolean
}

/** `POST /api/media/scan` 响应里的文件项 */
export interface ScannedFileDto {
  path: string
  name: string
  size: number
}

/** `POST /api/media/scan` 响应 */
export interface MediaScanResultDto {
  files: ScannedFileDto[]
}

/** `GET /api/files/extensions` */
export interface FileExtensionsDto {
  extensions: string[]
}

/** `POST /api/files/dialog` 请求体 */
export interface FileDialogRequest {
  kind: 'file' | 'folder'
  title?: string
  initialDirectory?: string
}

/** `POST /api/files/dialog` 响应（取消/不支持/超时会给出 reason） */
export interface FileDialogResultDto {
  path: string | null
  reason?: 'cancelled' | 'unsupported' | 'timeout' | null
}

/** `GET /api/files/dialog/backend` */
export interface FileDialogBackendDto {
  isSupported: boolean
  /** 例如「macOS 原生选择器（osascript）」 */
  backend: string
}

/** `POST /api/files/check` 请求体 */
export interface FileCheckRequest {
  path: string
  kind: 'file' | 'folder'
}

/** `POST /api/files/check` 响应 */
export interface FileCheckResultDto {
  exists: boolean
  accessible: boolean
  writable: boolean
  size: number
  diskFree: number
}

/** `POST /api/files/reveal` 响应（失败时给 reason） */
export interface RevealResultDto {
  ok?: boolean
  reason?: string | null
}

/* ══════════════════════════════════════════════════════════════════════════
   1.5 任务
   ══════════════════════════════════════════════════════════════════════════ */

/** models::ConversionTask 的 camelCase 序列化 + 计算字段 */
export interface TaskDto {
  id: Id
  inputPath: string
  outputPath: string
  templateId: Id
  /** 模板名快照（模板被删后仍可展示） */
  templateName: string
  /** 实际执行的 ffmpeg 参数串（不含可执行文件名） */
  actualCommand: string
  status: TaskStatus
  /** 0..100 */
  progress: number
  /** 输入总时长（秒） */
  totalDuration: number
  /** 已处理到的秒数 */
  currentTime: number
  estimatedTimeRemaining: number | null
  processingSpeed: number | null
  /** 日志文本（多行）；⚠️ 列表端点**不返回**该字段 */
  logOutput?: string
  errorMessage: string | null
  createdAt: Timestamp
  startedAt: Timestamp | null
  completedAt: Timestamp | null
  inputFileSize: number
  outputFileSize: number | null
  batchId: Id | null
  /** 创建任务时用户选定的参数（重试时复用） */
  templateParameters: Record<string, string> | null
  retryOfTaskId: Id | null
  /** 输出路径已由界面完成冲突解析 */
  outputPathResolved: boolean
  overwriteExisting: boolean
  /** 计算字段：实际耗时秒数（起止时间都有值时才返回） */
  durationSeconds?: number | null
  /** 计算字段：日志行数 */
  logLines?: number
}

/** `POST /api/tasks/preview` 请求体 */
export interface TaskPreviewRequest {
  inputPath: string
  outputDirectory: string
  /** 可空：走设置里的命名规则 */
  outputFileNameTemplate?: string | null
  /** 可空：取模板 outputExtension */
  outputExtension?: string | null
  templateId: Id
  parameterValues: Record<string, string>
  /** 界面上的「覆盖已有文件」 */
  overwriteExisting: boolean
  allowRename: boolean
}

/** 预览里解析出的编码器画像（EncoderProfile 的投影） */
export interface EncoderProfileDto {
  name: string
  displayName: string
  type: EncoderType
  family: string
  codec: string
  /** 含 `{q}` 占位，已按质量值渲染 */
  qualityArgs: string
  qualityHint: string
  isHardware: boolean
}

/** `POST /api/tasks/preview` 响应（前端直接渲染，不做任何本地推断） */
export interface TaskPreviewDto {
  media: MediaInfoDto | null
  requestedOutputPath: string
  resolvedOutputPath: string
  renamed: boolean
  skip: boolean
  overwrite: boolean
  /** 例如「输出文件已存在，将自动改名后保存，避免覆盖原文件。」 */
  conflictNotice: string | null
  pathsEqualWarning: string | null
  outputDirectoryWritable: boolean
  encoder: EncoderProfileDto | null
  /** 完整命令（不含 `ffmpeg ` 前缀） */
  commandPreview: string
  warnings: string[]
  /** 例如「该模板要求使用 nvenc 硬件编码，但本机不可用。」 */
  blockingError: string | null
  templateRunnable: boolean
  usesHardwareAcceleration: boolean
  templateName: string
  issues: TemplateIssue[]
}

/** `POST /api/tasks` 请求体 */
export interface TaskCreateRequest {
  inputPath: string
  outputPath: string
  templateId: Id
  templateName?: string
  parameterValues?: Record<string, string>
  /** 输出路径是否已由预览解析过 */
  outputPathResolved?: boolean
  overwriteExisting?: boolean
  batchId?: Id | null
  startImmediately?: boolean
}

/** `POST /api/tasks` 响应（201） */
export interface TaskCreateResultDto {
  task: TaskDto
  enqueued: boolean
}

/** `POST /api/tasks/{id}/retry` 响应 */
export interface TaskRetryResultDto {
  task: TaskDto
}

/** `POST /api/tasks/{id}/cancel` 请求体 */
export interface TaskCancelRequest {
  graceful: boolean
}

/** `GET /api/tasks/{id}/log` 响应 */
export interface TaskLogDto {
  taskId: Id
  logOutput: string
  maxLogLines: number
}

/** `GET /api/tasks` 查询参数 */
export interface TaskListQuery {
  status?: TaskStatus
  batchId?: Id
  limit?: number
  offset?: number
}

/** `POST /api/tasks/delete` 请求体 */
export interface TaskDeleteRequest {
  ids: Id[]
}

/** `POST /api/tasks/delete` 响应 */
export interface TaskDeleteResultDto {
  deleted: number
  skippedRunning: number
}

/** `DELETE /api/tasks?status=` 响应 */
export interface TaskDeleteByStatusResultDto {
  deleted: number
}

/* ══════════════════════════════════════════════════════════════════════════
   1.6 批次
   ══════════════════════════════════════════════════════════════════════════ */

/** models::BatchTask */
export interface BatchDto {
  id: Id
  /** 创建时固定为 `批量任务 - {yyyy-MM-dd HH:mm}`（本地时间） */
  name: string
  templateId: Id
  totalFiles: number
  completedFiles: number
  /** 状态为 Failed 或 Cancelled 的子任务数量 */
  failedFiles: number
  status: TaskStatus
  createdAt: Timestamp
  completedAt: Timestamp | null
}

/** `POST /api/batches` 请求体 */
export interface BatchCreateRequest {
  inputPaths: string[]
  outputDirectory: string
  templateId: Id
  parameterValues: Record<string, string>
  startImmediately: boolean
}

/** `POST /api/batches` 响应 */
export interface BatchCreateResultDto {
  batch: BatchDto
  tasks: TaskDto[]
}

/** `POST /api/batches/{id}/cancel` 响应 */
export interface BatchCancelResultDto {
  cancelled: number
}

/** `POST /api/batches/{id}/retry-failed` 响应 */
export interface BatchRetryFailedResultDto {
  retried: number
  queued: number
}

/** `DELETE /api/batches/{id}` 响应 */
export interface BatchDeleteResultDto {
  deletedTasks: number
}

/* ══════════════════════════════════════════════════════════════════════════
   1.7 历史
   ══════════════════════════════════════════════════════════════════════════ */

/** `POST /api/history/cleanup` 请求体 */
export interface HistoryCleanupRequest {
  retentionDays: number
}

/** `POST /api/history/cleanup` 响应 */
export interface HistoryCleanupResultDto {
  deletedTasks: number
  deletedBatches: number
}

/** `POST /api/history/clear` 响应 */
export interface HistoryClearResultDto {
  deletedTasks: number
}

/* ══════════════════════════════════════════════════════════════════════════
   2. WebSocket 协议
   ══════════════════════════════════════════════════════════════════════════ */

/** 客户端可发消息的 action */
export type WsAction = 'subscribe' | 'ping' | 'task.cancel'

/** 客户端可发的订阅主题（缺省 = 全部） */
export type WsTopic = 'tasks' | 'logs' | 'queue'

/** 客户端 → 服务端 */
export interface WsClientSubscribeMessage {
  action: 'subscribe'
  topics: WsTopic[]
}

export interface WsClientPingMessage {
  action: 'ping'
}

/** 可选便捷通道：不经过 REST 直接取消任务 */
export interface WsClientCancelMessage {
  action: 'task.cancel'
  taskId: Id
  graceful: boolean
}

export type WsClientMessage =
  | WsClientSubscribeMessage
  | WsClientPingMessage
  | WsClientCancelMessage

/** 连接建立后服务端立即推送的快照（避免首屏竞态） */
export interface WsSnapshotDto {
  queue: QueueStatusDto
  runningTasks: TaskDto[]
  pendingTasks: TaskDto[]
}

export interface TaskProgressEventDto {
  taskId: Id
  progress: number
  speed: number | null
  eta: number | null
  currentTime: number
  totalDuration: number
}

export interface TaskStatusEventDto {
  taskId: Id
  oldStatus: TaskStatus
  newStatus: TaskStatus
  errorMessage: string | null
  /** 服务端带上最新任务对象（契约 §2 事件表） */
  task?: TaskDto
}

export interface TaskLogEventDto {
  taskId: Id
  /** 服务端已合并（100–200ms 一批，每批最多 50 行） */
  lines: string[]
  dropped: number
  truncated: boolean
}

export interface QueueChangedEventDto {
  queued: number
  active: number
}

export interface BatchStatusEventDto {
  batchId: Id
  status: TaskStatus
  totalFiles: number
  completedFiles: number
  failedFiles: number
}

export interface TemplatesChangedEventDto {
  reason: 'import' | 'sync' | 'crud'
  count: number
}

export interface ServerShutdownEventDto {
  reason: string
}

/** 每个事件信封都是 `{"type","ts","data"}` */
export interface WsEnvelope<TType extends string, TData> {
  type: TType
  ts: Timestamp
  data: TData
}

/** 服务端 → 客户端的事件联合（页面通过 `onWsEvent` 订阅） */
export type WsServerEvent =
  | WsEnvelope<'snapshot', WsSnapshotDto>
  | WsEnvelope<'task.progress', TaskProgressEventDto>
  | WsEnvelope<'task.status', TaskStatusEventDto>
  | WsEnvelope<'task.log', TaskLogEventDto>
  | WsEnvelope<'queue.changed', QueueChangedEventDto>
  | WsEnvelope<'batch.status', BatchStatusEventDto>
  | WsEnvelope<'encoders.detected', HardwareReportDto>
  | WsEnvelope<'settings.changed', SettingsDto>
  | WsEnvelope<'templates.changed', TemplatesChangedEventDto>
  | WsEnvelope<'pong', Record<string, never>>
  | WsEnvelope<'server.shutdown', ServerShutdownEventDto>

/** 事件 type 的联合 */
export type WsEventType = WsServerEvent['type']

/** 取出某个 type 对应的事件类型 */
export type WsEventOf<TType extends WsEventType> = Extract<WsServerEvent, { type: TType }>

/** 心跳间隔：客户端 20s 一次 ping（服务端 90s 未收到任何消息则关闭） */
export const WS_PING_INTERVAL_MS = 20_000

/* ══════════════════════════════════════════════════════════════════════════
   3. 全站状态映射（ui-spec §4.10，唯一真相）
   ══════════════════════════════════════════════════════════════════════════ */

/** 状态中文名（`运行中`；批量文件列表用 `进行中` 是刻意的不一致，见 ui-spec §4.10 注） */
export const TASK_STATUS_TEXT: Record<TaskStatus, string> = {
  Pending: '等待中',
  Running: '运行中',
  Completed: '已完成',
  Failed: '失败',
  Cancelled: '已取消',
  Skipped: '已跳过',
}

/** 状态图标 */
export const TASK_STATUS_ICON: Record<TaskStatus, string> = {
  Pending: '⏸️',
  Running: '⏳',
  Completed: '✅',
  Failed: '❌',
  Cancelled: '⏹️',
  Skipped: '⏭️',
}

/** 状态徽标修饰类（.ffui-badge + 该后缀；Pending 用默认无色徽标） */
export const TASK_STATUS_BADGE_CLASS: Record<TaskStatus, string> = {
  Pending: '',
  Running: 'ffui-badge-info',
  Completed: 'ffui-badge-success',
  Failed: 'ffui-badge-danger',
  Cancelled: 'ffui-badge-warning',
  Skipped: 'ffui-badge-warning',
}

/** 状态徽标色调（直接传 `<Badge tone={TASK_STATUS_TONE[status]}>`；取值与 Badge 的 BadgeTone 一致） */
export const TASK_STATUS_TONE: Record<
  TaskStatus,
  'default' | 'primary' | 'success' | 'danger' | 'warning' | 'info'
> = {
  Pending: 'default',
  Running: 'info',
  Completed: 'success',
  Failed: 'danger',
  Cancelled: 'warning',
  Skipped: 'warning',
}

/** 状态进度条填充类（.ffui-progress-fill 的修饰类） */
export const TASK_STATUS_FILL_CLASS: Record<TaskStatus, string> = {
  Pending: 'pending',
  Running: 'running',
  Completed: 'completed',
  Failed: 'failed',
  Cancelled: 'cancelled',
  Skipped: 'skipped',
}

/** 任务行左侧竖条颜色令牌 */
export const TASK_STATUS_STRIPE_VAR: Record<TaskStatus, string> = {
  Pending: '--border-strong',
  Running: '--info-color',
  Completed: '--success-color',
  Failed: '--danger-color',
  Cancelled: '--warning-color',
  Skipped: '--warning-color',
}

/** 状态中文名；`variant='batch'` 时 Running 显示为「进行中」 */
export function taskStatusText(status: TaskStatus, variant: 'default' | 'batch' = 'default'): string {
  if (variant === 'batch' && status === 'Running') return '进行中'
  return TASK_STATUS_TEXT[status] ?? String(status)
}

/** 是否终态 */
export function isTerminalStatus(status: TaskStatus): boolean {
  return status === 'Completed' || status === 'Failed' || status === 'Cancelled' || status === 'Skipped'
}

/** 编码器图标（ui-spec §3.4 厂商图标映射） */
export const ENCODER_TYPE_ICON: Record<EncoderType, string> = {
  Nvenc: '🟢',
  Qsv: '🔵',
  Amf: '🔴',
  VideoToolbox: '⚪',
  Vaapi: '🟣',
  Vulkan: '🟠',
  MediaFoundation: '🟡',
  Software: '💻',
  Other: '❓',
}

/** 编码器是否硬件编码器（服务端没给 isHardware 时兜底） */
export function isHardwareEncoder(encoder: HardwareEncoderDto): boolean {
  if (typeof encoder.isHardware === 'boolean') return encoder.isHardware
  return encoder.type !== 'Software'
}
