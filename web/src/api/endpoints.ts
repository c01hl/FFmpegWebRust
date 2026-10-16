/**
 * 按资源分组的强类型调用函数（全部对应 docs/api-contract.md §1 的端点清单）。
 *
 * 约定：
 * - 每个函数只负责「拼请求 + 解析响应」，**不做业务推断**（预览、校验、命名规则都由后端算）；
 * - 慢端点显式传 `timeoutMs: LONG_TIMEOUT_MS`（模板试运行、硬件检测、目录扫描）；
 * - 失败一律抛 `ApiError`（见 api/client.ts），页面用 `getErrorMessage` / `reportApiError` 处理。
 */

import { LONG_TIMEOUT_MS, apiRequest, http } from './client'
import type {
  AppStatusDto,
  BatchCancelResultDto,
  BatchCreateRequest,
  BatchCreateResultDto,
  BatchDeleteResultDto,
  BatchDto,
  BatchRetryFailedResultDto,
  FFmpegInfoDto,
  FFmpegValidateResultDto,
  FileCheckRequest,
  FileCheckResultDto,
  FileDialogBackendDto,
  FileDialogRequest,
  FileDialogResultDto,
  FileExtensionsDto,
  HardwareReportDto,
  HealthDto,
  HistoryCleanupRequest,
  HistoryCleanupResultDto,
  HistoryClearResultDto,
  MediaInfoResultDto,
  MediaScanRequest,
  MediaScanResultDto,
  Paged,
  QueueStatusDto,
  RecommendedEncoderDto,
  RevealResultDto,
  SettingsDto,
  SettingsPortDto,
  SettingsUpdateResultDto,
  SystemPathsDto,
  TaskCancelRequest,
  TaskCreateRequest,
  TaskCreateResultDto,
  TaskDeleteByStatusResultDto,
  TaskDeleteRequest,
  TaskDeleteResultDto,
  TaskDto,
  TaskListQuery,
  TaskLogDto,
  TaskPreviewDto,
  TaskPreviewRequest,
  TaskRetryResultDto,
  TaskStatus,
  TemplateCategoryDto,
  TemplateExportDto,
  TemplateImportRequest,
  TemplateImportResultDto,
  TemplateListQuery,
  TemplateListItem,
  TemplateSystemResetResultDto,
  TemplateSystemSyncResultDto,
  TemplateTestRequest,
  TemplateTestResultDto,
  TemplateValidateResultDto,
  CommandTemplate,
} from './types'

/* ══════════════════════════════════════════════════════════════════════════
   1.1 系统与环境
   ══════════════════════════════════════════════════════════════════════════ */

export const systemApi = {
  /** `GET /api/health` */
  health: () => http.get<HealthDto>('/health'),

  /** `GET /api/system/status?force=`（首页/转换页/设置页的环境状态） */
  status: (force = false) => http.get<AppStatusDto>('/system/status', { query: { force } }),

  /** `GET /api/system/ffmpeg?force=`（可能需要数秒，给长超时） */
  ffmpeg: (force = false) =>
    http.get<FFmpegInfoDto>('/system/ffmpeg', { query: { force }, timeoutMs: LONG_TIMEOUT_MS }),

  /** `POST /api/system/ffmpeg/validate`（设置页「测试路径」） */
  validateFFmpeg: (path: string) =>
    http.post<FFmpegValidateResultDto>('/system/ffmpeg/validate', { path }, { timeoutMs: LONG_TIMEOUT_MS }),

  /** `GET /api/system/encoders?force=`（同步返回，首次可能耗时数十秒） */
  encoders: (force = false) =>
    http.get<HardwareReportDto>('/system/encoders', { query: { force }, timeoutMs: LONG_TIMEOUT_MS }),

  /** `GET /api/system/encoders/recommend?codec=&required=` */
  recommendEncoder: (codec: string, required?: string | null) =>
    http.get<RecommendedEncoderDto>('/system/encoders/recommend', {
      query: { codec, required: required ?? undefined },
      timeoutMs: LONG_TIMEOUT_MS,
    }),

  /** `GET /api/system/paths` */
  paths: () => http.get<SystemPathsDto>('/system/paths'),

  /** `GET /api/system/queue` */
  queue: () => http.get<QueueStatusDto>('/system/queue'),
}

/* ══════════════════════════════════════════════════════════════════════════
   1.2 设置
   ══════════════════════════════════════════════════════════════════════════ */

export const settingsApi = {
  /** `GET /api/settings` */
  get: () => http.get<SettingsDto>('/settings'),

  /** `PUT /api/settings`（body = 完整 SettingsDto，整表替换 + normalize） */
  update: (settings: SettingsDto) => http.put<SettingsUpdateResultDto>('/settings', settings),

  /** `POST /api/settings/reset` */
  reset: () => http.post<SettingsDto>('/settings/reset'),

  /** `GET /api/settings/port` */
  port: () => http.get<SettingsPortDto>('/settings/port'),
}

