/**
 * 模板选择器（转换页 / 批量页共用，对应 ui-spec §3.6 `TemplateSelector.razor`）。
 *
 * 关键约定：
 * 1. **模板可用性一律来自后端**（`GET /api/templates` 的 `available` / `unavailableReason`），
 *    前端不自己判断本机有没有某个编码器；
 * 2. 编码器徽标只做展示：`Required` 用硬件报告里同族的可用成员；
 *    `Auto` / `None` 用 `GET /api/system/encoders/recommend`（每种 编码格式+必需族 只查一次）；
 * 3. 每一行都是真 `<button>`（键盘可聚焦、回车/空格触发、`aria-pressed` 表达选中）；
 * 4. 选中只通过 `onChange` 通知父组件，**不修改传入的 props**；
 * 5. 搜索防抖 250 ms，用「代号」让更晚的输入作废（不依赖可取消的定时器竞态）。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { getErrorMessage } from '../../api/client'
import { systemApi, templatesApi } from '../../api/endpoints'
import type {
  CommandTemplate,
  HardwareEncoderDto,
  HardwareReportDto,
  TemplateListItem,
} from '../../api/types'
import { onWsEvent } from '../../api/ws'
import { cx } from '../../lib/cx'
import { toast } from '../../store/toast'
import { Alert } from '../ui/Alert'
import { Badge } from '../ui/Badge'
import { Button } from '../ui/Button'
import { describeEncoder, familyDisplayName, isHardwareEncoderName } from '../convert/hardwareLabels'

/** 搜索防抖时长（毫秒） */
export const TEMPLATE_SEARCH_DEBOUNCE_MS = 250

export interface TemplateSelectorProps {
  selectedTemplate: CommandTemplate | null
  onChange: (template: CommandTemplate) => void
  /** 当前输入文件路径（只用于「输入格式可能不兼容」提示） */
  inputPath?: string | null
  /** 初始是否显示不可用模板（false = 隐藏并给计数提示），组件内可切换 */
  showUnavailable?: boolean
}

/** 一行的完整呈现数据 */
interface TemplateRow {
  template: TemplateListItem
  category: string
  categoryCount: number
  startsGroup: boolean
  isSelected: boolean
  isUnavailable: boolean
  unavailableReason: string | null
  encoderBadge: string | null
  encoderBadgeTone: 'default' | 'primary' | 'success' | 'danger' | 'warning' | 'info'
  inputFormatRisky: boolean
  inputExtension: string | null
  parameterSummary: string | null
}

/** 编码格式归一化（展示用；真正的编码器挑选由后端完成） */
function normalizeCodec(value: string | null | undefined): string {
  const key = (value ?? '').trim().toLowerCase()
  switch (key) {
    case 'avc':
    case 'h264':
    case 'x264':
    case 'avc1':
      return 'h264'
    case 'h265':
    case 'hevc':
    case 'x265':
    case 'hvc1':
    case 'hev1':
      return 'hevc'
    case 'av1':
    case 'av01':
      return 'av1'
    case 'vp9':
    case 'vp09':
      return 'vp9'
    case 'vp8':
      return 'vp8'
    case 'prores':
      return 'prores'
    default:
      return key
  }
}

/** 硬件加速策略：兼容旧数据（只标了需要硬件加速但没升级硬件模式） */
function effectiveMode(template: CommandTemplate): 'None' | 'Auto' | 'Required' {
  if (template.hardwareMode !== 'None') return template.hardwareMode
  if (template.requiresHardwareAcceleration && (template.requiredEncoder ?? '').trim() !== '') return 'Required'
  return 'None'
}

function needsEncoder(template: CommandTemplate): boolean {
  return (
    effectiveMode(template) !== 'None' ||
    (template.commandArgs ?? '').toLowerCase().includes('{encoder}')
  )
}

function determineCodec(template: CommandTemplate): string {
  if ((template.targetCodec ?? '').trim() !== '') return normalizeCodec(template.targetCodec)
  const required = (template.requiredEncoder ?? '').trim()
  if (required !== '') {
    // 具体名形如 `h264_nvenc`：取前缀里的编码格式
    const fromName = normalizeCodec(required.split('_')[0])
    if (['h264', 'hevc', 'av1', 'vp9', 'vp8', 'prores'].includes(fromName)) return fromName
  }
  const fromExtension = normalizeCodec(template.outputExtension)
  return ['h264', 'hevc', 'av1', 'vp9', 'vp8'].includes(fromExtension) ? fromExtension : 'h264'
}

