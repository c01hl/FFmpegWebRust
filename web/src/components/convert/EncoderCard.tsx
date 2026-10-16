/**
 * 编码器信息卡（对应 ui-spec §3.4 `EncoderCard.razor`）。
 *
 * 展示顺序（逐字照抄规格）：
 * 1. 头部：厂商图标 + 显示名 + 可用性徽标（`✓可用` / `✗不可用`，带 `状态：可用` 的 aria-label）
 * 2. 详情行：等宽编码器名（模板 RequiredEncoder 要填这个）+ 右侧族显示名
 * 3. 标签行：`{Codec}` 徽标 + `硬件编码` / `软件编码` 徽标
 * 4. 其它 codecs（去掉已在徽标里出现过的值）
 * 5. 不可用原因（缺失构建 → 警告条 + 原因 + 平台提示；否则单条警告条）
 * 6. 可用但不支持质量参数 → 黄色徽标
 */

import type { HardwareEncoderDto } from '../../api/types'
import { isHardwareEncoder } from '../../api/types'
import { cx } from '../../lib/cx'
import { Badge } from '../ui/Badge'
import { encoderTypeIcon, familyDisplayName, platformBuildHint } from './hardwareLabels'

export interface EncoderCardProps {
  /** 编码器信息；null = 预渲染/参数未就绪，渲染降级卡片 */
  encoder: HardwareEncoderDto | null
  /** 平台提示（`macOS` / `Windows` / `Linux`）；不传时用通用文案 */
  platform?: string | null
}

/** 附加 codecs：去掉空值、去掉已在 Codec 徽标里出现过的值（忽略大小写） */
function extraCodecs(encoder: HardwareEncoderDto): string[] {
  const list = encoder.supportedCodecs ?? []
  const seen = new Set<string>()
  const result: string[] = []
  for (const raw of list) {
    const value = (raw ?? '').trim()
    if (value === '') continue
    const key = value.toLowerCase()
    if (key === (encoder.codec ?? '').trim().toLowerCase()) continue
    if (seen.has(key)) continue
    seen.add(key)
    result.push(value)
  }
  return result
}

export function EncoderCard({ encoder, platform }: EncoderCardProps) {
  if (!encoder) {
    return (
      <div
        className="rounded-md border border-line bg-surface-3 p-4 m-2 min-w-[250px] opacity-75"
        aria-busy="true"
      >
        <span className="font-semibold" style={{ fontSize: '0.95rem' }}>
          编码器信息不可用
        </span>
      </div>
    )
  }

  const available = Boolean(encoder.isAvailable)
  const hardware = isHardwareEncoder(encoder)
  const codecs = extraCodecs(encoder)

  return (
    <div
      className={cx(
        'rounded-md border border-line p-4 m-2 min-w-[250px] transition-shadow hover:shadow-md',
        available ? 'bg-surface' : 'bg-surface-3 opacity-75',
      )}
    >
      <div className="flex flex-wrap items-center gap-2 mb-2">
        <span className="leading-none" aria-hidden="true">
          {encoderTypeIcon(encoder.type)}
        </span>
        <span className="font-semibold" style={{ fontSize: '0.95rem' }}>
          {encoder.displayName || encoder.name}
        </span>
        <Badge
          tone={available ? 'success' : 'danger'}
          icon={available ? '✓' : '✗'}
          ariaLabel={available ? '状态：可用' : '状态：不可用'}
        >
          {available ? '可用' : '不可用'}
        </Badge>
      </div>

      <div className="flex flex-wrap items-center justify-between gap-2 text-secondary-text" style={{ fontSize: '0.85rem' }}>
        <code
          className="mono break-all bg-code px-1.5 py-0.5 rounded-sm"
          style={{ fontSize: '0.8rem' }}
          title="模板 RequiredEncoder 需要填写这个名称"
        >
          {encoder.name}
        </code>
        <span className="text-muted-text" title={encoder.family ?? undefined}>
          {familyDisplayName(encoder.family)}
        </span>
      </div>

      <div className="flex flex-wrap gap-1.5 mt-2">
        {encoder.codec?.trim() ? (
          <Badge tone="info" title="编码格式">
            {encoder.codec}
          </Badge>
        ) : null}
        <Badge title={hardware ? '硬件编码器' : '软件编码器'}>{hardware ? '硬件编码' : '软件编码'}</Badge>
      </div>

      {!available ? (
        encoder.missingFromBuild ? (
          <>
            <div className="ffui-alert ffui-alert-warning mt-2.5" role="status">
              <span aria-hidden="true">⚠️</span>
              <div>当前 FFmpeg 构建未包含该编码器</div>
            </div>
            {encoder.unavailableReason?.trim() ? (
              <div className="mt-1.5 text-muted-text" style={{ fontSize: '0.78rem' }}>
                {encoder.unavailableReason}
              </div>
            ) : null}
            {hardware ? (
              <div className="ffui-alert ffui-alert-info mt-2" role="status">
                <span aria-hidden="true">ℹ️</span>
                <div>{platformBuildHint(platform)}</div>
              </div>
            ) : null}
          </>
        ) : encoder.unavailableReason?.trim() ? (
          <div className="ffui-alert ffui-alert-warning mt-2.5" role="status">
            <span aria-hidden="true">⚠️</span>
            <div>{encoder.unavailableReason}</div>
          </div>
        ) : null
      ) : !encoder.supportsQualityOptions ? (
        <div className="mt-2.5">
          <Badge tone="warning">该编码器在当前 FFmpeg 版本下只能使用最简参数</Badge>
        </div>
      ) : null}

      {codecs.length > 0 ? (
        <div className="flex flex-wrap gap-1 mt-2">
          {codecs.map((codec) => (
            <span
              key={codec}
              className="rounded-sm border border-line bg-surface-3 px-1.5 py-0.5 mono text-secondary-text"
              style={{ fontSize: '0.75rem' }}
            >
              {codec}
            </span>
          ))}
        </div>
      ) : null}
    </div>
  )
}
