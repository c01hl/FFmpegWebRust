/**
 * 模板编辑器（新建 / 编辑，xl 大弹窗，对应 ui-spec §3.9 / §8.4）。
 *
 * 七个区块顺序固定：基本信息 → FFmpeg 命令参数 → 命令预览 → 试运行 → 校验结果（实时）
 * → 参数（N）→ 硬件加速，底部是 sticky 按钮栏。
 *
 * 设计要点（与参考实现一致）：
 * - 一律在**深拷贝的草稿**上编辑，绝不修改传入的 `template`；
 * - 校验结果来自服务端 `POST /api/templates/validate`（250–400ms 防抖），界面里不重复实现一套校验；
 * - 「试运行」调用 `POST /api/templates/{id}/test`；**该端点只接受已保存的模板**，所以草稿有修改
 *   （或本身就是新建）时会先保存再试运行（保存成功用 Toast `模板「{名称}」已保存` 告知），
 *   这样试运行的结果始终对应用户眼前的草稿；
 * - 关闭时若有未保存修改，弹 ConfirmDialog（`放弃未保存的修改？`），Esc / 遮罩 / ✕ 都走同一拦截；
 * - 存在 Error 时无法保存（按钮禁用 + 底部徽标 `{N} 个错误，无法保存`）。
 *
 * 差异说明：「命令预览」用 `lib/templateCatalog.buildCommandPreview` 做本地近似展开，
 * 原因是契约里没有「用未保存草稿构建命令行」的端点（见该函数注释）。
 */

import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import type {
  CommandTemplate,
  HardwareReportDto,
  TemplateIssue,
  TemplateParameter,
  TemplateParameterKind,
  TemplateParameterOption,
  TemplateTestResultDto,
} from '../../api/types'
import { getErrorMessage } from '../../api/client'
import { systemApi, templatesApi } from '../../api/endpoints'
import { COPY_FAILURE_MESSAGES, copyText } from '../../lib/clipboard'
import { cx } from '../../lib/cx'
import { formatDateTimeMinute, formatTestDuration } from '../../lib/format'
import {
  BUILT_IN_PLACEHOLDER_COUNT,
  CODEC_CHOICES,
  ENCODER_FAMILY_NAMES,
  HARDWARE_MODE_OPTIONS,
  ISSUE_SEVERITY_STYLE,
  PARAMETER_KIND_OPTIONS,
  PARAMETER_PRESETS,
  PLACEHOLDER_GROUPS,
  braced,
  buildCommandPreview,
  cloneTemplate,
  countIssues,
  createEmptyTemplate,
  findDuplicateName,
  isValidParameterName,
  issueFieldLabel,
  listToText,
  normalizeFormatList,
  normalizeParameterName,
  normalizeTagList,
  normalizeTemplateDraft,
  parameterDefaultValues,
  sortIssues,
  splitList,
  undeclaredPlaceholders,
} from '../../lib/templateCatalog'
import { toast } from '../../store/toast'
import { Alert } from '../ui/Alert'
import { Badge } from '../ui/Badge'
import { Button } from '../ui/Button'
import { ConfirmDialog } from '../ui/ConfirmDialog'
import { IconButton } from '../ui/IconButton'
import { Modal } from '../ui/Modal'

export interface TemplateEditorProps {
  /** null = 新建（类型由 `isSystem` 决定） */
  template: CommandTemplate | null
  open: boolean
  onOpenChange: (open: boolean) => void
  /** true → 系统预设（标题、徽标、保存后的提示都不同） */
  isSystem: boolean
  /** 新建时的初始排序权重（预设模板管理页用「最大值 + 10」，§4.6） */
  sortOrderHint?: number
  /** 保存成功回调；`closeEditor=false` 表示「试运行前的自动保存」，页面只刷新列表、不关弹窗 */
  onSaved: (template: CommandTemplate, meta: { closeEditor: boolean }) => void
}

/** 弹窗标题（§3.9 表格逐字） */
function editorTitle(isNew: boolean, isSystem: boolean): string {
  if (isNew) return isSystem ? '新建系统预设' : '新建模板'
  return isSystem ? '编辑系统预设' : '编辑模板'
}

export function TemplateEditor(props: TemplateEditorProps) {
  // 只在打开时挂载：草稿的初始化交给 useState 初始化器，避免在 effect 里同步 setState
  if (!props.open) return null
  return <TemplateEditorDialog {...props} />
}

type TemplateEditorDialogProps = Omit<TemplateEditorProps, 'open'>

