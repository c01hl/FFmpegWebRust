/**
 * 原生文件对话框后端信息（`GET /api/files/dialog/backend`）。
 *
 * 后端描述会出现在 ui-spec §3.3 / §6.3 的多条失败文案里（例如
 * `macOS 原生选择器（osascript）`），所以必须能**同步**拿到：
 * 这里在模块级缓存一次结果，组件里先 await 再拼文案。
 */

import { filesApi } from '../../api/endpoints'
import type { FileDialogBackendDto } from '../../api/types'

/** 查询失败时的兜底描述（用户仍可手动粘贴路径） */
export const UNKNOWN_DIALOG_BACKEND: FileDialogBackendDto = {
  isSupported: false,
  backend: '不可用（无法连接服务）',
}

let cache: FileDialogBackendDto | null = null

/** 取对话框后端信息（同一会话只请求一次） */
export async function resolveDialogBackend(): Promise<FileDialogBackendDto> {
  if (cache) return cache
  try {
    cache = await filesApi.dialogBackend()
  } catch {
    cache = UNKNOWN_DIALOG_BACKEND
  }
  return cache
}

/** 已缓存的后端信息（没有就返回 null，用于纯展示） */
export function cachedDialogBackend(): FileDialogBackendDto | null {
  return cache
}