function encoderKey(codec: string, required: string | null | undefined): string {
  return `${codec}|${(required ?? '').trim()}`
}

function extensionOf(path: string | null | undefined): string | null {
  if (!path) return null
  const name = path.replace(/\\/g, '/').split('/').pop() ?? ''
  const index = name.lastIndexOf('.')
  if (index <= 0) return null
  const extension = name.slice(index + 1).toLowerCase()
  return extension === '' ? null : extension
}

/** 模板是否匹配搜索词（名称 / 描述 / 分类 / 标签，忽略大小写） */
function matches(template: CommandTemplate, search: string): boolean {
  const needle = search.toLowerCase()
  return (
    (template.name ?? '').toLowerCase().includes(needle) ||
    (template.description ?? '').toLowerCase().includes(needle) ||
    (template.category ?? '').toLowerCase().includes(needle) ||
    (template.tags ?? []).some((tag) => (tag ?? '').toLowerCase().includes(needle))
  )
}

export function TemplateSelector({
  selectedTemplate,
  onChange,
  inputPath = null,
  showUnavailable = false,
}: TemplateSelectorProps) {
  const [templates, setTemplates] = useState<TemplateListItem[]>([])
  const [categories, setCategories] = useState<string[]>([])
  const [report, setReport] = useState<HardwareReportDto | null>(null)
  const [recommended, setRecommended] = useState<Record<string, string>>({})
  const [isLoading, setIsLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [availabilityError, setAvailabilityError] = useState<string | null>(null)

  const [groupByCategory, setGroupByCategory] = useState(true)
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null)
  const [searchInput, setSearchInput] = useState('')
  const [searchText, setSearchText] = useState('')
  const [showUnavailableOverride, setShowUnavailableOverride] = useState<boolean | null>(null)

  const searchGeneration = useRef(0)
  const mounted = useRef(true)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      searchGeneration.current += 1
    }
  }, [])

  /** 拉取模板 + 分类 + 硬件报告，并预计算推荐编码器映射 */
  const loadData = useCallback(async () => {
    try {
      const [list, categoryList, hardware] = await Promise.all([
        templatesApi.list({ includeSystem: true }),
        templatesApi.categories(),
        systemApi.encoders(),
      ])
      if (!mounted.current) return

      setTemplates(list ?? [])
      setCategories((categoryList ?? []).map((item) => item.name))
      setReport(hardware ?? null)
      setLoadError(null)

      // 推荐编码器：每种「编码格式 + 必需族」只查一次
      const pairs = new Map<string, { codec: string; required: string | null }>()
      for (const template of list ?? []) {
        if (!needsEncoder(template)) continue
        const mode = effectiveMode(template)
        if (mode === 'Required') continue // Required 的徽标用硬件报告即可
        const codec = determineCodec(template)
        const required = (template.requiredEncoder ?? '').trim() || null
        const key = encoderKey(codec, required)
        if (!pairs.has(key)) pairs.set(key, { codec, required })
      }

      const resolved: Record<string, string> = {}
      let recommendError: string | null = null
      for (const [key, pair] of pairs) {
        try {
          const result = await systemApi.recommendEncoder(pair.codec, pair.required)
          if (result.encoder) resolved[key] = result.encoder
        } catch (recommendFailure) {
          recommendError = `查询推荐编码器失败：${getErrorMessage(recommendFailure)}`
          break
        }
      }
      if (!mounted.current) return
      setRecommended(resolved)
      setAvailabilityError(recommendError)
      if (recommendError) toast.warning(recommendError)
    } catch (error) {
      if (!mounted.current) return
      const message = `加载模板失败：${getErrorMessage(error)}`
      setLoadError(message)
      toast.error(message)
    } finally {
      if (mounted.current) setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    // 先让出一轮微任务，避免在 effect 的同步阶段触发 setState（级联渲染）
    void (async () => {
      await Promise.resolve()
      await loadData()
    })()
  }, [loadData])

  // 模板被别处改动（导入/同步/增删改）时重新拉一次
  useEffect(
    () =>
      onWsEvent('templates.changed', () => {
        void (async () => {
          await Promise.resolve()
          await loadData()
        })()
      }),
    [loadData],
  )

  const handleSearchInput = (raw: string) => {
    setSearchInput(raw)
    const generation = ++searchGeneration.current
    setTimeout(() => {
      if (!mounted.current || generation !== searchGeneration.current) return
      setSearchText(raw)
    }, TEMPLATE_SEARCH_DEBOUNCE_MS)
  }

  const handleSearchCommit = (raw: string) => {
    setSearchInput(raw)
    searchGeneration.current += 1
    setSearchText(raw)
  }

  const effectiveShowUnavailable = showUnavailableOverride ?? showUnavailable

  const { rows, unavailableCount } = useMemo(() => {
    const categoryOrder = new Map<string, number>()
    categories.forEach((name, index) => categoryOrder.set(name.toLowerCase(), index))

    let matched = templates
    if (selectedCategory !== null) {
      matched = matched.filter(
        (template) => (template.category ?? '').toLowerCase() === selectedCategory.toLowerCase(),
      )
    }
    const search = searchText.trim()
    if (search !== '') matched = matched.filter((template) => matches(template, search))

    const categoryCounts = new Map<string, number>()
    for (const template of matched) {
      const key = (template.category ?? '').toLowerCase()
      categoryCounts.set(key, (categoryCounts.get(key) ?? 0) + 1)
    }

    const ordered = groupByCategory
      ? [...matched].sort((left, right) => {
          const leftOrder = categoryOrder.get((left.category ?? '').toLowerCase()) ?? Number.MAX_SAFE_INTEGER
          const rightOrder = categoryOrder.get((right.category ?? '').toLowerCase()) ?? Number.MAX_SAFE_INTEGER
          if (leftOrder !== rightOrder) return leftOrder - rightOrder
          if (left.sortOrder !== right.sortOrder) return left.sortOrder - right.sortOrder
          return (left.name ?? '').toLowerCase().localeCompare((right.name ?? '').toLowerCase())
        })
      : matched // 平铺保持服务返回顺序（推荐优先）

    const inputExtension = extensionOf(inputPath)
    const encoders = report?.encoders ?? []
    const built: TemplateRow[] = []
    let unavailable = 0
    let lastCategory: string | null = null

    for (const template of ordered) {
      const category = template.category ?? ''
      const mode = effectiveMode(template)
      const codec = determineCodec(template)
      const recommendedName = needsEncoder(template)
        ? (recommended[encoderKey(codec, template.requiredEncoder)] ?? null)
        : null

      let encoderBadge: string | null = null
      let encoderBadgeTone: TemplateRow['encoderBadgeTone'] = 'info'
      let isUnavailable = false
      let unavailableReason: string | null = null

      if (mode === 'Required') {
        const required = (template.requiredEncoder ?? '').trim()
        if (required === '') {
          isUnavailable = true
          unavailableReason = '模板未指定编码器族'
          encoderBadge = '⚠ 硬件不可用'
          encoderBadgeTone = 'danger'
        } else if (!report) {
          isUnavailable = true
          unavailableReason = `${familyDisplayName(required)} 可用性未知：未能完成硬件编码器检测`
          encoderBadge = '⚠ 可用性未知'
          encoderBadgeTone = 'warning'
        } else if (template.available === false) {
          isUnavailable = true
          unavailableReason = template.unavailableReason ?? `需要 ${familyDisplayName(required)} 硬件编码器，本机不可用`
          encoderBadge = '⚠ 硬件不可用'
          encoderBadgeTone = 'danger'
        } else {
          const member = findAvailableMember(required, encoders)
          encoderBadge = `🚀 ${describeEncoder(member ?? required, encoders)}`
          encoderBadgeTone = 'success'
        }
      } else if (mode === 'Auto') {
        if (!recommendedName) {
          encoderBadge = '🚀 自动选择编码器'
          encoderBadgeTone = 'info'
        } else if (isHardwareEncoderName(recommendedName, encoders)) {
          encoderBadge = `🚀 将使用 ${describeEncoder(recommendedName, encoders)}`
          encoderBadgeTone = 'info'
        } else {
          encoderBadge = `将使用 ${describeEncoder(recommendedName, encoders)}（软件编码）`
          encoderBadgeTone = 'warning'
        }
      } else if (recommendedName) {
        encoderBadge = `🔧 使用 ${describeEncoder(recommendedName, encoders)}`
        encoderBadgeTone = 'default'
      }

      const formats = (template.supportedInputFormats ?? []).filter((item) => (item ?? '').trim() !== '')
      const inputFormatRisky =
        inputExtension !== null &&
        formats.length > 0 &&
        !formats.some((item) => item.trim().replace(/^\./, '').toLowerCase() === inputExtension)

      const plainParameters = (template.parameters ?? []).filter(
        (parameter) => (parameter.name ?? '').trim() !== '',
      )

      const row: TemplateRow = {
        template,
        category,
        categoryCount: categoryCounts.get(category.toLowerCase()) ?? 0,
        startsGroup: false,
        isSelected: selectedTemplate?.id === template.id,
        isUnavailable,
        unavailableReason,
        encoderBadge,
        encoderBadgeTone,
        inputFormatRisky,
        inputExtension: inputFormatRisky ? inputExtension : null,
        parameterSummary: plainParameters.length > 0 ? `${plainParameters.length} 个可调参数` : null,
      }

      if (isUnavailable) {
        unavailable++
        if (!effectiveShowUnavailable) continue
      }

      if (groupByCategory && lastCategory !== category.toLowerCase()) {
        row.startsGroup = true
        lastCategory = category.toLowerCase()
      }

      built.push(row)
    }

    return { rows: built, unavailableCount: unavailable }
  }, [
    categories,
    effectiveShowUnavailable,
    groupByCategory,
    inputPath,
    recommended,
    report,
    searchText,
    selectedCategory,
    selectedTemplate,
    templates,
  ])

  const hasHardware = (report?.encoders ?? []).some(
    (encoder) => encoder.isAvailable && encoder.type !== 'Software',
  )
  const capabilitySummary = !report
    ? '尚未检测到硬件编码能力，模板可用性未知。'
    : hasHardware
      ? capabilitySummaryText(report)
      : '未检测到硬件编码，将使用 CPU 软件编码。'

  const unavailableNotice = effectiveShowUnavailable
    ? `${unavailableCount} 个模板需要本机没有的硬件编码器，当前仅作展示、无法选择。`
    : `${unavailableCount} 个模板需要本机没有的硬件编码器，已隐藏。`

  const handleSelect = (row: TemplateRow) => {
    if (row.isUnavailable) return
    try {
      onChange(row.template)
    } catch (error) {
      toast.error(`选择模板失败：${getErrorMessage(error)}`)
    }
  }

  const clearFilters = () => {
    setSelectedCategory(null)
    setSearchInput('')
    setSearchText('')
    searchGeneration.current += 1
  }

  const searchId = 'ffui-template-search'

  return (
    <div className="rounded-md border border-line bg-surface overflow-hidden">
      <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3 bg-surface-3 border-b border-line">
        <h3 className="m-0 font-semibold" style={{ fontSize: '1rem' }}>
          选择转换模板
        </h3>
        <div className="flex gap-1" role="group" aria-label="模板列表显示方式">
          <Button size="sm" aria-pressed={groupByCategory} onClick={() => setGroupByCategory(true)}>
            按分类分组
          </Button>
          <Button size="sm" aria-pressed={!groupByCategory} onClick={() => setGroupByCategory(false)}>
            平铺列表
          </Button>
        </div>
      </div>

      {isLoading ? (
        <div className="ffui-loading-row" role="status">
          <span className="spinner" aria-hidden="true" />
          <span>正在加载模板与硬件编码器…</span>
        </div>
      ) : (
        <>
          {loadError ? (
            <div className="mx-4 mt-3 flex flex-wrap items-center justify-between gap-2">
              <Alert tone="danger" className="flex-1">
                {loadError}
              </Alert>
              <Button size="sm" onClick={() => void loadData()}>
                重试
              </Button>
            </div>
          ) : null}

          <div className="mx-4 mt-3">
            <Alert tone={!report || hasHardware ? 'info' : 'warning'} role="status">
              {capabilitySummary}
            </Alert>
          </div>

          {availabilityError ? (
            <div className="mx-4 mt-3">
              <Alert tone="warning" role="status">
                {availabilityError}
              </Alert>
            </div>
          ) : null}

          <div className="flex items-center gap-2 px-4 py-3 border-b border-line">
            <label className="text-secondary-text whitespace-nowrap" style={{ fontSize: '0.85rem' }} htmlFor={searchId}>
              搜索
            </label>
            <input
              id={searchId}
              type="search"
              className="flex-1 min-w-0"
              placeholder="按名称、描述或标签搜索模板…"
              value={searchInput}
              onChange={(event) => handleSearchInput(event.target.value)}
              onBlur={(event) => handleSearchCommit(event.target.value)}
            />
          </div>

          <div
            className="flex gap-1 px-4 py-2 bg-surface-3 border-b border-line overflow-x-auto"
            role="group"
            aria-label="模板分类筛选"
          >
            <button
              type="button"
              className={cx(
                'rounded-sm px-3 py-1.5 whitespace-nowrap cursor-pointer',
                selectedCategory === null ? 'bg-primary text-primary-contrast' : 'text-secondary-text hover:bg-surface-2',
              )}
              style={{ fontSize: '0.85rem', border: 'none' }}
              aria-pressed={selectedCategory === null}
              aria-label="显示全部分类的模板"
              onClick={() => setSelectedCategory(null)}
            >
              全部
            </button>
            {categories.map((category) => (
              <button
                key={category}
                type="button"
                className={cx(
                  'rounded-sm px-3 py-1.5 whitespace-nowrap cursor-pointer',
                  selectedCategory === category
                    ? 'bg-primary text-primary-contrast'
                    : 'text-secondary-text hover:bg-surface-2',
                )}
                style={{ fontSize: '0.85rem', border: 'none' }}
                aria-pressed={selectedCategory === category}
                aria-label={`筛选分类：${category}`}
                onClick={() => setSelectedCategory(category)}
              >
                {category}
              </button>
            ))}
          </div>

          {unavailableCount > 0 ? (
            <div className="mx-4 mt-3 flex flex-wrap items-center justify-between gap-2">
              <Alert tone={effectiveShowUnavailable ? 'info' : 'warning'} role="status" className="flex-1">
                {unavailableNotice}
              </Alert>
              <Button size="sm" onClick={() => setShowUnavailableOverride(!effectiveShowUnavailable)}>
                {effectiveShowUnavailable ? '隐藏不可用模板' : '显示不可用模板'}
              </Button>
            </div>
          ) : null}

          <div style={{ maxHeight: '420px', overflowY: 'auto' }}>
            {rows.length === 0 ? (
              <div className="ffui-empty-state">
                {templates.length === 0 ? (
                  <>
                    <p>没有模板</p>
                    <p className="text-muted-text mt-2" style={{ fontSize: '0.85rem' }}>
                      模板库为空，请先在「模板管理」中创建模板或重置系统预设。
                    </p>
                  </>
                ) : unavailableCount > 0 ? (
                  <>
                    <p>{unavailableCount} 个匹配的模板在本机不可用</p>
                    <Button size="sm" onClick={() => setShowUnavailableOverride(true)}>
                      显示不可用模板
                    </Button>
                  </>
                ) : (
                  <>
                    <p>没有匹配的搜索结果</p>
                    <Button size="sm" onClick={clearFilters}>
                      清除筛选
                    </Button>
                  </>
                )}
              </div>
            ) : (
              rows.map((row) => (
                <div key={row.template.id}>
                  {row.startsGroup ? (
                    <div className="ffui-section-title group-title flex items-center gap-2 px-4 pt-3 pb-1 text-secondary-text bg-surface border-t border-line">
                      <span>{row.category === '' ? '未分类' : row.category}</span>
                      <Badge>{row.categoryCount}</Badge>
                    </div>
                  ) : null}

                  <button
                    type="button"
                    className={cx(
                      'block w-full text-left cursor-pointer border-b border-line bg-transparent',
                      row.isSelected && 'bg-primary-light',
                      row.isUnavailable && 'opacity-65 cursor-not-allowed',
                    )}
                    style={{
                      padding: '1rem',
                      borderLeft: row.isSelected
                        ? '3px solid var(--primary-color)'
                        : row.inputFormatRisky
                          ? '3px solid var(--warning-color)'
                          : '3px solid transparent',
                    }}
                    disabled={row.isUnavailable}
                    aria-pressed={row.isSelected}
                    title={
                      row.isUnavailable
                        ? (row.unavailableReason ?? undefined)
                        : row.inputFormatRisky
                          ? '输入格式可能不兼容，仍然可以选择'
                          : undefined
                    }
                    onClick={() => handleSelect(row)}
                  >
                    <span className="flex flex-wrap items-center gap-2">
                      <span className="font-semibold">{row.template.name}</span>
                      <Badge tone={row.template.type === 'System' ? 'default' : 'success'}>
                        {row.template.type === 'System' ? '预设' : '自定义'}
                      </Badge>
                      {row.encoderBadge ? <Badge tone={row.encoderBadgeTone}>{row.encoderBadge}</Badge> : null}
                    </span>

                    <span className="block mt-1 text-secondary-text" style={{ fontSize: '0.9rem' }}>
                      {row.template.description}
                    </span>

                    <span className="flex flex-wrap gap-3 mt-2 text-muted-text" style={{ fontSize: '0.8rem' }}>
                      <span>📁 输出: .{row.template.outputExtension}</span>
                      {row.template.supportedInputFormats?.length > 0 ? (
                        <span>
                          📥 输入:{' '}
                          {row.template.supportedInputFormats.slice(0, 3).join(', ')}
                          {row.template.supportedInputFormats.length > 3 ? '…' : ''}
                        </span>
                      ) : null}
                      {row.parameterSummary ? <span>⚙ {row.parameterSummary}</span> : null}
                    </span>

                    {row.isUnavailable && row.unavailableReason ? (
                      <span
                        className="block mt-2 px-2 py-1 rounded-sm"
                        style={{ background: 'var(--danger-bg)', color: 'var(--danger-text)', fontSize: '0.8rem' }}
                      >
                        {row.unavailableReason}
                      </span>
                    ) : row.inputFormatRisky ? (
                      <span
                        className="block mt-2 px-2 py-1 rounded-sm"
                        style={{ background: 'var(--warning-bg)', color: 'var(--warning-text)', fontSize: '0.8rem' }}
                      >
                        输入格式可能不兼容（当前文件为 {row.inputExtension}）
                      </span>
                    ) : null}
                  </button>
                </div>
              ))
            )}
          </div>
        </>
      )}
    </div>
  )
}