/* ══════════════════════════════════════════════════════════════════════════
   1.3 模板
   ══════════════════════════════════════════════════════════════════════════ */

export const templatesApi = {
  /** `GET /api/templates?includeSystem=&category=&q=`（含服务端计算的可用性字段） */
  list: (query: TemplateListQuery = {}) =>
    http.get<TemplateListItem[]>('/templates', {
      query: {
        includeSystem: query.includeSystem ?? true,
        category: query.category,
        q: query.q,
      },
    }),

  /** `GET /api/templates/categories` */
  categories: () => http.get<TemplateCategoryDto[]>('/templates/categories'),

  /** `GET /api/templates/{id}` */
  get: (id: string) => http.get<CommandTemplate>(`/templates/${encodeURIComponent(id)}`),

  /** `POST /api/templates`（重名 → 409） */
  create: (template: Partial<CommandTemplate>) => http.post<CommandTemplate>('/templates', template),

  /** `PUT /api/templates/{id}` */
  update: (id: string, template: Partial<CommandTemplate>) =>
    http.put<CommandTemplate>(`/templates/${encodeURIComponent(id)}`, template),

  /** `DELETE /api/templates/{id}`（系统模板 → 403） */
  remove: (id: string) => http.delete<void>(`/templates/${encodeURIComponent(id)}`),

  /** `POST /api/templates/{id}/copy` */
  copy: (id: string, newName: string) =>
    http.post<CommandTemplate>(`/templates/${encodeURIComponent(id)}/copy`, { newName }),

  /** `POST /api/templates/validate`（body = 模板对象） */
  validate: (template: Partial<CommandTemplate>) =>
    http.post<TemplateValidateResultDto>('/templates/validate', template),

  /** `POST /api/templates/{id}/test`（同步试运行，最长约 30s → 给 120s 超时） */
  test: (id: string, parameterValues: Record<string, string>) =>
    http.post<TemplateTestResultDto>(
      `/templates/${encodeURIComponent(id)}/test`,
      { parameterValues } satisfies TemplateTestRequest,
      { timeoutMs: LONG_TIMEOUT_MS },
    ),

  /** `GET /api/templates/export?ids=a,b` */
  exportJson: (ids: string[]) =>
    http.get<TemplateExportDto>('/templates/export', {
      query: { ids: ids.length > 0 ? ids.join(',') : undefined },
      timeoutMs: LONG_TIMEOUT_MS,
    }),

  /** `POST /api/templates/import` */
  importJson: (body: TemplateImportRequest) =>
    http.post<TemplateImportResultDto>('/templates/import', body, { timeoutMs: LONG_TIMEOUT_MS }),

  /** `POST /api/templates/system/reset` */
  resetSystem: () => http.post<TemplateSystemResetResultDto>('/templates/system/reset', undefined, {
    timeoutMs: LONG_TIMEOUT_MS,
  }),

  /** `POST /api/templates/system/sync` */
  syncSystem: () => http.post<TemplateSystemSyncResultDto>('/templates/system/sync', undefined, {
    timeoutMs: LONG_TIMEOUT_MS,
  }),
}

/* ══════════════════════════════════════════════════════════════════════════
   1.4 媒体与文件
   ══════════════════════════════════════════════════════════════════════════ */

export const mediaApi = {
  /** `POST /api/media/info`（ffprobe 探测，可能慢） */
  info: (path: string) =>
    apiRequest<MediaInfoResultDto>('/media/info', {
      method: 'POST',
      body: { path },
      timeoutMs: LONG_TIMEOUT_MS,
    }),

  /** `POST /api/media/scan`（大目录扫描可能慢） */
  scan: (body: MediaScanRequest) =>
    apiRequest<MediaScanResultDto>('/media/scan', {
      method: 'POST',
      body,
      timeoutMs: LONG_TIMEOUT_MS,
    }),
}

export const filesApi = {
  /** `GET /api/files/extensions` */
  extensions: () => http.get<FileExtensionsDto>('/files/extensions'),

  /** `POST /api/files/dialog`（服务端拉起原生对话框，最长等 5 分钟 → 给 120s） */
  dialog: (body: FileDialogRequest) =>
    apiRequest<FileDialogResultDto>('/files/dialog', {
      method: 'POST',
      body,
      timeoutMs: LONG_TIMEOUT_MS,
    }),

  /** `GET /api/files/dialog/backend` */
  dialogBackend: () => http.get<FileDialogBackendDto>('/files/dialog/backend'),

  /** `POST /api/files/check` */
  check: (body: FileCheckRequest) => http.post<FileCheckResultDto>('/files/check', body),

  /** `POST /api/files/reveal`（在系统文件管理器中打开） */
  reveal: (path: string) => http.post<RevealResultDto>('/files/reveal', { path }),
}

/* ══════════════════════════════════════════════════════════════════════════
   1.5 任务
   ══════════════════════════════════════════════════════════════════════════ */

