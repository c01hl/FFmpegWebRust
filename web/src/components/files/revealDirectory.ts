/**
 * 「打开输出目录」的统一行为（ui-spec §4.8 / §4.9）。
 *
 * - 目录为空或不存在 → Warning `输出目录不存在，可能已被移动或删除。`
 * - 成功 → 由调用方决定是否弹 Success（首页/任务列表用 `已在文件管理器中打开输出目录。`，
 *   转换页与批量页的内部调用**不弹提示**，§12 注 17）
 * - 失败 → Error `{错误前缀}：{原因}`（默认前缀 `无法打开目录`，任务列表页用 `无法打开输出目录`）
 */

import { getErrorMessage } from '../../api/client'
import { filesApi } from '../../api/endpoints'
import { toast } from '../../store/toast'

export interface RevealDirectoryOptions {
  /** 成功时的提示（不传则不提示） */
  successText?: string
  /** 失败文案前缀，默认 `无法打开目录` */
  errorPrefix?: string
}

/** 打开输出目录（目录路径，不是文件路径） */
export async function revealDirectory(
  directory: string,
  options: RevealDirectoryOptions = {},
): Promise<boolean> {
  const { successText, errorPrefix = '无法打开目录' } = options

  if (!directory || directory.trim() === '') {
    toast.warning('输出目录不存在，可能已被移动或删除。')
    return false
  }

  try {
    // 先确认目录还在：服务端 reveal 失败的原因不一定可读，这里给出确定的中文提示
    const check = await filesApi.check({ path: directory, kind: 'folder' })
    if (!check.exists) {
      toast.warning('输出目录不存在，可能已被移动或删除。')
      return false
    }

    const result = await filesApi.reveal(directory)
    if (result?.ok === false) {
      toast.error(`${errorPrefix}：${result.reason ?? '系统拒绝了打开目录的操作'}`)
      return false
    }

    if (successText) toast.success(successText)
    return true
  } catch (error) {
    toast.error(`${errorPrefix}：${getErrorMessage(error)}`)
    return false
  }
}