/** 能力摘要：`本机可用硬件编码：{前两个}。` / `本机可用硬件编码：{前两个} 等 {N} 项。` */
function capabilitySummaryText(report: HardwareReportDto): string {
  const hardware = report.encoders.filter(
    (encoder) => encoder.isAvailable && encoder.type !== 'Software',
  )
  if (hardware.length === 0) return '未检测到硬件编码，将使用 CPU 软件编码。'
  const names = hardware.map((encoder) => (encoder.displayName?.trim() ? encoder.displayName : encoder.name))
  const head = names.slice(0, 2).join('、')
  return hardware.length > 2
    ? `本机可用硬件编码：${head} 等 ${hardware.length} 项。`
    : `本机可用硬件编码：${head}。`
}

/** 在硬件报告里找同族/同名的可用编码器（仅用于徽标展示） */
function findAvailableMember(required: string, encoders: readonly HardwareEncoderDto[]): string | null {
  const key = required.trim().toLowerCase()
  const exact = encoders.find(
    (encoder) => encoder.isAvailable && encoder.name.toLowerCase() === key,
  )
  if (exact) return exact.name

  const family = encoders.find(
    (encoder) =>
      encoder.isAvailable &&
      (encoder.family ?? '').toLowerCase() === key &&
      (encoder.type !== 'Software' || key === 'software'),
  )
  return family?.name ?? null
}
