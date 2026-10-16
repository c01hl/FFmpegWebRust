/**
 * 模板卡片（网格 / 列表两用，对应 ui-spec §3.7 / §8.2）。
 *
 * 与参考实现一致的部分：
 * - 标题行 → 名称 + `预设` / `自定义` 徽标 + 硬件徽标（`🚀 硬件可用` / `⚠ 硬件不可用`）；
 * - 描述 → 参数摘要 `⚙ 可调参数（N）` → `📁 .{扩展名}` / `📂 {分类}`；
 * - 列表模式额外显示命令参数区块（标题行 + 复制按钮 + `<code>`）；
 * - 操作区：🔍 查看详情、📋 复制命令参数（网格模式）、📑 复制为自定义模板（系统）或
 *   ✏️ 编辑 + 🗑️ 删除（用户）、主按钮（`直接使用` / `选择`，不可用时 disabled）；
 * - 复制状态用 `visually-hidden` 的 `role="status" aria-live="polite"` 播报；
 * - 不可用卡片加 `blocked` 视觉（置灰 + 警告色边框），并直接显示**服务端给的**不可用原因。
 *
 * 本任务追加的两项（见需求）：标签 chip 行与 `inputFormatsDisplay` 一行；点击卡片正文也打开详情。
 * 可用性**完全来自服务端**（`available` / `unavailableReason`），前端不做任何推断。
 */

import { useEffect, useRef, useState } from 'react'
import type { MouseEvent } from 'react'
import type { TemplateListItem } from '../../api/types'
import { COPY_FAILURE_MESSAGES, copyText } from '../../lib/clipboard'
import { cx } from '../../lib/cx'
import { availabilityBadge, parameterLabelsText } from '../../lib/templateCatalog'
import { toast } from '../../store/toast'
import { Badge } from '../ui/Badge'
import { IconButton } from '../ui/IconButton'

export interface TemplateCardViewProps {
  template: TemplateListItem
  /** `grid`（默认）/ `list` */
  viewMode?: 'grid' | 'list'
  isSelected?: boolean
  /** 是否显示 `预设` / `自定义` 徽标（默认 true） */
  showSystemBadges?: boolean
  onSelect?: (template: TemplateListItem) => void
  onView?: (template: TemplateListItem) => void
  onEdit?: (template: TemplateListItem) => void
  onDelete?: (template: TemplateListItem) => void
  onCopy?: (template: TemplateListItem) => void
}