function TemplateEditorDialog({
  template,
  onOpenChange,
  isSystem,
  sortOrderHint,
  onSaved,
}: TemplateEditorDialogProps) {
  const isNew = template === null
  const initial = useMemo<CommandTemplate>(
    () =>
      template
        ? cloneTemplate(template)
        : createEmptyTemplate({ type: isSystem ? 'System' : 'User', sortOrder: sortOrderHint ?? 0 }),
    [template, isSystem, sortOrderHint],
  )

  const [draft, setDraft] = useState<CommandTemplate>(initial)
  const [formatsText, setFormatsText] = useState(listToText(initial.supportedInputFormats))
  const [tagsText, setTagsText] = useState(listToText(initial.tags))
  const [filtersText, setFiltersText] = useState(listToText(initial.requiredFilters))
  const [baseline, setBaseline] = useState(() => JSON.stringify(normalizeTemplateDraft(initial)))

  const [categories, setCategories] = useState<string[]>([])
  const [allTemplates, setAllTemplates] = useState<CommandTemplate[]>([])
  const [report, setReport] = useState<HardwareReportDto | null>(null)
  const [auxLoading, setAuxLoading] = useState(true)
  const [loadError, setLoadError] = useState('')

  const [issues, setIssues] = useState<TemplateIssue[]>([])
  const [validating, setValidating] = useState(false)
  const [validateError, setValidateError] = useState('')

  const [testResult, setTestResult] = useState<TemplateTestResultDto | null>(null)
  const [isTesting, setIsTesting] = useState(false)
  const [isSaving, setIsSaving] = useState(false)
  const [confirmDiscardOpen, setConfirmDiscardOpen] = useState(false)
  const [selectedPlaceholder, setSelectedPlaceholder] = useState('input')

  const commandRef = useRef<HTMLTextAreaElement | null>(null)
  const validateSeq = useRef(0)

  const normalized = useMemo(() => normalizeTemplateDraft(draft), [draft])
  const dirty = useMemo(() => JSON.stringify(normalized) !== baseline, [normalized, baseline])
  const counts = countIssues(issues)
  const duplicate = findDuplicateName(allTemplates, normalized.name, template?.id ?? null)
  const undeclared = undeclaredPlaceholders(draft.commandArgs, draft.parameters)
  const preview = buildCommandPreview(draft)
  const errorFields = useMemo(() => {
    const fields = new Set<string>()
    for (const issue of issues) {
      if (issue.severity === 'Error' && issue.field) fields.add(issue.field.toLowerCase())
    }
    return fields
  }, [issues])

  /* ── 辅助信息：分类 / 已有模板名（同名提示）/ 硬件编码器（可用性标记） ──── */
  useEffect(() => {
    let cancelled = false
    const run = async () => {
      const results = await Promise.allSettled([
        templatesApi.categories(),
        templatesApi.list({ includeSystem: true }),
        systemApi.encoders(),
      ])
      if (cancelled) return

      const [categoriesResult, templatesResult, encodersResult] = results
      if (categoriesResult.status === 'fulfilled') {
        setCategories(categoriesResult.value.map((item) => item.name))
      }
      if (templatesResult.status === 'fulfilled') {
        setAllTemplates(templatesResult.value)
      }
      if (encodersResult.status === 'fulfilled') {
        setReport(encodersResult.value)
      }
      const failed = results.find((result) => result.status === 'rejected')
      if (failed && failed.status === 'rejected') setLoadError(getErrorMessage(failed.reason))
      setAuxLoading(false)
    }
    void run()
    return () => {
      cancelled = true
    }
  }, [])

  /* ── 实时校验（防抖 400ms）──────────────────────────────────────────── */
  const runValidation = useCallback(async (payload: CommandTemplate): Promise<TemplateIssue[] | null> => {
    const seq = (validateSeq.current += 1)
    setValidating(true)
    try {
      const result = await templatesApi.validate(payload)
      if (seq !== validateSeq.current) return []
      setValidateError('')
      setIssues(result.issues ?? [])
      return result.issues ?? []
    } catch (error) {
      if (seq === validateSeq.current) {
        setValidateError(getErrorMessage(error))
        setIssues([])
      }
      return null
    } finally {
      if (seq === validateSeq.current) setValidating(false)
    }
  }, [])

  useEffect(() => {
    const timer = setTimeout(() => {
      void runValidation(normalized)
    }, 400)
    return () => clearTimeout(timer)
  }, [normalized, runValidation])

  /* ── 草稿编辑 ──────────────────────────────────────────────────────── */
  const patch = (changes: Partial<CommandTemplate>) => {
    setDraft((current) => ({ ...current, ...changes }))
  }

  const updateParameter = (index: number, changes: Partial<TemplateParameter>) => {
    setDraft((current) => ({
      ...current,
      parameters: current.parameters.map((parameter, position) =>
        position === index ? { ...parameter, ...changes } : parameter,
      ),
    }))
  }

  const addParameter = (parameter: TemplateParameter) => {
    setDraft((current) => ({ ...current, parameters: [...current.parameters, parameter] }))
  }

  const moveParameter = (index: number, delta: number) => {
    setDraft((current) => {
      const next = [...current.parameters]
      const target = index + delta
      if (target < 0 || target >= next.length) return current
      const [item] = next.splice(index, 1)
      next.splice(target, 0, item)
      return { ...current, parameters: next }
    })
  }

  const removeParameter = (index: number) => {
    setDraft((current) => ({
      ...current,
      parameters: current.parameters.filter((_, position) => position !== index),
    }))
  }

  /** 出错字段的控件样式与服务端信息（用于「高亮出问题的字段」） */
  const fieldClass = (key: string) => cx('w-full', errorFields.has(key) ? 'border-danger' : '')

  const fieldError = (key: string) => {
    const messages = issues
      .filter((issue) => issue.severity === 'Error' && (issue.field ?? '').toLowerCase() === key)
      .map((issue) => issue.message)
    return messages.length > 0 ? messages.join('；') : null
  }

  /* ── 关闭与保存 ────────────────────────────────────────────────────── */
  const requestClose = () => {
    if (isSaving || isTesting) return
    if (dirty) {
      setConfirmDiscardOpen(true)
      return
    }
    onOpenChange(false)
  }

  const saveDraft = async (): Promise<CommandTemplate | null> => {
    setIsSaving(true)
    try {
      const payload = normalizeTemplateDraft(draft)
      const saved = template?.id
        ? await templatesApi.update(template.id, payload)
        : await templatesApi.create(payload)
      setDraft(cloneTemplate(saved))
      setFormatsText(listToText(saved.supportedInputFormats))
      setTagsText(listToText(saved.tags))
      setFiltersText(listToText(saved.requiredFilters))
      setBaseline(JSON.stringify(normalizeTemplateDraft(saved)))
      return saved
    } catch (error) {
      toast.error(`保存失败：${getErrorMessage(error)}`)
      return null
    } finally {
      setIsSaving(false)
    }
  }

  const save = async () => {
    if (counts.errors > 0) {
      toast.warning(`还有 ${counts.errors} 个错误未修正，无法保存。`)
      return
    }
    const saved = await saveDraft()
    if (!saved) return
    toast.success(`模板「${saved.name}」已保存`)
    onSaved(saved, { closeEditor: true })
  }

  /** 复制一段文本并给出 §5.3 的 Toast 文案 */
  const copyPreview = async (text: string) => {
    let ok = false
    try {
      ok = await copyText(text)
    } catch (error) {
      toast.error(`复制失败：${getErrorMessage(error)}`)
      return
    }
    if (!ok) {
      toast.error(COPY_FAILURE_MESSAGES.templateEditor)
      return
    }
    toast.success('命令已复制到剪贴板')
  }

  const runTest = async () => {
    if (isTesting) return
    setIsTesting(true)
    setTestResult(null)
    try {
      // 1. 先校验（服务端完整结果）
      const fresh = await runValidation(normalizeTemplateDraft(draft))
      if (fresh === null) {
        toast.error('模板试运行失败，请看下方详情')
        return
      }
      const errors = fresh.filter((issue) => issue.severity === 'Error').length
      if (errors > 0) {
        toast.error('模板试运行失败，请看下方详情')
        return
      }

      // 2. 试运行端点只接受已保存的模板：草稿有改动（或本身是新建）时先保存
      let target = template
      if (dirty || !template?.id) {
        const saved = await saveDraft()
        if (!saved) {
          toast.error('模板试运行失败，请看下方详情')
          return
        }
        target = saved
        toast.success(`模板「${saved.name}」已保存`)
        onSaved(saved, { closeEditor: false })
      }
      if (!target?.id) return

      const result = await templatesApi.test(target.id, parameterDefaultValues(draft))
      setTestResult(result)
      if (result.success) toast.success('模板试运行成功，该模板在本机可以跑通')
      else toast.error('模板试运行失败，请看下方详情')
    } catch (error) {
      toast.error(`试运行出错：${getErrorMessage(error)}`)
    } finally {
      setIsTesting(false)
    }
  }

  /* ── 参数辅助操作 ──────────────────────────────────────────────────── */
  const addPresetParameter = (key: string) => {
    const preset = PARAMETER_PRESETS.find((item) => item.key === key)
    if (!preset) return
    if (draft.parameters.some((parameter) => parameter.name.toLowerCase() === preset.key)) {
      toast.warning(`已经存在参数「${preset.key}」，无需重复添加。`)
      return
    }
    addParameter({
      name: preset.key,
      label: preset.displayLabel,
      description: preset.description,
      kind: preset.kind,
      defaultValue: preset.defaultValue,
      options: preset.options.map((option) => ({ ...option })),
      min: preset.min,
      max: preset.max,
      trueValue: '1',
      falseValue: '',
    })
  }

  const declarePlaceholder = (name: string) => {
    addParameter({
      name: normalizeParameterName(name),
      label: name,
      description: '',
      kind: 'Text',
      defaultValue: '',
      options: [],
      min: null,
      max: null,
      trueValue: '1',
      falseValue: '',
    })
  }

  const insertPlaceholder = () => {
    if (!selectedPlaceholder) return
    const base = draft.commandArgs.trimEnd()
    patch({ commandArgs: base === '' ? `{${selectedPlaceholder}}` : `${base} {${selectedPlaceholder}}` })
    commandRef.current?.focus()
  }

  /* ── 硬件编码器下拉项（族 + 具体编码器）──────────────────────────────── */
  const encoderOptions = useMemo(() => {
    const families = new Map<string, { label: string; disabledHint: boolean }>()
    for (const [id, label] of Object.entries(ENCODER_FAMILY_NAMES)) {
      if (id === 'software' || id === 'other') continue
      families.set(id, { label, disabledHint: true })
    }
    const concrete: { value: string; label: string; unavailable: boolean }[] = []
    for (const encoder of report?.encoders ?? []) {
      const family = encoder.family.toLowerCase()
      if (family !== 'software' && families.has(family)) {
        const available = families.get(family)
        if (available && !encoder.isAvailable) available.disabledHint = false
      }
      concrete.push({
        value: encoder.name,
        label: `${encoder.name} · ${encoder.displayName}${encoder.isAvailable ? '' : '（本机不可用）'}`,
        unavailable: !encoder.isAvailable,
      })
    }
    for (const family of families.values()) family.disabledHint = family.disabledHint && report !== null
    return {
      families: Array.from(families.entries()).map(([value, info]) => ({
        value,
        label: `${info.label} · 按族自动挑选${info.disabledHint ? '（本机不可用）' : ''}`,
      })),
      concrete,
    }
  }, [report])

  const currentRequired = (draft.requiredEncoder ?? '').trim()
  const knownEncoders = new Set([
    ...encoderOptions.families.map((item) => item.value.toLowerCase()),
    ...encoderOptions.concrete.map((item) => item.value.toLowerCase()),
  ])
  const unknownEncoder = currentRequired && !knownEncoders.has(currentRequired.toLowerCase())
  const detectionLabel = report?.detectedAt ? formatDateTimeMinute(report.detectedAt) : ''

  const title = editorTitle(isNew, isSystem)

  /* ── 渲染 ──────────────────────────────────────────────────────────── */
  return (
    <>
      <Modal
        open
        onOpenChange={(next) => {
          if (!next) requestClose()
        }}
        size="xl"
        title={title}
      >
        <div className="mb-3 flex flex-wrap gap-[0.4rem]">
          <Badge tone={isSystem ? 'primary' : 'info'}>{isSystem ? '系统预设' : '用户模板'}</Badge>
          <Badge>{isNew ? '新建' : '编辑'}</Badge>
        </div>

        {auxLoading ? (
          <div className="ffui-loading-row">
            <span className="spinner" aria-hidden="true" />
            <span>正在读取分类与硬件编码器状态…</span>
          </div>
        ) : null}

        {loadError ? (
          <Alert tone="warning" role="status">
            辅助信息加载失败：{loadError}（不影响手动填写与保存）
          </Alert>
        ) : null}

        {isSystem ? (
          <Alert tone="info" role="note">
            系统预设可以编辑，但应用升级时同名预设会被内置内容覆盖（按预设版本同步）。需要长期保留修改，请改完后复制成普通模板。
          </Alert>
        ) : null}

        {/* 1. 基本信息 */}
        <section className="mb-5">
          <h3 className="ffui-section-title">基本信息</h3>
          <div className="grid gap-3 sm:grid-cols-2">
            <TeField label="模板名称" required htmlFor="te-name" error={fieldError('name')}>
              <input
                id="te-name"
                type="text"
                className={fieldClass('name')}
                value={draft.name}
                placeholder="例如：压缩 · 均衡（CRF 23）"
                onChange={(event) => patch({ name: event.target.value })}
                onBlur={() => patch({ name: draft.name.trim().replace(/^\{+|\}+$/g, '').trim() })}
              />
            </TeField>

            <TeField
              label="分类"
              htmlFor="te-category"
              hint="留空会保存为「未分类」。"
              error={fieldError('category')}
            >
              <input
                id="te-category"
                type="text"
                list="te-category-list"
                className={fieldClass('category')}
                value={draft.category}
                placeholder="选择已有分类，或直接输入新分类"
                onChange={(event) => patch({ category: event.target.value })}
              />
              <datalist id="te-category-list">
                {categories.map((category) => (
                  <option key={category} value={category} />
                ))}
              </datalist>
            </TeField>
          </div>

          {duplicate ? (
            <Alert tone="warning" role="status">
              已经有同名模板「{duplicate.name}」（{duplicate.type === 'System' ? '系统预设' : '用户模板'}）。
              保存时服务会自动改名（追加序号）；如果这是有意覆盖同名模板，请忽略这条提示。
            </Alert>
          ) : null}

          <TeField label="描述" htmlFor="te-description">
            <textarea
              id="te-description"
              rows={2}
              className="w-full"
              value={draft.description}
              placeholder="一句话说明这个模板做什么、什么场景用"
              onChange={(event) => patch({ description: event.target.value })}
            />
          </TeField>

          <div className="grid gap-3 sm:grid-cols-2">
            <TeField
              label="输出扩展名"
              required
              htmlFor="te-extension"
              hint="不需要写点号，会自动转成小写（mp4 / mkv / mp3 …）。"
              error={fieldError('outputextension')}
            >
              <input
                id="te-extension"
                type="text"
                className={fieldClass('outputextension')}
                value={draft.outputExtension}
                placeholder="mp4"
                onChange={(event) => patch({ outputExtension: event.target.value })}
                onBlur={() =>
                  patch({ outputExtension: draft.outputExtension.trim().replace(/^\.+/, '').toLowerCase() })
                }
              />
            </TeField>

            <TeField label="排序权重" htmlFor="te-sortorder" hint="数字越小越靠前。">
              <input
                id="te-sortorder"
                type="number"
                className="w-full"
                value={draft.sortOrder}
                onChange={(event) => patch({ sortOrder: Number(event.target.value) })}
              />
            </TeField>
          </div>

          <TeField
            label="支持的输入格式"
            htmlFor="te-formats"
            hint="留空表示不限格式；会自动去点号、转小写、去重。"
          >
            <input
              id="te-formats"
              type="text"
              className="w-full"
              value={formatsText}
              placeholder="mp4, mkv, avi（逗号或空格分隔）"
              onChange={(event) => {
                setFormatsText(event.target.value)
                patch({ supportedInputFormats: normalizeFormatList(event.target.value) })
              }}
              onBlur={() => {
                const formats = normalizeFormatList(formatsText)
                setFormatsText(listToText(formats))
                patch({ supportedInputFormats: formats })
              }}
            />
            {draft.supportedInputFormats.length > 0 ? (
              <div className="mt-1 flex flex-wrap gap-[0.35rem]">
                {draft.supportedInputFormats.map((format) => (
                  <Badge key={format} tone="info">
                    {format}
                    <button
                      type="button"
                      className="ml-[0.3rem] cursor-pointer border-0 bg-transparent p-0 text-inherit"
                      aria-label={`移除输入格式 ${format}`}
                      onClick={() => {
                        const formats = draft.supportedInputFormats.filter((item) => item !== format)
                        setFormatsText(listToText(formats))
                        patch({ supportedInputFormats: formats })
                      }}
                    >
                      ✕
                    </button>
                  </Badge>
                ))}
              </div>
            ) : null}
          </TeField>

          <TeField label="标签" htmlFor="te-tags">
            <input
              id="te-tags"
              type="text"
              className="w-full"
              value={tagsText}
              placeholder="压缩, 归档, 手机（逗号或空格分隔）"
              onChange={(event) => {
                setTagsText(event.target.value)
                patch({ tags: normalizeTagList(event.target.value) })
              }}
              onBlur={() => {
                const tags = normalizeTagList(tagsText)
                setTagsText(listToText(tags))
                patch({ tags })
              }}
            />
            {draft.tags.length > 0 ? (
              <div className="mt-1 flex flex-wrap gap-[0.35rem]">
                {draft.tags.map((tag) => (
                  <Badge key={tag}>
                    {tag}
                    <button
                      type="button"
                      className="ml-[0.3rem] cursor-pointer border-0 bg-transparent p-0 text-inherit"
                      aria-label={`移除标签 ${tag}`}
                      onClick={() => {
                        const tags = draft.tags.filter((item) => item !== tag)
                        setTagsText(listToText(tags))
                        patch({ tags })
                      }}
                    >
                      ✕
                    </button>
                  </Badge>
                ))}
              </div>
            ) : null}
          </TeField>

          <TeField
            label="所需滤镜"
            htmlFor="te-filters"
            hint="该模板依赖以下 FFmpeg 滤镜（多个用逗号或空格分隔）；如果当前 FFmpeg 构建没有编译它们，模板会显示为不可用。"
          >
            <input
              id="te-filters"
              type="text"
              className="w-full"
              value={filtersText}
              placeholder="subtitles, libass（逗号或空格分隔）"
              onChange={(event) => {
                setFiltersText(event.target.value)
                patch({ requiredFilters: splitList(event.target.value) })
              }}
              onBlur={() => {
                const filters = splitList(filtersText)
                setFiltersText(listToText(filters))
                patch({ requiredFilters: filters })
              }}
            />
            {draft.requiredFilters.length > 0 ? (
              <div className="mt-1 flex flex-wrap gap-[0.35rem]">
                {draft.requiredFilters.map((filter) => (
                  <Badge key={filter} tone="warning">
                    {filter}
                    <button
                      type="button"
                      className="ml-[0.3rem] cursor-pointer border-0 bg-transparent p-0 text-inherit"
                      aria-label={`移除滤镜 ${filter}`}
                      onClick={() => {
                        const filters = draft.requiredFilters.filter((item) => item !== filter)
                        setFiltersText(listToText(filters))
                        patch({ requiredFilters: filters })
                      }}
                    >
                      ✕
                    </button>
                  </Badge>
                ))}
              </div>
            ) : null}
          </TeField>

          <label className="mt-1 inline-flex cursor-pointer items-center gap-[0.4rem]">
            <input
              id="te-recommended"
              type="checkbox"
              className="w-auto"
              checked={draft.isRecommended}
              onChange={(event) => patch({ isRecommended: event.target.checked })}
            />
            在模板列表中标记为推荐
          </label>
        </section>

        {/* 2. FFmpeg 命令参数 */}
        <section className="mb-5">
          <h3 className="ffui-section-title">FFmpeg 命令参数</h3>
          <TeField
            label="命令参数"
            required
            htmlFor="te-command"
            hint='不含 ffmpeg 本身；不要写 -y / -n（执行层会按「文件已存在时的处理方式」自动添加）。含空格的路径写成 "{input}" 这种带引号的形式。'
            error={fieldError('commandargs')}
          >
            <textarea
              id="te-command"
              ref={commandRef}
              rows={5}
              className={fieldClass('commandargs')}
              value={draft.commandArgs}
              placeholder='-i "{input}" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} -c:a aac "{output}"'
              onChange={(event) => patch({ commandArgs: event.target.value })}
            />
          </TeField>

          <div className="flex flex-wrap items-end gap-2">
            <TeField label="内置占位符" htmlFor="te-placeholder-select" noMargin>
              <select
                id="te-placeholder-select"
                className="w-full"
                value={selectedPlaceholder}
                onChange={(event) => setSelectedPlaceholder(event.target.value)}
              >
                {PLACEHOLDER_GROUPS.map((group) => (
                  <optgroup key={group.title} label={group.title}>
                    {group.items.map((item) => (
                      <option key={item.name} value={item.name}>
                        {braced(item.name)} — {item.description}
                      </option>
                    ))}
                  </optgroup>
                ))}
              </select>
            </TeField>
            <div className="pb-[0.15rem]">
              <Button onClick={insertPlaceholder}>插入占位符</Button>
            </div>
          </div>
          <p className="text-[0.78rem] text-muted-text">
            浏览器端拿不到可靠的光标位置（app.js 里没有「插入到光标」的函数），所以这里只有一种行为：把选中的占位符追加到命令末尾。
          </p>

          <details className="mt-1">
            <summary className="cursor-pointer">
              全部内置占位符说明（共 {BUILT_IN_PLACEHOLDER_COUNT} 个）
            </summary>
            <table className="mt-2 w-full border-collapse text-[0.85rem]">
              <thead>
                <tr>
                  <th scope="col" className="border-b border-line px-2 py-1 text-left">
                    占位符
                  </th>
                  <th scope="col" className="border-b border-line px-2 py-1 text-left">
                    含义
                  </th>
                </tr>
              </thead>
              <tbody>
                {PLACEHOLDER_GROUPS.map((group) => (
                  <Fragment key={group.title}>
                    <tr>
                      <th
                        scope="colgroup"
                        colSpan={2}
                        className="border-b border-line bg-surface-3 px-2 py-1 text-left"
                      >
                        {group.title}
                      </th>
                    </tr>
                    {group.items.map((item) => (
                      <tr key={item.name}>
                        <td className="mono border-b border-line px-2 py-1">{braced(item.name)}</td>
                        <td className="border-b border-line px-2 py-1">{item.description}</td>
                      </tr>
                    ))}
                  </Fragment>
                ))}
              </tbody>
            </table>
          </details>
        </section>

        {/* 3. 命令预览 */}
        <section className="mb-5">
          <h3 className="ffui-section-title">命令预览</h3>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-[0.8rem] text-muted-text">
              按各参数的默认值展开；{'{input}'} 与 {'{output}'} 用占位路径代替。
            </span>
            <Button
              size="sm"
              disabled={!preview}
              onClick={() => void copyPreview(`ffmpeg ${preview}`)}
            >
              📋 复制
            </Button>
          </div>

          {!draft.commandArgs.trim() ? (
            <Alert tone="warning" role="status">
              命令参数为空，暂时无法生成预览。
            </Alert>
          ) : (
            <pre className="mono max-h-[260px] overflow-auto rounded-sm border border-line bg-code p-2 text-[0.8rem]">
              ffmpeg {preview}
            </pre>
          )}
          <p className="text-[0.75rem] text-muted-text">
            预览仅展开参数默认值与路径占位符；{'{encoder}'} 一类内置占位符由服务端在执行时展开（契约暂无「草稿预览」端点）。
          </p>
        </section>

        {/* 4. 试运行 */}
        <section className="mb-5">
          <h3 className="ffui-section-title">试运行</h3>
          <div className="flex flex-wrap items-center gap-3">
            <Button loading={isTesting} disabled={isTesting} onClick={() => void runTest()}>
              {isTesting ? '试运行中…' : '🧪 试运行模板'}
            </Button>
            <span className="text-[0.8rem] text-muted-text">
              会用 FFmpeg 合成 3 秒测试素材真实执行一次，可验证模板在<b>本机</b>能否跑通（通常几秒钟）。
            </span>
          </div>

          {testResult ? (
            <Alert
              tone={testResult.success ? 'success' : 'danger'}
              className="mt-2"
              role="status"
              icon={testResult.success ? '✅' : '❌'}
            >
              <div className="flex flex-col gap-[0.3rem]" aria-live="polite">
                <div>{testResult.message}</div>
                <div className="text-[0.78rem] text-muted-text">耗时 {formatTestDuration(testResult.durationMs)}</div>
                {testResult.command ? (
                  <div>
                    <div className="flex items-center gap-2">
                      <span className="text-[0.78rem] text-muted-text">执行的命令</span>
                      <IconButton
                        ariaLabel="复制试运行命令"
                        title="复制试运行命令"
                        onClick={() => void copyPreview(testResult.command)}
                      >
                        📋
                      </IconButton>
                    </div>
                    <pre className="mono max-h-[200px] overflow-auto whitespace-pre-wrap break-all rounded-sm border border-line bg-code p-2 text-[0.78rem]">
                      {testResult.command}
                    </pre>
                  </div>
                ) : null}
                {testResult.output ? (
                  <details>
                    <summary className="cursor-pointer">FFmpeg 输出（诊断信息）</summary>
                    <pre className="mono max-h-[240px] overflow-auto whitespace-pre-wrap break-all rounded-sm border border-line bg-code p-2 text-[0.75rem]">
                      {testResult.output}
                    </pre>
                  </details>
                ) : null}
              </div>
            </Alert>
          ) : null}
        </section>

        {/* 5. 校验结果（实时） */}
        <section className="mb-5">
          <h3 className="ffui-section-title">校验结果（实时）</h3>
          <div aria-live="polite" className="flex flex-col gap-[0.4rem]">
            {validateError ? (
              <Alert tone="danger" role="alert">
                校验失败：{validateError}
              </Alert>
            ) : null}

            {validating ? (
              <div className="ffui-loading-row">
                <span className="spinner" aria-hidden="true" />
                <span>正在校验…</span>
              </div>
            ) : null}

            {!validating && !validateError && issues.length === 0 ? (
              <Alert tone="success" role="status">
                没有发现问题。
              </Alert>
            ) : null}

            {sortIssues(issues).map((issue, index) => {
              const style = ISSUE_SEVERITY_STYLE[issue.severity]
              const fieldLabel = issueFieldLabel(issue.field)
              return (
                <Alert key={`${index}-${issue.message}`} tone={style.tone} role="status" icon={style.icon}>
                  <span className="mr-1 font-medium">{style.label}</span>
                  {issue.message}
                  {fieldLabel ? (
                    <span className="ml-1 text-[0.78rem] text-muted-text">（字段：{fieldLabel}）</span>
                  ) : null}
                </Alert>
              )
            })}
          </div>
          <p className="text-[0.75rem] text-muted-text">
            这里是 ITemplateService.Validate 的完整结果（错误 / 警告 / 提示）；存在错误时无法保存。
          </p>
        </section>

        {/* 6. 参数 */}
        <section className="mb-5">
          <h3 className="ffui-section-title">参数（{draft.parameters.length}）</h3>
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-[0.8rem] text-muted-text">添加常用参数：</span>
            {PARAMETER_PRESETS.map((preset) => (
              <Button key={preset.key} size="sm" onClick={() => addPresetParameter(preset.key)}>
                {preset.label}
              </Button>
            ))}
          </div>

          {undeclared.length > 0 ? (
            <Alert tone="warning" role="status">
              <div className="flex flex-col gap-[0.3rem]">
                <div>命令里用到、但没有声明的占位符（保存后用户无法为它们填值）：</div>
                <div className="flex flex-wrap gap-[0.35rem]">
                  {undeclared.map((name) => (
                    <Badge key={name} tone="warning">
                      {braced(name)}
                      <button
                        type="button"
                        className="ml-[0.3rem] cursor-pointer border-0 bg-transparent p-0 text-inherit"
                        aria-label={`为 ${name} 创建参数`}
                        onClick={() => declarePlaceholder(name)}
                      >
                        ＋
                      </button>
                    </Badge>
                  ))}
                </div>
              </div>
            </Alert>
          ) : null}

          {draft.parameters.length === 0 ? (
            <div className="ffui-empty-state">
              还没有声明任何参数。无参数模板也能用，只是用户在转换时无法调整这一项。
            </div>
          ) : null}

          {draft.parameters.map((parameter, index) => (
            <ParameterCard
              key={`${index}-${parameter.name}`}
              parameter={parameter}
              index={index}
              count={draft.parameters.length}
              used={isParameterUsedLocal(parameter, draft.commandArgs)}
              prefixError={fieldError('parameters')}
              onPatch={(changes) => updateParameter(index, changes)}
              onMove={(delta) => moveParameter(index, delta)}
              onRemove={() => removeParameter(index)}
            />
          ))}

          <div className="mt-2">
            <Button
              size="sm"
              onClick={() =>
                addParameter({
                  name: `param${draft.parameters.length + 1}`,
                  label: '',
                  description: '',
                  kind: 'Text',
                  defaultValue: '',
                  options: [],
                  min: null,
                  max: null,
                  trueValue: '1',
                  falseValue: '',
                })
              }
            >
              ＋ 添加空参数
            </Button>
          </div>
        </section>

        {/* 7. 硬件加速 */}
        <section className="mb-3">
          <h3 className="ffui-section-title">硬件加速</h3>
          <div className="grid gap-3 sm:grid-cols-2">
            <TeField label="硬件加速策略" htmlFor="te-hardware-mode" hint="「是否需要硬件加速」由这一项推导，两者不会再出现不一致。">
              <select
                id="te-hardware-mode"
                className="w-full"
                value={draft.hardwareMode}
                onChange={(event) => {
                  const mode = event.target.value as CommandTemplate['hardwareMode']
                  patch({
                    hardwareMode: mode,
                    requiredEncoder: mode === 'None' ? null : draft.requiredEncoder,
                  })
                }}
              >
                {HARDWARE_MODE_OPTIONS.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </TeField>

            <TeField
              label="所需编码器"
              htmlFor="te-required-encoder"
              error={fieldError('requiredencoder')}
              hint="「不使用」时会清空这一项；标记「（本机不可用）」的选项仍可选——模板也可能是为别的机器准备的。"
            >
              <select
                id="te-required-encoder"
                className={fieldClass('requiredencoder')}
                value={currentRequired}
                disabled={draft.hardwareMode === 'None'}
                onChange={(event) => patch({ requiredEncoder: event.target.value || null })}
              >
                <option value="">（不指定）</option>
                {unknownEncoder ? (
                  <option value={currentRequired}>{currentRequired}（不在编码器目录中）</option>
                ) : null}
                <optgroup label="按编码器族">
                  {encoderOptions.families.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </optgroup>
                <optgroup label="按具体编码器">
                  {encoderOptions.concrete.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </optgroup>
              </select>
              <span className="text-[0.75rem] text-muted-text">
                {detectionLabel
                  ? `本机可用性来自 ${detectionLabel} 的硬件检测。`
                  : auxLoading
                    ? '正在检测本机编码器，完成后会刷新标记…'
                    : '尚未拿到本机硬件检测结果，编码器可用性标记暂缺。'}
              </span>
            </TeField>

            <TeField
              label="目标编码格式"
              htmlFor="te-target-codec"
              hint="决定 {encoder} 会挑哪一类编码器，也决定没有硬件时回退到哪个软件编码器。"
            >
              <select
                id="te-target-codec"
                className="w-full"
                value={draft.targetCodec}
                onChange={(event) => patch({ targetCodec: event.target.value })}
              >
                <option value="">自动推断</option>
                {CODEC_CHOICES.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </TeField>
          </div>

          <Alert tone="info" role="note">
            <div className="flex flex-col gap-[0.35rem]">
              <div>硬件加速模板推荐用 {'{encoder}'} + {'{quality_args}'} + {'{preset_args}'} + {'{hwupload_args}'} 组合：</div>
              <ul className="m-0 list-disc pl-[1.1rem]">
                <li>
                  {'{encoder}'}：按目标格式和本机硬件自动挑编码器（h264_nvenc / h264_qsv / h264_videotoolbox /
                  h264_vaapi / libx264 …）。
                </li>
                <li>
                  {'{quality_args}'}：展开成当前编码器真正接受的质量参数（-crf / -cq / -qp / -q:v / -global_quality
                  …），由名为 quality 的参数驱动。
                </li>
                <li>{'{preset_args}'}：展开成该编码器的预设参数（如 -preset medium / -quality balanced）。</li>
                <li>{'{hwupload_args}'}：需要把帧上传到显存时补上滤镜（VA-API 的 format=nv12,hwupload）。</li>
              </ul>
              <div>
                不要硬写 -crf：硬件编码器不接受它，FFmpeg 只会打印一条警告然后忽略，命令照样成功退出，画质却完全没变——这种「看起来生效了」的模板最难排查。上面的校验会用警告提示这种情况。
              </div>
            </div>
          </Alert>
        </section>

        {/* 底部按钮栏 */}
        <div className="ffui-modal-footer">
          {counts.errors > 0 ? (
            <Badge tone="danger">{counts.errors} 个错误，无法保存</Badge>
          ) : counts.warnings > 0 ? (
            <Badge tone="warning">{counts.warnings} 个警告</Badge>
          ) : null}
          {isTesting ? <Badge tone="info">试运行中…</Badge> : null}
          <span className="mr-auto" />
          <Button disabled={isSaving || isTesting} onClick={requestClose}>
            取消
          </Button>
          <Button
            variant="primary"
            loading={isSaving}
            disabled={isSaving || counts.errors > 0}
            onClick={() => void save()}
          >
            {isSaving ? '保存中…' : '保存'}
          </Button>
        </div>
      </Modal>

      <ConfirmDialog
        open={confirmDiscardOpen}
        onOpenChange={setConfirmDiscardOpen}
        title="放弃未保存的修改？"
        message={`模板「${draft.name.trim() || '未命名'}」有未保存的修改。`}
        detail="关闭后会丢失本次编辑的全部内容，此操作无法撤销。"
        confirmText="放弃修改"
        cancelText="继续编辑"
        danger
        onConfirm={() => {
          setConfirmDiscardOpen(false)
          onOpenChange(false)
        }}
      />
    </>
  )
}

/* ── 局部小组件（不导出，避免 Fast Refresh 规则告警）─────────────────────── */

interface TeFieldProps {
  label: string
  htmlFor?: string
  hint?: string
  /** 最高优先级的错误文案（服务端校验结果） */
  error?: string | null
  required?: boolean
  noMargin?: boolean
  children: ReactNode
}

function TeField({ label, htmlFor, hint, error, required = false, noMargin = false, children }: TeFieldProps) {
  return (
    <div className={cx('flex flex-col gap-[0.3rem]', noMargin ? '' : 'mb-3')}>
      <label className="text-[0.85rem] font-medium text-secondary-text" htmlFor={htmlFor}>
        {label}
        {required ? (
          <>
            {' '}
            <span aria-hidden="true" className="text-danger">
              *
            </span>
            <span className="visually-hidden">（必填）</span>
          </>
        ) : null}
      </label>
      {children}
      {hint ? <span className="text-[0.78rem] text-muted-text">{hint}</span> : null}
      {error ? (
        <span className="text-[0.78rem] text-danger" role="alert">
          {error}
        </span>
      ) : null}
    </div>
  )
}

/** 参数是否被命令引用（§3.9 区块 6 的判定，等价 lib 里的实现，这里避免再引入一次查找） */
function isParameterUsedLocal(parameter: TemplateParameter, commandArgs: string): boolean {
  const name = parameter.name.trim().toLowerCase()
  if (!name) return false
  const placeholders = new Set(
    (commandArgs.match(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g) ?? []).map((token) => token.slice(1, -1).toLowerCase()),
  )
  if (placeholders.has(name)) return true
  if (name === 'quality' || name === 'q' || name === 'crf') {
    return ['quality_args', 'crf', 'quality', 'q'].some((item) => placeholders.has(item))
  }
  return false
}

interface ParameterCardProps {
  parameter: TemplateParameter
  index: number
  count: number
  used: boolean
  prefixError?: string | null
  onPatch: (changes: Partial<TemplateParameter>) => void
  onMove: (delta: number) => void
  onRemove: () => void
}

function ParameterCard({
  parameter,
  index,
  count,
  used,
  prefixError,
  onPatch,
  onMove,
  onRemove,
}: ParameterCardProps) {
  const nameValid = isValidParameterName(parameter.name) || !parameter.name.trim()

  const patchOption = (optionIndex: number, changes: Partial<TemplateParameterOption>) => {
    onPatch({
      options: parameter.options.map((option, position) =>
        position === optionIndex ? { ...option, ...changes } : option,
      ),
    })
  }

  const moveOption = (optionIndex: number, delta: number) => {
    const next = [...parameter.options]
    const target = optionIndex + delta
    if (target < 0 || target >= next.length) return
    const [item] = next.splice(optionIndex, 1)
    next.splice(target, 0, item)
    onPatch({ options: next })
  }

  const numberValue = (value: number | null): string => (value === null ? '' : String(value))

  const toNumber = (value: string): number | null => {
    if (value.trim() === '') return null
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : null
  }

  return (
    <div className="mt-2 rounded-md border border-line bg-surface p-3">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone="primary">#{index + 1}</Badge>
        <Badge tone={used ? 'success' : 'warning'}>{used ? '命令中已使用' : '命令中未使用'}</Badge>
        <span className="mr-auto" />
        <IconButton
          ariaLabel={`上移参数 ${parameter.name}`}
          disabled={index === 0}
          onClick={() => onMove(-1)}
        >
          ↑
        </IconButton>
        <IconButton
          ariaLabel={`下移参数 ${parameter.name}`}
          disabled={index === count - 1}
          onClick={() => onMove(1)}
        >
          ↓
        </IconButton>
        <IconButton ariaLabel={`删除参数 ${parameter.name}`} onClick={onRemove}>
          ✕
        </IconButton>
      </div>

      <div className="mt-2 grid gap-3 sm:grid-cols-2">
        <TeField
          label="参数名（占位符）"
          htmlFor={`te-p${index}-name`}
          hint={`命令里用 ${braced(parameter.name)} 引用；会自动转小写、去花括号、非法字符换成下划线。`}
        >
          <input
            id={`te-p${index}-name`}
            type="text"
            className="w-full"
            value={parameter.name}
            placeholder="quality"
            onChange={(event) => onPatch({ name: event.target.value })}
            onBlur={() => onPatch({ name: normalizeParameterName(parameter.name) })}
          />
        </TeField>

        <TeField label="界面显示名称" htmlFor={`te-p${index}-label`}>
          <input
            id={`te-p${index}-label`}
            type="text"
            className="w-full"
            value={parameter.label}
            placeholder="质量"
            onChange={(event) => onPatch({ label: event.target.value })}
          />
        </TeField>

        <TeField label="控件类型" htmlFor={`te-p${index}-kind`}>
          <select
            id={`te-p${index}-kind`}
            className="w-full"
            value={parameter.kind}
            onChange={(event) => {
              const kind = event.target.value as TemplateParameterKind
              onPatch({
                kind,
                options: kind === 'Select' ? parameter.options : [],
                min: kind === 'Number' ? parameter.min : null,
                max: kind === 'Number' ? parameter.max : null,
              })
            }}
          >
            {PARAMETER_KIND_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </TeField>

        <TeField label="默认值" htmlFor={`te-p${index}-default`}>
          <input
            id={`te-p${index}-default`}
            type="text"
            className="w-full"
            value={parameter.defaultValue}
            placeholder="留空表示不设置"
            onChange={(event) => onPatch({ defaultValue: event.target.value })}
          />
        </TeField>
      </div>

      <TeField label="帮助说明" htmlFor={`te-p${index}-description`}>
        <input
          id={`te-p${index}-description`}
          type="text"
          className="w-full"
          value={parameter.description}
          placeholder="显示在参数控件下方的一句话说明"
          onChange={(event) => onPatch({ description: event.target.value })}
        />
      </TeField>

      {!nameValid ? (
        <Alert tone="warning" role="status">
          参数名无效：只允许小写字母、数字和下划线，且不能以数字开头，否则 {braced(parameter.name)} 不会被识别成占位符。
        </Alert>
      ) : null}

      {prefixError ? (
        <Alert tone="danger" role="alert">
          {prefixError}
        </Alert>
      ) : null}

      {parameter.kind === 'Number' ? (
        <div className="grid gap-3 sm:grid-cols-2">
          <TeField label="最小值" htmlFor={`te-p${index}-min`}>
            <input
              id={`te-p${index}-min`}
              type="number"
              step="any"
              className="w-full"
              value={numberValue(parameter.min)}
              onChange={(event) => onPatch({ min: toNumber(event.target.value) })}
            />
          </TeField>
          <TeField label="最大值" htmlFor={`te-p${index}-max`}>
            <input
              id={`te-p${index}-max`}
              type="number"
              step="any"
              className="w-full"
              value={numberValue(parameter.max)}
              onChange={(event) => onPatch({ max: toNumber(event.target.value) })}
            />
          </TeField>
        </div>
      ) : null}

      {parameter.kind === 'Boolean' ? (
        <div className="grid gap-3 sm:grid-cols-2">
          <TeField label="开启时替换成" htmlFor={`te-p${index}-true`}>
            <input
              id={`te-p${index}-true`}
              type="text"
              className="w-full"
              value={parameter.trueValue}
              placeholder="1"
              onChange={(event) => onPatch({ trueValue: event.target.value })}
            />
          </TeField>
          <TeField label="关闭时替换成" htmlFor={`te-p${index}-false`}>
            <input
              id={`te-p${index}-false`}
              type="text"
              className="w-full"
              value={parameter.falseValue}
              placeholder="留空表示移除该片段"
              onChange={(event) => onPatch({ falseValue: event.target.value })}
            />
          </TeField>
        </div>
      ) : null}

      {parameter.kind === 'Select' ? (
        <div className="mt-1 rounded-sm border border-line bg-surface-3 p-2">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-[0.8rem] text-muted-text">下拉选项（{parameter.options.length}）</span>
            <span className="mr-auto" />
            <Button
              size="sm"
              onClick={() => onPatch({ options: [...parameter.options, { value: '', label: '', description: '' }] })}
            >
              ＋ 添加选项
            </Button>
          </div>

          {parameter.options.length === 0 ? (
            <Alert tone="warning" role="status">
              下拉参数至少需要一个选项，否则转换时用户没有可选的值。
            </Alert>
          ) : null}

          {parameter.options.map((option, optionIndex) => (
            <div key={optionIndex} className="mt-2 flex flex-wrap items-end gap-2">
              <div className="min-w-[8rem] flex-1">
                <label className="visually-hidden" htmlFor={`te-p${index}-opt${optionIndex}-value`}>
                  选项值
                </label>
                <input
                  id={`te-p${index}-opt${optionIndex}-value`}
                  type="text"
                  className="w-full"
                  value={option.value}
                  placeholder="值"
                  onChange={(event) => patchOption(optionIndex, { value: event.target.value })}
                />
              </div>
              <div className="min-w-[8rem] flex-1">
                <label className="visually-hidden" htmlFor={`te-p${index}-opt${optionIndex}-label`}>
                  显示文本
                </label>
                <input
                  id={`te-p${index}-opt${optionIndex}-label`}
                  type="text"
                  className="w-full"
                  value={option.label}
                  placeholder="显示文本"
                  onChange={(event) => patchOption(optionIndex, { label: event.target.value })}
                />
              </div>
              <div className="min-w-[8rem] flex-1">
                <label className="visually-hidden" htmlFor={`te-p${index}-opt${optionIndex}-description`}>
                  选项说明
                </label>
                <input
                  id={`te-p${index}-opt${optionIndex}-description`}
                  type="text"
                  className="w-full"
                  value={option.description}
                  placeholder="说明（可选）"
                  onChange={(event) => patchOption(optionIndex, { description: event.target.value })}
                />
              </div>
              <div className="flex items-center gap-[0.25rem]">
                <IconButton
                  ariaLabel="上移选项"
                  disabled={optionIndex === 0}
                  onClick={() => moveOption(optionIndex, -1)}
                >
                  ↑
                </IconButton>
                <IconButton
                  ariaLabel="下移选项"
                  disabled={optionIndex === parameter.options.length - 1}
                  onClick={() => moveOption(optionIndex, 1)}
                >
                  ↓
                </IconButton>
                <IconButton
                  ariaLabel="删除选项"
                  onClick={() =>
                    onPatch({ options: parameter.options.filter((_, position) => position !== optionIndex) })
                  }
                >
                  ✕
                </IconButton>
              </div>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  )
}
