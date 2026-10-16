/**
 * 模板管理页 `/templates`（对应 ui-spec §4.5、§8.1）。
 *
 * 结构：页头（标题 + 计数徽标）→ 提示条堆叠（系统预设同步结果 + 硬件能力）→ 操作按钮行
 * → 筛选工具栏（搜索 / 分类 / 排序 / 只看可用 / 视图切换）→ 模板网格（平铺或按分类分组）。
 *
 * 数据来源与边界：
 * - 模板列表来自 `GET /api/templates?includeSystem=true`，**可用性、不可用原因、缺失滤镜、
 *   输入格式展示串全部用服务端给的字段**，前端不做任何可用性推断；
 * - 分类计数来自 `GET /api/templates/categories`，分类筛选做成带计数的 chip（任务要求）；
 * - 搜索 / 排序 / 「只看本机可用」按 §4.5 在本地完成（与参考实现一致）；
 * - 「选择 / 直接使用」跳转 `/convert?template={id}`，这是 App.tsx 路由注释里约定的预选方式
 *   （查询参数，而不是共享 store）。
 */

import { useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import type { TemplateCategoryDto, TemplateListItem } from '../api/types'
import { getErrorMessage } from '../api/client'
import { systemApi, templatesApi } from '../api/endpoints'
import { onWsEvent, onWsStatus } from '../api/ws'
import {
  categoryDisplayName,
  downloadTextFile,
  exportFileName,
  matchesTemplateSearch,
} from '../lib/templateCatalog'
import { PAGE_TITLES, useDocumentTitle } from '../lib/title'
import { toast } from '../store/toast'
import { TemplateCardView } from '../components/template/TemplateCardView'
import { TemplateDetailViewer } from '../components/template/TemplateDetailViewer'
import { TemplateEditor } from '../components/template/TemplateEditor'
import { TemplateImportDialog } from '../components/template/TemplateImportDialog'
import { Alert } from '../components/ui/Alert'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { ConfirmDialog } from '../components/ui/ConfirmDialog'
import { Spinner } from '../components/ui/Spinner'

type SortMode = 'recommended' | 'name' | 'updated'
/** 视图：平铺网格 / 按分类分组 / 单列列表（§4.5 的 `▦ 平铺` / `☰ 分组`，外加列表视图） */
type LayoutMode = 'flat' | 'group' | 'list'

/** 拉一次「模板 + 分类计数」（挂载与刷新共用，避免把 setState 写在 effect 同步阶段） */
async function fetchTemplates(): Promise<{
  templates: TemplateListItem[]
  categories: TemplateCategoryDto[]
}> {
  const [templates, categories] = await Promise.all([
    templatesApi.list({ includeSystem: true }),
    templatesApi.categories(),
  ])
  return { templates, categories }
}

export function TemplatesPage() {
  useDocumentTitle(PAGE_TITLES.templates)
  const navigate = useNavigate()

  const [templates, setTemplates] = useState<TemplateListItem[]>([])
  const [categories, setCategories] = useState<TemplateCategoryDto[]>([])
  const [loading, setLoading] = useState(true)

  const [hardwareText, setHardwareText] = useState('正在检测本机可用的硬件编码器…')
  const [hardwareWarning, setHardwareWarning] = useState(false)

  const [syncSummary, setSyncSummary] = useState('')
  const [syncChanged, setSyncChanged] = useState(false)

  const [searchInput, setSearchInput] = useState('')
  const [searchText, setSearchText] = useState('')
  const [selectedCategory, setSelectedCategory] = useState('')
  const [sortMode, setSortMode] = useState<SortMode>('recommended')
  const [onlyAvailable, setOnlyAvailable] = useState(false)
  const [layout, setLayout] = useState<LayoutMode>('flat')

  const [exporting, setExporting] = useState(false)
  const [importOpen, setImportOpen] = useState(false)

  const [editorOpen, setEditorOpen] = useState(false)
  const [editorTemplate, setEditorTemplate] = useState<TemplateListItem | null>(null)
  const [editorIsSystem, setEditorIsSystem] = useState(false)

  const [viewing, setViewing] = useState<TemplateListItem | null>(null)
  const [viewerOpen, setViewerOpen] = useState(false)

  const [deleteTarget, setDeleteTarget] = useState<TemplateListItem | null>(null)
  const [deleteOpen, setDeleteOpen] = useState(false)

  const [selectedId, setSelectedId] = useState<string | null>(null)

  /* ── 加载：系统预设同步 → 硬件能力 → 模板列表 ───────────────────────── */
  useEffect(() => {
    let cancelled = false

    const syncAndLoad = async () => {
      // 1. 每次打开本页都自动同步一次系统预设（§4.5）
      try {
        const sync = await templatesApi.syncSystem()
        if (cancelled) return
        setSyncChanged(sync.added + sync.updated > 0)
        setSyncSummary(sync.summary?.trim() ? sync.summary : `系统模板已是最新（预设 v${sync.presetVersion}）`)
      } catch (error) {
        if (cancelled) return
        setSyncSummary('')
        toast.error(`同步系统预设模板失败：${getErrorMessage(error)}`)
      }

      // 2. 模板列表 + 分类计数
      try {
        const result = await fetchTemplates()
        if (cancelled) return
        setTemplates(result.templates)
        setCategories(result.categories)
      } catch (error) {
        if (cancelled) return
        setTemplates([])
        setCategories([])
        toast.error(`加载模板失败：${getErrorMessage(error)}`)
      } finally {
        if (!cancelled) setLoading(false)
      }

      // 3. 硬件能力（失败只影响可用性展示）
      try {
        const hardwareReport = await systemApi.encoders()
        if (cancelled) return
        const available = (hardwareReport.encoders ?? []).filter(
          (encoder) => encoder.isAvailable && encoder.type !== 'Software',
        )
        const summary = (hardwareReport.summary ?? '').trim()
        const base = summary
          ? summary
          : available.length > 0
            ? `检测到 ${available.length} 个可用的硬件编码器。`
            : '本机未检测到可用的硬件编码器，仅软件编码可用。'
        setHardwareText(
          hardwareReport.ffmpegVersion ? `${base}（检测于 FFmpeg ${hardwareReport.ffmpegVersion}）` : base,
        )
        setHardwareWarning(available.length === 0)
      } catch (error) {
        if (cancelled) return
        const message = getErrorMessage(error)
        setHardwareWarning(true)
        setHardwareText(`硬件检测失败，暂时无法判断哪些模板可在本机运行：${message}`)
        toast.warning(`硬件检测失败，模板可用性将显示为「未知」：${message}`)
      }
    }

    void syncAndLoad()
    return () => {
      cancelled = true
    }
  }, [])

  /** 重新加载模板列表（保存 / 删除 / 复制 / 导入 / WS 广播后调用） */
  const reload = useCallback(async (options: { silent?: boolean } = {}) => {
    if (!options.silent) setLoading(true)
    try {
      const result = await fetchTemplates()
      setTemplates(result.templates)
      setCategories(result.categories)
    } catch (error) {
      toast.error(`加载模板失败：${getErrorMessage(error)}`)
    } finally {
      setLoading(false)
    }
  }, [])

  // 服务端广播模板变化（导入 / 同步 / 增删改）时刷新列表
  useEffect(() => {
    return onWsEvent('templates.changed', () => {
      void reload({ silent: true })
    })
  }, [reload])

  // 断线重连后重新拉取模板列表（api/ws.ts 的约定：页面自己维护的数据不会自动恢复）
  useEffect(() => {
    return onWsStatus((status) => {
      if (status === 'open') void reload({ silent: true })
    })
  }, [reload])

  /* ── 搜索防抖 250ms（§4.5）─────────────────────────────────────────── */
  useEffect(() => {
    const timer = setTimeout(() => setSearchText(searchInput), 250)
    return () => clearTimeout(timer)
  }, [searchInput])

  /* ── 筛选 / 排序 / 分组 ────────────────────────────────────────────── */
  const visible = useMemo(() => {
    let list = templates
    if (searchText.trim()) {
      list = list.filter((template) => matchesTemplateSearch(template, searchText))
    }
    if (selectedCategory.trim()) {
      list = list.filter((template) => template.category === selectedCategory)
    }
    // 「只看本机可用」只保留服务端确认可用的模板
    if (onlyAvailable) list = list.filter((template) => template.available)

    const sorted = [...list]
    if (sortMode === 'name') {
      sorted.sort((a, b) => a.name.localeCompare(b.name, 'zh-Hans-CN'))
    } else if (sortMode === 'updated') {
      sorted.sort(
        (a, b) =>
          new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime() ||
          a.name.localeCompare(b.name, 'zh-Hans-CN'),
      )
    } else {
      sorted.sort(
        (a, b) =>
          Number(b.isRecommended) - Number(a.isRecommended) ||
          a.sortOrder - b.sortOrder ||
          a.name.localeCompare(b.name, 'zh-Hans-CN'),
      )
    }
    return sorted
  }, [templates, searchText, selectedCategory, onlyAvailable, sortMode])

  const groups = useMemo(() => {
    const map = new Map<string, TemplateListItem[]>()
    for (const template of visible) {
      const key = categoryDisplayName(template.category)
      const bucket = map.get(key)
      if (bucket) bucket.push(template)
      else map.set(key, [template])
    }
    return Array.from(map.entries())
  }, [visible])

  const systemCount = templates.filter((template) => template.type === 'System').length
  const userCount = templates.filter((template) => template.type === 'User').length

  const clearFilters = () => {
    setSearchInput('')
    setSearchText('')
    setSelectedCategory('')
    setOnlyAvailable(false)
  }

  /* ── 动作 ──────────────────────────────────────────────────────────── */
  const exportTemplates = async (filtered: boolean) => {
    if (exporting) return
    const list = filtered ? visible : templates
    if (list.length === 0) {
      toast.warning('没有可导出的模板。')
      return
    }
    setExporting(true)
    try {
      const result = await templatesApi.exportJson(list.map((template) => template.id))
      const fileName = result.fileName?.trim() ? result.fileName : exportFileName()
      if (downloadTextFile(fileName, result.json)) {
        toast.success(`已导出${filtered ? '当前筛选结果' : '全部模板'} ${list.length} 个到 ${fileName}`)
      } else {
        toast.error('导出失败：浏览器没有触发下载，请检查浏览器的下载权限或拦截设置。')
      }
    } catch (error) {
      toast.error(`导出失败：${getErrorMessage(error)}`)
    } finally {
      setExporting(false)
    }
  }

  /** 「选择 / 直接使用」→ `/convert?template={id}`（查询参数预选） */
  const applyTemplate = (template: TemplateListItem) => {
    setSelectedId(template.id)
    toast.success(`已打开转换页面，模板已预选：${template.name}`)
    try {
      navigate(`/convert?template=${encodeURIComponent(template.id)}`)
    } catch (error) {
      toast.error(`打开转换页面失败：${getErrorMessage(error)}`)
    }
  }

  const copyTemplate = async (template: TemplateListItem) => {
    try {
      const created = await templatesApi.copy(template.id, `${template.name} - 副本`)
      toast.success(`已复制为「${created?.name ?? template.name}」`)
      await reload({ silent: true })
    } catch (error) {
      toast.error(`复制模板失败：${getErrorMessage(error)}`)
    }
  }

  const openCreateEditor = () => {
    setEditorTemplate(null)
    setEditorIsSystem(false)
    setEditorOpen(true)
  }

  const openEditEditor = (template: TemplateListItem) => {
    setViewerOpen(false)
    setEditorTemplate(template)
    setEditorIsSystem(template.type === 'System')
    setEditorOpen(true)
  }

  const openViewer = (template: TemplateListItem) => {
    setViewing(template)
    setViewerOpen(true)
  }

  const confirmDelete = async () => {
    const target = deleteTarget
    setDeleteOpen(false)
    setDeleteTarget(null)
    if (!target) return

    if (target.type === 'System') {
      // 系统预设不能在这里删除：引导到「预设模板管理」，服务端也会返回 403
      toast.warning('系统预设模板不能在这里删除，已跳转到「预设模板管理」页面。')
      try {
        navigate('/templates-admin')
      } catch (error) {
        toast.error(`跳转失败，请手动打开 /templates-admin：${getErrorMessage(error)}`)
      }
      return
    }

    try {
      await templatesApi.remove(target.id)
      toast.success(`模板「${target.name}」已删除`)
      await reload({ silent: true })
    } catch (error) {
      toast.error(`删除模板失败：${getErrorMessage(error)}`)
    }
  }

  /* ── 渲染 ──────────────────────────────────────────────────────────── */
  const cardProps = {
    onSelect: applyTemplate,
    onView: openViewer,
    onEdit: openEditEditor,
    onDelete: (template: TemplateListItem) => {
      setDeleteTarget(template)
      setDeleteOpen(true)
    },
    onCopy: (template: TemplateListItem) => void copyTemplate(template),
  }

  return (
    <div className="ffui-container">
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="m-0">📋 模板管理</h1>
          <p className="mt-1 mb-0 text-[0.88rem] text-muted-text">
            浏览、筛选并预览模板；点「选择」会带着该模板打开转换页面。
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Badge>全部 {templates.length}</Badge>
          <Badge tone="info">系统预设 {systemCount}</Badge>
          <Badge tone="primary">我的模板 {userCount}</Badge>
        </div>
      </div>

      <div className="mb-4 flex flex-col gap-2">
        {syncSummary ? (
          <Alert tone={syncChanged ? 'success' : 'info'} role="status" icon={syncChanged ? '✅' : 'ℹ️'}>
            {syncSummary}（每次打开本页都会自动同步一次系统预设）
          </Alert>
        ) : null}

        <Alert tone={hardwareWarning ? 'warning' : 'info'} role="status" icon={hardwareWarning ? '⚠️' : '🚀'}>
          {hardwareText}
        </Alert>
      </div>

      <div className="mb-4 flex flex-wrap gap-2">
        <Button variant="primary" onClick={openCreateEditor}>
          ➕ 创建模板
        </Button>
        <Button
          loading={exporting}
          disabled={exporting || templates.length === 0}
          onClick={() => void exportTemplates(false)}
        >
          ⬇️ 导出全部
        </Button>
        <Button
          loading={exporting}
          disabled={exporting || visible.length === 0}
          onClick={() => void exportTemplates(true)}
        >
          ⬇️ 导出当前筛选结果（{visible.length}）
        </Button>
        <Button onClick={() => setImportOpen(true)}>📥 导入</Button>
      </div>

      <div className="ffui-panel mb-4 flex flex-col gap-3">
        <div className="flex flex-wrap items-end gap-3">
          <label className="flex min-w-[170px] flex-[1_1_260px] flex-col gap-[0.3rem]">
            <span className="text-[0.8rem] text-secondary-text">搜索</span>
            <input
              type="search"
              className="w-full"
              aria-label="搜索模板"
              placeholder="搜索名称、描述、分类、标签…"
              value={searchInput}
              onChange={(event) => setSearchInput(event.target.value)}
            />
          </label>

          <label className="flex min-w-[170px] flex-col gap-[0.3rem]">
            <span className="text-[0.8rem] text-secondary-text">排序</span>
            <select
              className="w-full"
              aria-label="排序方式"
              value={sortMode}
              onChange={(event) => setSortMode(event.target.value as SortMode)}
            >
              <option value="recommended">推荐优先</option>
              <option value="name">名称</option>
              <option value="updated">最近修改</option>
            </select>
          </label>

          <label className="inline-flex cursor-pointer items-center gap-[0.4rem] pb-[0.35rem] text-[0.9rem]">
            <input
              type="checkbox"
              className="w-auto"
              checked={onlyAvailable}
              onChange={(event) => setOnlyAvailable(event.target.checked)}
            />
            只看本机可用的模板
          </label>

          <div className="ml-auto inline-flex gap-1" role="group" aria-label="视图切换">
            <Button
              size="sm"
              variant={layout === 'flat' ? 'primary' : 'default'}
              aria-pressed={layout === 'flat'}
              aria-label="平铺显示"
              title="平铺显示"
              onClick={() => setLayout('flat')}
            >
              ▦ 平铺
            </Button>
            <Button
              size="sm"
              variant={layout === 'group' ? 'primary' : 'default'}
              aria-pressed={layout === 'group'}
              aria-label="按分类分组显示"
              title="按分类分组显示"
              onClick={() => setLayout('group')}
            >
              ☰ 分组
            </Button>
            <Button
              size="sm"
              variant={layout === 'list' ? 'primary' : 'default'}
              aria-pressed={layout === 'list'}
              aria-label="列表显示"
              title="列表显示（额外显示命令参数）"
              onClick={() => setLayout('list')}
            >
              ▤ 列表
            </Button>
          </div>
        </div>

        {/* 分类筛选：chip 形式，计数来自 /api/templates/categories */}
        <div className="flex flex-wrap items-center gap-[0.4rem]" role="group" aria-label="模板分类筛选">
          <Button
            size="sm"
            variant={selectedCategory === '' ? 'primary' : 'default'}
            aria-pressed={selectedCategory === ''}
            aria-label="显示全部分类的模板"
            onClick={() => setSelectedCategory('')}
          >
            全部 {templates.length}
          </Button>
          {categories.map((category) => (
            <Button
              key={category.name}
              size="sm"
              variant={selectedCategory === category.name ? 'primary' : 'default'}
              aria-pressed={selectedCategory === category.name}
              aria-label={`筛选分类：${category.name}`}
              onClick={() => setSelectedCategory(category.name)}
            >
              {category.name} {category.count}
            </Button>
          ))}
        </div>

        <p className="m-0 text-[0.88rem] text-secondary-text" aria-live="polite">
          {loading ? '正在加载模板…' : `${visible.length} 个模板符合当前条件（共 ${templates.length} 个）`}
        </p>
      </div>

      {loading ? (
        <div className="ffui-loading-row" aria-busy="true">
          <Spinner />
          <span>正在加载模板…</span>
        </div>
      ) : visible.length === 0 ? (
        <div className="ffui-panel ffui-empty-state">
          {templates.length === 0 ? (
            <>
              <p>还没有任何模板，先创建一个吧。</p>
              <Button variant="primary" onClick={openCreateEditor}>
                创建第一个模板
              </Button>
            </>
          ) : (
            <>
              <p>没有找到匹配的模板。</p>
              <Button onClick={clearFilters}>清除筛选条件</Button>
            </>
          )}
        </div>
      ) : layout === 'group' ? (
        groups.map(([category, items]) => (
          <section key={category} className="mb-6">
            <h2 className="ffui-section-title">
              {category} <Badge>{items.length}</Badge>
            </h2>
            <div className="grid gap-3 [grid-template-columns:repeat(auto-fill,minmax(300px,1fr))]">
              {items.map((template) => (
                <TemplateCardView
                  key={template.id}
                  template={template}
                  isSelected={selectedId === template.id}
                  {...cardProps}
                />
              ))}
            </div>
          </section>
        ))
      ) : layout === 'list' ? (
        <div className="flex flex-col gap-3">
          {visible.map((template) => (
            <TemplateCardView
              key={template.id}
              template={template}
              viewMode="list"
              isSelected={selectedId === template.id}
              {...cardProps}
            />
          ))}
        </div>
      ) : (
        <div className="grid gap-3 [grid-template-columns:repeat(auto-fill,minmax(300px,1fr))]">
          {visible.map((template) => (
            <TemplateCardView
              key={template.id}
              template={template}
              isSelected={selectedId === template.id}
              {...cardProps}
            />
          ))}
        </div>
      )}

      {/* 详情弹窗复用兄弟任务的 TemplateDetailViewer（§3.8 全部区块 + 服务端命令预览） */}
      <TemplateDetailViewer
        template={viewing}
        open={viewerOpen}
        onOpenChange={setViewerOpen}
        onCopy={(template) => void copyTemplate(template as TemplateListItem)}
        onEdit={(template) => openEditEditor(template as TemplateListItem)}
        onUse={(template) => applyTemplate(template as TemplateListItem)}
      />

      <TemplateEditor
        template={editorTemplate}
        open={editorOpen}
        onOpenChange={setEditorOpen}
        isSystem={editorIsSystem}
        onSaved={(_saved, meta) => {
          // 「模板「{名称}」已保存」由编辑器自己 Toast（避免重复弹出）
          if (meta.closeEditor) setEditorOpen(false)
          void reload({ silent: true })
        }}
      />

      <TemplateImportDialog
        open={importOpen}
        onOpenChange={setImportOpen}
        asSystemLabel="导入为系统模板（高级选项）"
        asSystemWarning="导入为系统模板后，这些模板会进入「系统预设」分组，并可能在后续的系统预设同步或重置时被内置预设覆盖、删除。只在明确知道用途时启用。"
        onImported={(result, asSystem) => {
          toast.success(
            asSystem ? `已导入 ${result.imported} 个模板（作为系统预设）` : `已导入 ${result.imported} 个模板`,
          )
          void reload({ silent: true })
        }}
      />

      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        danger
        title={deleteTarget?.type === 'System' ? '删除系统预设模板' : '删除模板'}
        message={
          deleteTarget?.type === 'System'
            ? `「${deleteTarget?.name ?? ''}」是系统预设模板，无法在这里删除。`
            : `确定要删除模板「${deleteTarget?.name ?? ''}」吗？此操作不可撤销。`
        }
        detail={
          deleteTarget?.type === 'System'
            ? '删除系统预设请前往「预设模板管理」页面；即使在那里删除，只要它仍然存在于内置预设列表中，下次系统预设同步（应用升级或重置）时它还会重新出现。'
            : '删除只影响模板本身，已经用它执行过的任务记录不会受影响。'
        }
        confirmText={deleteTarget?.type === 'System' ? '前往管理页面' : '删除'}
        onConfirm={() => void confirmDelete()}
        onCancel={() => setDeleteTarget(null)}
      />
    </div>
  )
}