export const tasksApi = {
  /** `POST /api/tasks/preview`（**关键端点**：命令预览 + 输出路径冲突解析） */
  preview: (body: TaskPreviewRequest) =>
    apiRequest<TaskPreviewDto>('/tasks/preview', {
      method: 'POST',
      body,
      timeoutMs: LONG_TIMEOUT_MS,
    }),

  /** `POST /api/tasks`（201） */
  create: (body: TaskCreateRequest) => http.post<TaskCreateResultDto>('/tasks', body),

  /** `POST /api/tasks/{id}/start`（已终态 → 409） */
  start: (id: string) => http.post<TaskDto>(`/tasks/${encodeURIComponent(id)}/start`),

  /** `POST /api/tasks/{id}/cancel` */
  cancel: (id: string, graceful = false) =>
    http.post<TaskDto>(`/tasks/${encodeURIComponent(id)}/cancel`, {
      graceful,
    } satisfies TaskCancelRequest),

  /** `POST /api/tasks/{id}/retry`（新建 + 入队） */
  retry: (id: string) => http.post<TaskRetryResultDto>(`/tasks/${encodeURIComponent(id)}/retry`),

  /** `GET /api/tasks?status=&batchId=&limit=&offset=`（**不含** logOutput） */
  list: (query: TaskListQuery = {}) =>
    http.get<Paged<TaskDto>>('/tasks', {
      query: {
        status: query.status,
        batchId: query.batchId,
        limit: query.limit,
        offset: query.offset,
      },
    }),

  /** `GET /api/tasks/{id}`（含 logOutput） */
  get: (id: string) => http.get<TaskDto>(`/tasks/${encodeURIComponent(id)}`),

  /** `GET /api/tasks/{id}/log?tailLines=`（重连后补齐错过的日志） */
  log: (id: string, tailLines = 200) =>
    http.get<TaskLogDto>(`/tasks/${encodeURIComponent(id)}/log`, { query: { tailLines } }),

  /** `GET /api/tasks/running` */
  running: () => http.get<TaskDto[]>('/tasks/running'),

  /** `GET /api/tasks/pending` */
  pending: () => http.get<TaskDto[]>('/tasks/pending'),

  /** `POST /api/tasks/delete` */
  removeMany: (ids: string[]) => http.post<TaskDeleteResultDto>('/tasks/delete', { ids } satisfies TaskDeleteRequest),

  /** `DELETE /api/tasks?status=` */
  deleteByStatus: (status: TaskStatus) =>
    http.delete<TaskDeleteByStatusResultDto>('/tasks', { query: { status } }),
}

/* ══════════════════════════════════════════════════════════════════════════
   1.6 批次
   ══════════════════════════════════════════════════════════════════════════ */

export const batchesApi = {
  /** `POST /api/batches` */
  create: (body: BatchCreateRequest) =>
    http.post<BatchCreateResultDto>('/batches', body, { timeoutMs: LONG_TIMEOUT_MS }),

  /** `GET /api/batches?limit=&offset=` */
  list: (query: { limit?: number; offset?: number } = {}) =>
    http.get<Paged<BatchDto>>('/batches', { query: { limit: query.limit, offset: query.offset } }),

  /** `GET /api/batches/{id}` */
  get: (id: string) => http.get<BatchDto>(`/batches/${encodeURIComponent(id)}`),

  /** `GET /api/batches/{id}/tasks`（按 createdAt 正序） */
  tasks: (id: string) => http.get<TaskDto[]>(`/batches/${encodeURIComponent(id)}/tasks`),

  /** `POST /api/batches/{id}/cancel` */
  cancel: (id: string) => http.post<BatchCancelResultDto>(`/batches/${encodeURIComponent(id)}/cancel`),

  /** `POST /api/batches/{id}/retry-failed` */
  retryFailed: (id: string) =>
    http.post<BatchRetryFailedResultDto>(`/batches/${encodeURIComponent(id)}/retry-failed`),

  /** `DELETE /api/batches/{id}` */
  remove: (id: string) => http.delete<BatchDeleteResultDto>(`/batches/${encodeURIComponent(id)}`),
}

/* ══════════════════════════════════════════════════════════════════════════
   1.7 历史
   ══════════════════════════════════════════════════════════════════════════ */

export const historyApi = {
  /** `POST /api/history/cleanup` */
  cleanup: (retentionDays: number) =>
    http.post<HistoryCleanupResultDto>('/history/cleanup', {
      retentionDays,
    } satisfies HistoryCleanupRequest),

  /** `POST /api/history/clear` */
  clear: () => http.post<HistoryClearResultDto>('/history/clear', undefined, {
    timeoutMs: LONG_TIMEOUT_MS,
  }),
}

/** 汇总导出（页面按需 `import { api } from '../api/endpoints'`） */
export const api = {
  system: systemApi,
  settings: settingsApi,
  templates: templatesApi,
  media: mediaApi,
  files: filesApi,
  tasks: tasksApi,
  batches: batchesApi,
  history: historyApi,
}
