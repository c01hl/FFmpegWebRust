/**
 * FileSelector 的用户可见文案（ui-spec §3.3 / §6.2 逐字）。
 *
 * 单独放一个非组件模块：组件文件只导出组件，才能让 Vite 的 Fast Refresh 正常工作。
 */

/** 输入防抖时长（毫秒）：避免每敲一个键都回调父组件 */
export const INPUT_DEBOUNCE_MS = 350

/** 路径校验错误文案（ui-spec §3.3 / §6.2 逐字） */
export const FILE_SELECTOR_ERRORS = {
  directoryMissing: '目录不存在',
  fileMissing: '文件不存在或无法访问',
  unsupportedFormat: (allowed: string[]) => `不支持的文件格式，允许: ${allowed.join(', ')}`,
  readFailed: (message: string) => `路径无法读取：${message}`,
  badFormat: (message: string) => `路径格式无法识别：${message}`,
} as const

/** 文件对话框失败文案（ui-spec §3.3 四档 + 回调/初始目录两档，逐字） */
export const FILE_DIALOG_MESSAGES = {
  unsupported: (backend: string) =>
    `当前环境不支持系统文件对话框（${backend}），请在输入框中手动粘贴路径。`,
  cancelled: (kind: '文件' | '文件夹', backend: string) =>
    `没有选择${kind}：可能已取消，或系统对话框（${backend}）不可用。可直接把路径粘贴到输入框。`,
  failed: (message: string, backend: string) =>
    `打开文件对话框失败：${message}（对话框后端：${backend}）。可直接把路径粘贴到输入框。`,
  notSaved: (message: string) => `路径未能保存：${message}`,
  initialDirectory: (message: string) => `无法识别当前路径所在目录，将使用默认目录：${message}`,
} as const