export function TemplateCardView({
  template,
  viewMode = 'grid',
  isSelected = false,
  showSystemBadges = true,
  onSelect,
  onView,
  onEdit,
  onDelete,
  onCopy,
}: TemplateCardViewProps) {
  const [copied, setCopied] = useState(false)
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    return () => {
      if (timerRef.current !== null) clearTimeout(timerRef.current)
    }
  }, [])

  const isSystem = template.type === 'System'
  const isBlocked = !template.available
  const badge = availabilityBadge(template)
  const unavailableTitle = template.unavailableReason || '本机缺少该模板所需的硬件编码器'
  const parameterSummary = parameterLabelsText(template.parameters)

  const copyCommand = async () => {
    const text = template.commandArgs ?? ''
    if (text.trim() === '') {
      toast.warning('该模板没有可复制的命令参数')
      return
    }
    let ok = false
    try {
      ok = await copyText(text)
    } catch (error) {
      toast.error(`复制失败：${error instanceof Error ? error.message : String(error)}`)
      return
    }
    if (!ok) {
      toast.error(COPY_FAILURE_MESSAGES.commandArgs)
      return
    }
    toast.success('命令参数已复制到剪贴板')
    setCopied(true)
    if (timerRef.current !== null) clearTimeout(timerRef.current)
    timerRef.current = setTimeout(() => setCopied(false), 2000)
  }

  const guarded = (action: ((value: TemplateListItem) => void) | undefined, name: string) => {
    return () => {
      if (!action) {
        toast.warning(`${name}功能不可用：父组件未提供对应回调`)
        return
      }
      try {
        action(template)
      } catch (error) {
        toast.error(`${name}失败：${error instanceof Error ? error.message : String(error)}`)
      }
    }
  }

  const select = () => {
    if (isBlocked) {
      toast.warning(unavailableTitle)
      return
    }
    guarded(onSelect, isSystem ? '使用模板' : '选择模板')()
  }

  /** 点击卡片正文 → 打开详情（按钮/链接上的点击不算，避免与操作区冲突） */
  const onMainClick = (event: MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement
    if (target.closest('button, a, input, select, textarea')) return
    guarded(onView, '查看详情')()
  }

  return (
    <div
      className={cx(
        'flex flex-col rounded-md border bg-surface p-4 transition-shadow hover:shadow-md',
        isSelected ? 'border-primary shadow-[var(--focus-ring)]' : 'border-line',
        isBlocked ? 'border-warning opacity-[0.65]' : '',
      )}
      data-template-id={template.id}
    >
      <div className="flex cursor-pointer flex-col gap-[0.55rem]" onClick={onMainClick}>
        <div className="flex flex-wrap items-start justify-between gap-[0.4rem]">
          <button
            type="button"
            className="cursor-pointer border-0 bg-transparent p-0 text-left text-[1rem] font-semibold text-primary-text hover:text-primary"
            aria-label={`查看模板详情：${template.name}`}
            onClick={guarded(onView, '查看详情')}
          >
            {template.name}
          </button>
          <div className="flex flex-wrap gap-[0.35rem]">
            {showSystemBadges ? (
              <Badge tone={isSystem ? 'default' : 'success'}>{isSystem ? '预设' : '自定义'}</Badge>
            ) : null}
            <Badge tone={badge.tone} title={badge.title}>
              {badge.text}
            </Badge>
          </div>
        </div>

        {template.description ? (
          <p className="m-0 text-[0.88rem] text-secondary-text">{template.description}</p>
        ) : null}

        {parameterSummary ? (
          <div className="flex flex-wrap items-baseline gap-[0.4rem] text-[0.82rem] text-muted-text">
            <span>⚙ 可调参数（{(template.parameters ?? []).length}）</span>
            <span>{parameterSummary}</span>
          </div>
        ) : null}

        <div className="flex flex-wrap gap-[0.75rem] text-[0.82rem] text-muted-text">
          <span className="mono">📁 .{template.outputExtension}</span>
          <span>📂 {template.category || '未分类'}</span>
          <span>📥 {template.inputFormatsDisplay || '任意格式'}</span>
        </div>

        {(template.tags ?? []).length > 0 ? (
          <div className="flex flex-wrap gap-[0.3rem]">
            {(template.tags ?? []).map((tag) => (
              <Badge key={tag} tone="info">
                {tag}
              </Badge>
            ))}
          </div>
        ) : null}

        {(template.missingFilters ?? []).length > 0 ? (
          <div className="rounded-sm bg-danger-bg px-[0.5rem] py-[0.25rem] text-[0.78rem] text-danger-text">
            缺少滤镜：{(template.missingFilters ?? []).join('、')}
          </div>
        ) : null}

        {isBlocked ? (
          <div className="rounded-sm bg-danger-bg px-[0.5rem] py-[0.25rem] text-[0.78rem] text-danger-text" role="alert">
            {unavailableTitle}
          </div>
        ) : null}

        {viewMode === 'list' ? (
          <div className="flex flex-col gap-[0.25rem]">
            <div className="flex items-center gap-[0.4rem]">
              <span className="text-[0.8rem] text-muted-text">FFmpeg 命令参数:</span>
              <IconButton
                ariaLabel={copied ? '命令参数已复制' : '复制命令参数'}
                title="复制命令参数"
                onClick={() => void copyCommand()}
              >
                {copied ? '✓' : '📋'}
              </IconButton>
            </div>
            <code className="mono block overflow-x-auto text-[0.8rem]">{template.commandArgs}</code>
          </div>
        ) : null}
      </div>

      <div className="mt-auto flex flex-wrap items-center justify-end gap-[0.4rem] border-t border-line pt-[0.6rem]">
        <IconButton ariaLabel="查看模板详情" title="查看详情" onClick={guarded(onView, '查看详情')}>
          🔍
        </IconButton>

        {viewMode === 'grid' ? (
          <IconButton
            ariaLabel={copied ? '命令参数已复制' : '复制命令参数'}
            title="复制命令参数"
            onClick={() => void copyCommand()}
          >
            {copied ? '✓' : '📋'}
          </IconButton>
        ) : null}

        {isSystem ? (
          <IconButton ariaLabel="复制为自定义模板" title="复制为自定义模板" onClick={guarded(onCopy, '复制模板')}>
            📑
          </IconButton>
        ) : (
          <>
            <IconButton ariaLabel="编辑模板" title="编辑" onClick={guarded(onEdit, '编辑模板')}>
              ✏️
            </IconButton>
            <IconButton ariaLabel="删除模板" title="删除" onClick={guarded(onDelete, '删除模板')}>
              🗑️
            </IconButton>
          </>
        )}

        <button
          type="button"
          className="ffui-btn ffui-btn-primary ffui-btn-sm ml-[0.35rem]"
          disabled={isBlocked}
          title={isBlocked ? unavailableTitle : undefined}
          onClick={select}
        >
          {isSystem ? '直接使用' : '选择'}
        </button>
      </div>

      <span className="visually-hidden" role="status" aria-live="polite">
        {copied ? '命令参数已复制' : ''}
      </span>
    </div>
  )
}
