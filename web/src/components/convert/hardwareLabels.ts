/**
 * 硬件/软件编码器的展示名与图标映射（对应参考实现 `Services/EncoderCatalog.cs` 的展示部分）。
 *
 * ⚠️ 这里**只做展示**：编码器是否可用、为什么不可用一律由后端给出
 * （`GET /api/system/encoders` 的 `isAvailable` / `unavailableReason` / `missingFromBuild`），
 * 前端不自己判断某台机器能不能跑某个编码器。
 */

import type { EncoderType, HardwareEncoderDto } from '../../api/types'
import { ENCODER_TYPE_ICON, isHardwareEncoder } from '../../api/types'

/** 编码器族显示名（EncoderCatalog.FamilyDisplayName，逐字） */
export function familyDisplayName(family: string | null | undefined): string {
  if (!family || family.trim() === '') return '未知族'
  const key = family.trim().toLowerCase()
  switch (key) {
    case 'nvenc':
      return 'NVIDIA NVENC'
    case 'qsv':
      return 'Intel Quick Sync'
    case 'amf':
      return 'AMD AMF'
    case 'videotoolbox':
      return 'Apple VideoToolbox'
    case 'vaapi':
      return 'Linux VA-API'
    case 'vulkan':
      return 'Vulkan Video'
    case 'mf':
      return 'Windows Media Foundation'
    case 'software':
      return '软件编码'
    default:
      return family
  }
}

/** 厂商图标（ui-spec §3.4 映射表） */
export function encoderTypeIcon(type: EncoderType): string {
  return ENCODER_TYPE_ICON[type] ?? '❓'
}

export { isHardwareEncoder }

/** 平台名（`macOS` / `Windows` / `Linux`）→ `MissingFromBuild` 时的可操作提示 */
export function platformBuildHint(platform: string | null | undefined): string {
  switch ((platform ?? '').toLowerCase()) {
    case 'windows':
      return 'Windows：请安装显卡驱动，并使用带 nvenc / qsv / amf 的 FFmpeg 构建。'
    case 'macos':
      return 'macOS：仅 VideoToolbox 可用，请使用带 videotoolbox 的 FFmpeg 构建。'
    case 'linux':
      return 'Linux：需要 /dev/dri 设备与 VA-API 驱动（如 intel-media-driver / mesa-va-drivers）。'
    default:
      return '请改用带该编码器的 FFmpeg 构建。'
  }
}

/** 从硬件报告里按编码器名取展示名（找不到就回退到编码器名本身） */
export function describeEncoder(
  encoderName: string,
  encoders: readonly HardwareEncoderDto[] | null | undefined,
): string {
  const matched = encoders?.find((item) => item.name.toLowerCase() === encoderName.toLowerCase())
  return matched?.displayName?.trim() || encoderName
}

/** 从硬件报告里判断某个编码器名是否是硬件编码器（报告缺失时按名字无法判断，返回 false） */
export function isHardwareEncoderName(
  encoderName: string,
  encoders: readonly HardwareEncoderDto[] | null | undefined,
): boolean {
  const matched = encoders?.find((item) => item.name.toLowerCase() === encoderName.toLowerCase())
  return matched ? isHardwareEncoder(matched) : false
}
