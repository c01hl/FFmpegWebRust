/**
 * 预设模板管理页 `/templates-admin`（也挂在 `/templates/admin`，对应 ui-spec §4.6）。
 *
 * 与模板管理页的区别：
 * - **可以**修改与删除系统预设（模板管理页只能引导过来）；
 * - 表格视图（可点表头排序）+ 运行诊断面板 + 危险区域（重置系统预设）；
 * - 导入弹窗的复选框文案是 `导入为系统模板`（模板管理页是 `导入为系统模板（高级选项）`）。
 *
 * 服务端消息一律逐字展示：删除失败（例如系统模板返回 403）的 `error.message` 原样放进
 * 页面顶部的 `role="alert"` 告警条与 Toast 里，前端不编造任何文案。
 */

import { useCallback, useEffect, useMemo, useState } from 'react'
import type {
  CommandTemplate,
  QueueStatusDto,
  SystemPathsDto,
  AppStatusDto,
  TemplateListItem,
} from '../api/types'
import { getErrorMessage } from '../api/client'
import { historyApi, systemApi, templatesApi } from '../api/endpoints'
import { onWsEvent, onWsStatus } from '../api/ws'
import {
  categoryDisplayName,
  downloadTextFile,
  exportFileName,
  matchesTemplateSearch,
  templateEncoderColumnText,
} from '../lib/templateCatalog'
import { formatDateTimeMinute } from '../lib/format'
import { PAGE_TITLES, useDocumentTitle } from '../lib/title'
import { toast } from '../store/toast'
import { TemplateDetailViewer } from '../components/template/TemplateDetailViewer'
import { TemplateEditor } from '../components/template/TemplateEditor'
import { TemplateImportDialog } from '../components/template/TemplateImportDialog'
import { Alert } from '../components/ui/Alert'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { ConfirmDialog } from '../components/ui/ConfirmDialog'
import { Spinner } from '../components/ui/Spinner'

type TypeFilter = 'all' | 'system' | 'user'
type SortColumn = 'name' | 'category' | 'type' | 'extension' | 'encoder' | 'sort' | 'updated'

/** 各列首次点击时的默认方向（§4.6：名称/分类/输出格式/编码器升序，类型/排序/更新时间降序） */
const ASCENDING_FIRST: Readonly<Record<SortColumn, boolean>> = {
  name: true,
  category: true,
  extension: true,
  encoder: true,
  type: false,
  sort: false,
  updated: false,
}

const SORT_INDICATOR: Readonly<Record<'none' | 'ascending' | 'descending', string>> = {
  none: '↕',
  ascending: '▲',
  descending: '▼',
}

/** 按列取排序键（返回值统一为字符串或数字，交给比较器比较） */
function sortKey(template: CommandTemplate, column: SortColumn): string | number {
  switch (column) {
    case 'name':
      return template.name ?? ''
    case 'category':
      return template.category ?? ''
    case 'type':
      return template.type === 'System' ? 0 : 1
    case 'extension':
      return (template.outputExtension ?? '').toLowerCase()
    case 'encoder':
      return templateEncoderColumnText(template)
    case 'updated':
      return template.updatedAt ? new Date(template.updatedAt).getTime() : 0
    case 'sort':
    default:
      return template.sortOrder
  }
}

export function TemplatesAdminPage() {
  useDocumentTitle(PAGE_TITLES.templatesAdmin)

  const [templates, setTemplates] = useState<TemplateListItem[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [statusMessage, setStatusMessage] = useState('')
  const [serverError, setServerError] = useState('')

  const [searchInput, setSearchInput] = useState('')
  const [typeFilter, setTypeFilter] = useState<TypeFilter>('all')
  const [sortColumn, setSortColumn] = useState<SortColumn>('sort')
  const [sortAscending, setSortAscending] = useState(true)

  const [presetVersion, setPresetVersion] = useState<number | null>(null)
  const [paths, setPaths] = useState<SystemPathsDto | null>(null)
  const [status, setStatus] = useState<AppStatusDto | null>(null)
  const [queue, setQueue] = useState<QueueStatusDto | null>(null)
  const [diagnosticsError, setDiagnosticsError] = useState('')

  const [cleanupDays, setCleanupDays] = useState(30)
  const [cleaning, setCleaning] = useState(false)

  const [exporting, setExporting] = useState(false)
  const [importOpen, setImportOpen] = useState(false)

  const [editorOpen, setEditorOpen] = useState(false)
  const [editorTemplate, setEditorTemplate] = useState<TemplateListItem | null>(null)
  const [editorIsSystem, setEditorIsSystem] = useState(true)

  const [viewing, setViewing] = useState<TemplateListItem | null>(null)
  const [viewerOpen, setViewerOpen] = useState(false)

  const [deleteTarget, setDeleteTarget] = useState<TemplateListItem | null>(null)
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [deleteBusy, setDeleteBusy] = useState(false)

  const [resetOpen, setResetOpen] = useState(false)
  const [resetBusy, setResetBusy] = useState(false)

  /* ── 初始化：模板列表 + 诊断（预设版本来自一次同步） ─────────────────── */
  useEffect(() => {
    let cancelled = false
    const run = async () => {
      try {
        const sync = await templatesApi.syncSystem()
        if (!cancelled) {
          setPresetVersion(sync.presetVersion)
          if (sync.summary?.trim()) setStatusMessage(sync.summary)
        }
      } catch {
        // 同步失败不阻塞页面；模板列表仍然可用
      }

      try {
        const list = await templatesApi.list({ includeSystem: true })
        if (!cancelled) setTemplates(list)
      } catch (error) {
        if (!cancelled) toast.error(`加载模板失败：${getErrorMessage(error)}`)
      } finally {
        if (!cancelled) setLoading(false)
      }

      await loadDiagnosticsInto(setPaths, setStatus, setQueue, setDiagnosticsError)
    }
    void run()
    return () => {
      cancelled = true
    }
  }, [])

  const reloadTemplates = useCallback(async () => {
    try {
      const list = await templatesApi.list({ includeSystem: true })
      setTemplates(list)
    } catch (error) {
      toast.error(`加载模板失败：${getErrorMessage(error)}`)
    }
  }, [])

  const refreshDiagnostics = useCallback(async () => {
    await loadDiagnosticsInto(setPaths, setStatus, setQueue, setDiagnosticsError)
  }, [])

  // 服务端广播模板变化（导入 / 同步 / 增删改）时刷新列表
  useEffect(() => {
    return onWsEvent('templates.changed', () => {
      void reloadTemplates()
    })
  }, [reloadTemplates])

  // 断线重连后重新拉取列表与诊断
  useEffect(() => {
    return onWsStatus((status) => {
      if (status !== 'open') return
      void reloadTemplates()
      void refreshDiagnostics()
    })
  }, [reloadTemplates, refreshDiagnostics])

  /* ── 筛选 / 排序（§4.6）──────────────────────────────────────────────── */
  const visible = useMemo(() => {
    let list = templates
    if (typeFilter === 'system') list = list.filter((template) => template.type === 'System')
    else if (typeFilter === 'user') list = list.filter((template) => template.type === 'User')
    if (searchInput.trim()) {
      list = list.filter((template) =>
        matchesTemplateSearch(template, searchInput, [templateEncoderColumnText(template)]),
      )
    }

    const sorted = [...list]
    sorted.sort((a, b) => {
      const left = sortKey(a, sortColumn)
      const right = sortKey(b, sortColumn)
      let comparison = 0
      if (typeof left === 'number' && typeof right === 'number') comparison = left - right
      else comparison = String(left).localeCompare(String(right), 'zh-Hans-CN')
      if (comparison === 0) return (a.name ?? '').localeCompare(b.name ?? '', 'zh-Hans-CN')
      return sortAscending ? comparison : -comparison
    })
    return sorted
  }, [templates, typeFilter, searchInput, sortColumn, sortAscending])

  const systemCount = templates.filter((template) => template.type === 'System').length
  const userCount = templates.filter((template) => template.type === 'User').length
  const hasFilters = searchInput.trim().length > 0 || typeFilter !== 'all'
  const isBusy = busy || exporting || deleteBusy || resetBusy

  const toggleSort = (column: SortColumn) => {
    if (column === sortColumn) {
      setSortAscending((current) => !current)
      return
    }
    setSortColumn(column)
    setSortAscending(ASCENDING_FIRST[column])
  }

  const ariaSort = (column: SortColumn): 'none' | 'ascending' | 'descending' => {
    if (column !== sortColumn) return 'none'
    return sortAscending ? 'ascending' : 'descending'
  }

  /* ── 动作 ──────────────────────────────────────────────────────────── */
  const openCreateEditor = () => {
    setEditorTemplate(null)
    setEditorIsSystem(true)
    setEditorOpen(true)
  }

  const openEditEditor = (template: TemplateListItem) => {
    setViewerOpen(false)
    setEditorTemplate(template)
    setEditorIsSystem(template.type === 'System')
    setEditorOpen(true)
  }

  const nextSortOrder = useMemo(() => {
    const max = templates.reduce((value, template) => Math.max(value, template.sortOrder ?? 0), 0)
    return templates.length === 0 ? 10 : max + 10
  }, [templates])

  const exportAll = async () => {
    if (exporting || templates.length === 0) {
      if (templates.length === 0) toast.warning('没有可导出的模板。')
      return
    }
    setExporting(true)
    try {
      const result = await templatesApi.exportJson(templates.map((template) => template.id))
      const fileName = result.fileName?.trim() ? result.fileName : exportFileName()
      if (downloadTextFile(fileName, result.json)) {
        toast.success(`已导出 ${templates.length} 个模板到 ${fileName}`)
      } else {
        toast.error('导出失败：浏览器没有触发下载，请检查浏览器的下载权限或拦截设置。')
      }
    } catch (error) {
      toast.error(`导出失败：${getErrorMessage(error)}`)
    } finally {
      setExporting(false)
    }
  }

  const copyTemplate = async (template: TemplateListItem) => {
    try {
      const created = await templatesApi.copy(template.id, `${template.name} - 副本`)
      toast.success(`已复制为「${created?.name ?? template.name}」`)
      await reloadTemplates()
    } catch (error) {
      toast.error(`复制模板失败：${getErrorMessage(error)}`)
    }
  }

  const confirmDelete = async () => {
    const target = deleteTarget
    if (!target) {
      setDeleteOpen(false)
      return
    }
    setDeleteBusy(true)
    setServerError('')
    try {
      await templatesApi.remove(target.id)
      toast.success(
        target.type === 'System'
          ? `已删除系统预设「${target.name}」；只要它仍在内置预设列表中，下次预设同步或重置时就会重新出现。`
          : `已删除用户模板「${target.name}」`,
      )
      setDeleteOpen(false)
      setDeleteTarget(null)
      await reloadTemplates()
    } catch (error) {
      // 服务端消息（例如系统预设的 403）逐字展示，不加工
      const message = getErrorMessage(error)
      setServerError(message)
      toast.error(`删除模板失败：${message}`)
      // 关闭确认框，让页面顶部的告警条（原始消息）露出来
      setDeleteOpen(false)
      setDeleteTarget(null)
    } finally {
      setDeleteBusy(false)
    }
  }

  const resetSystemTemplates = async () => {
    setResetBusy(true)
    setServerError('')
    try {
      const result = await templatesApi.resetSystem()
      setPresetVersion(result.presetVersion)
      setStatusMessage(result.summary?.trim() ? result.summary : '')
      toast.success(`系统预设模板已重置到 v${result.presetVersion}（用户自建模板未受影响）`)
      setResetOpen(false)
      await reloadTemplates()
      await refreshDiagnostics()
    } catch (error) {
      toast.error(`重置系统预设失败：${getErrorMessage(error)}`)
    } finally {
      setResetBusy(false)
    }
  }

  const syncSystemTemplates = async () => {
    setBusy(true)
    setServerError('')
    try {
      const result = await templatesApi.syncSystem()
      setPresetVersion(result.presetVersion)
      const summary = result.summary?.trim()
        ? result.summary
        : `新增 ${result.added} · 更新 ${result.updated} · 未变化 ${result.unchanged}（预设 v${result.presetVersion}）`
      setStatusMessage(summary)
      toast.success(summary)
      await reloadTemplates()
      await refreshDiagnostics()
    } catch (error) {
      toast.error(`同步系统预设失败：${getErrorMessage(error)}`)
    } finally {
      setBusy(false)
    }
  }

  const cleanupHistory = async () => {
    if (cleaning) return
    setCleaning(true)
    try {
      const days = Math.min(3650, Math.max(1, Math.trunc(cleanupDays) || 30))
      const result = await historyApi.cleanup(days)
      toast.success(
        result.deletedTasks > 0
          ? `已清理 ${result.deletedTasks} 条早于 ${days} 天的历史任务记录（运行中/等待中的任务不受影响）`
          : `没有早于 ${days} 天的历史任务记录需要清理`,
      )
      await refreshDiagnostics()
    } catch (error) {
      toast.error(`清理历史任务失败：${getErrorMessage(error)}`)
    } finally {
      setCleaning(false)
    }
  }

  const cardProps = {
    onView: (template: TemplateListItem) => {
      setViewing(template)
      setViewerOpen(true)
    },
    onEdit: openEditEditor,
    onDelete: (template: TemplateListItem) => {
      setServerError('')
      setDeleteTarget(template)
      setDeleteOpen(true)
    },
    onCopy: (template: TemplateListItem) => void copyTemplate(template),
  }

  return (
    <div className="ffui-container">
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="m-0">🔧 预设模板管理</h1>
          <p className="mt-1 mb-0 text-[0.88rem] text-muted-text">
            系统预设模板的完整增删改查、预设重置与运行诊断。
          </p>
        </div>
        <Badge tone="warning">⚠️ 高级页面</Badge>
      </div>

      <Alert tone="warning" role="alert" className="mb-4">
        <p className="m-0 mb-[0.35rem]">
          这个页面可以直接修改<strong>系统预设模板</strong>，改动会影响「转换」与「批量处理」里的选项。
        </p>
        <p className="m-0">
          应用默认只监听 127.0.0.1（仅本机可访问），因此没有额外的登录校验；如果把服务暴露到局域网，任何人都可以修改这里的模板。
        </p>
      </Alert>

      {/* 服务端消息逐字展示（删除 403 / 同步摘要等） */}
      {serverError ? (
        <Alert tone="danger" role="alert" className="mb-4">
          {serverError}
        </Alert>
      ) : null}
      {statusMessage ? (
        <Alert tone="info" role="status" className="mb-4" aria-live="polite">
          {statusMessage}
        </Alert>
      ) : null}

      <div className="mb-4 flex flex-wrap gap-2">
        <Button variant="primary" disabled={isBusy} onClick={openCreateEditor}>
          ➕ 新建
        </Button>
        <Button loading={exporting} disabled={isBusy} onClick={() => void exportAll()}>
          ⬇️ 导出全部
        </Button>
        <Button disabled={isBusy} onClick={() => setImportOpen(true)}>
          📥 导入
        </Button>
        <Button
          disabled={isBusy}
          onClick={() => {
            void reloadTemplates()
            void refreshDiagnostics()
          }}
        >
          🔄 刷新
        </Button>
        <Button disabled={isBusy} onClick={() => void syncSystemTemplates()}>
          🔄 同步系统模板
        </Button>
      </div>

      <div className="ffui-panel mb-4 flex flex-col gap-3">
        <div className="flex flex-wrap items-end gap-3">
          <label className="flex min-w-[170px] flex-[1_1_260px] flex-col gap-[0.3rem]">
            <span className="text-[0.8rem] text-secondary-text">搜索</span>
            <input
              type="search"
              className="w-full"
              aria-label="搜索模板"
              placeholder="搜索名称、描述、分类、编码器…"
              value={searchInput}
              onChange={(event) => setSearchInput(event.target.value)}
            />
          </label>

          <label className="flex min-w-[170px] flex-col gap-[0.3rem]">
            <span className="text-[0.8rem] text-secondary-text">类型</span>
            <select
              className="w-full"
              aria-label="按类型筛选"
              value={typeFilter}
              onChange={(event) => setTypeFilter(event.target.value as TypeFilter)}
            >
              <option value="all">全部</option>
              <option value="system">系统</option>
              <option value="user">用户</option>
            </select>
          </label>

          <Button
            disabled={!hasFilters}
            onClick={() => {
              setSearchInput('')
              setTypeFilter('all')
            }}
          >
            清除筛选
          </Button>
        </div>

        <p className="m-0 text-[0.88rem] text-secondary-text" aria-live="polite">
          显示 {visible.length} 个模板（共 {templates.length} 个：系统 {systemCount} · 用户 {userCount}）
        </p>
      </div>

      <div className="ffui-panel mb-4">
        <h2 className="ffui-panel-title">模板列表</h2>

        {loading ? (
          <div className="ffui-loading-row" aria-busy="true">
            <Spinner />
            <span>正在加载模板…</span>
          </div>
        ) : templates.length === 0 ? (
          <div className="ffui-empty-state">
            <p>数据库里还没有任何模板。可以点「新建」创建一个，或点「导入」导入 JSON。</p>
          </div>
        ) : visible.length === 0 ? (
          <div className="ffui-empty-state">
            <p>没有符合当前筛选条件的模板。</p>
            <Button
              onClick={() => {
                setSearchInput('')
                setTypeFilter('all')
              }}
            >
              清除筛选
            </Button>
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full border-collapse text-[0.9rem]">
              <thead>
                <tr>
                  {(
                    [
                      ['name', '名称'],
                      ['category', '分类'],
                      ['type', '类型'],
                      ['extension', '输出格式'],
                      ['encoder', '编码器'],
                      ['sort', '排序'],
                      ['updated', '更新时间'],
                    ] as const
                  ).map(([column, label]) => (
                    <th
                      key={column}
                      scope="col"
                      aria-sort={ariaSort(column)}
                      className="border-b border-line px-2 py-1 text-left"
                    >
                      <button
                        type="button"
                        className="cursor-pointer border-0 bg-transparent p-0 font-medium text-inherit"
                        onClick={() => toggleSort(column)}
                      >
                        {label} {SORT_INDICATOR[ariaSort(column)]}
                      </button>
                    </th>
                  ))}
                  <th scope="col" className="border-b border-line px-2 py-1 text-left">
                    操作
                  </th>
                </tr>
              </thead>
              <tbody>
                {visible.map((template) => (
                  <tr key={template.id}>
                    <td className="border-b border-line px-2 py-1">
                      <span className="font-medium">{template.name}</span>{' '}
                      {template.isRecommended ? <Badge tone="primary">推荐</Badge> : null}
                    </td>
                    <td className="border-b border-line px-2 py-1">
                      {categoryDisplayName(template.category)}
                    </td>
                    <td className="border-b border-line px-2 py-1">
                      <Badge tone={template.type === 'System' ? 'info' : 'success'}>
                        {template.type === 'System' ? '系统' : '用户'}
                      </Badge>
                    </td>
                    <td className="mono border-b border-line px-2 py-1">
                      {template.outputExtension ? `.${template.outputExtension}` : '-'}
                    </td>
                    <td className="border-b border-line px-2 py-1">{templateEncoderColumnText(template)}</td>
                    <td className="border-b border-line px-2 py-1 [font-variant-numeric:tabular-nums]">
                      {template.sortOrder}
                    </td>
                    <td className="border-b border-line px-2 py-1">
                      {formatDateTimeMinute(template.updatedAt)}
                    </td>
                    <td className="border-b border-line px-2 py-1">
                      <div className="flex flex-wrap gap-2">
                        <Button size="sm" disabled={isBusy} onClick={() => openEditEditor(template)}>
                          ✏️ 编辑
                        </Button>
                        <Button
                          size="sm"
                          disabled={isBusy}
                          onClick={() => void copyTemplate(template)}
                          aria-label={`复制模板 ${template.name}`}
                        >
                          📑 复制
                        </Button>
                        <Button
                          size="sm"
                          variant="danger"
                          disabled={isBusy}
                          onClick={() => cardProps.onDelete(template)}
                          aria-label={`删除模板 ${template.name}`}
                        >
                          🗑️ 删除
                        </Button>
                        <Button size="sm" disabled={isBusy} onClick={() => cardProps.onView(template)}>
                          🔍 详情
                        </Button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* 运行诊断 */}
      <div className="ffui-panel mb-4">
        <h2 className="ffui-panel-title">运行诊断</h2>
        {diagnosticsError ? (
          <Alert tone="warning" role="status" className="mb-2">
            读取运行状态失败：{diagnosticsError}
          </Alert>
        ) : null}
        <dl className="m-0 grid gap-x-4 gap-y-2 [grid-template-columns:auto_1fr]">
          <dt className="text-muted-text">数据目录</dt>
          <dd className="m-0 flex flex-wrap items-center gap-2">
            <span className="mono">{paths?.dataDirectory ?? '—'}</span>
            {paths ? (
              <Badge tone={paths.dataDirectoryWritable ? 'success' : 'danger'}>
                {paths.dataDirectoryWritable ? '可写' : '不可写'}
              </Badge>
            ) : null}
          </dd>

          <dt className="text-muted-text">模板数量</dt>
          <dd className="m-0">
            {templates.length} 个（系统 {systemCount} · 用户 {userCount}）
          </dd>

          <dt className="text-muted-text">任务队列</dt>
          <dd className="m-0">
            运行中 {queue?.active ?? 0} · 等待中 {queue?.queued ?? 0} · 历史记录 {status?.taskCount ?? 0}
          </dd>

          <dt className="text-muted-text">历史清理</dt>
          <dd className="m-0 flex flex-wrap items-center gap-2">
            <label className="inline-flex items-center gap-[0.4rem]">
              保留最近
              <input
                type="number"
                className="w-[5.5rem]"
                min={1}
                max={3650}
                aria-label="历史保留天数"
                value={cleanupDays}
                onChange={(event) => setCleanupDays(Number(event.target.value))}
              />
              天
            </label>
            <Button
              size="sm"
              loading={cleaning}
              disabled={cleaning}
              onClick={() => void cleanupHistory()}
            >
              🧹 清理历史
            </Button>
          </dd>

          <dt className="text-muted-text">FFmpeg</dt>
          <dd className="m-0">
            {status === null ? (
              <span>查询中…</span>
            ) : status.ffmpegAvailable ? (
              <span>{status.ffmpegVersion}</span>
            ) : (
              <Badge tone="danger">不可用</Badge>
            )}
            <div className="mono">{status?.ffmpegPath ?? ''}</div>
          </dd>

          <dt className="text-muted-text">预设版本</dt>
          <dd className="m-0">{presetVersion === null ? '—' : `v${presetVersion}`}</dd>
        </dl>
      </div>

      {/* 危险区域 */}
      <details className="mb-4 rounded-md border border-danger bg-surface p-3">
        <summary className="cursor-pointer font-medium text-danger">⚠️ 危险区域</summary>
        <div className="mt-2 flex flex-wrap items-start justify-between gap-3">
          <div>
            <h3 className="m-0 mb-1">重置系统预设模板</h3>
            <p className="m-0 text-[0.88rem] text-secondary-text">
              删除当前数据库里的<strong>全部系统预设模板</strong>，然后按内置预设
              （v{presetVersion ?? '—'}）重新写入。<strong>用户自己创建的模板会保留</strong>
              ，但对系统预设做过的所有手工修改都会丢失。
            </p>
          </div>
          <Button variant="danger" disabled={isBusy} onClick={() => setResetOpen(true)}>
            🔄 重置系统模板
          </Button>
        </div>
      </details>

      <TemplateEditor
        template={editorTemplate}
        open={editorOpen}
        onOpenChange={setEditorOpen}
        isSystem={editorIsSystem}
        sortOrderHint={nextSortOrder}
        onSaved={(_saved, meta) => {
          // 「模板「{名称}」已保存」由编辑器自己 Toast（避免重复弹出）
          if (meta.closeEditor) setEditorOpen(false)
          void reloadTemplates()
        }}
      />

      <TemplateDetailViewer
        template={viewing}
        open={viewerOpen}
        onOpenChange={setViewerOpen}
        onCopy={(template) => void copyTemplate(template as TemplateListItem)}
        onEdit={(template) => openEditEditor(template as TemplateListItem)}
      />

      <TemplateImportDialog
        open={importOpen}
        onOpenChange={setImportOpen}
        asSystemLabel="导入为系统模板"
        asSystemWarning="导入为系统模板后，这些模板会被标记成系统预设，并可能在后续的系统预设同步或重置时被内置预设覆盖、删除。只在明确知道用途时启用。"
        onImported={(result, asSystem) => {
          toast.success(
            asSystem
              ? `已导入 ${result.imported} 个模板（作为系统预设）`
              : `已导入 ${result.imported} 个用户模板`,
          )
          void reloadTemplates()
        }}
      />

      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        danger
        title="删除模板"
        message={`确定要删除「${deleteTarget?.name ?? ''}」吗？`}
        detail={
          deleteTarget?.type === 'System'
            ? '这是系统预设模板：删除后，只要它仍然存在于内置预设列表中，下次系统预设同步（应用升级或重置）时它还会重新出现。'
            : '这是用户自建模板：删除只影响模板本身，已经用它执行过的任务记录不受影响。'
        }
        confirmText="删除"
        busy={deleteBusy}
        onConfirm={() => void confirmDelete()}
        onCancel={() => {
          if (!deleteBusy) setDeleteTarget(null)
        }}
      />

      <ConfirmDialog
        open={resetOpen}
        onOpenChange={setResetOpen}
        danger
        title="重置系统预设模板"
        message="确定要删除所有系统预设模板并重新写入内置预设吗？"
        detail="用户自己创建的模板会被保留；对系统预设模板做过的所有手工修改都会丢失，且无法撤销。"
        confirmText="确认重置"
        busy={resetBusy}
        onConfirm={() => void resetSystemTemplates()}
      />
    </div>
  )
}

/** 拉取诊断面板需要的三份数据（失败只记一条中文提示，不阻塞页面） */
async function loadDiagnosticsInto(
  setPaths: (value: SystemPathsDto | null) => void,
  setStatus: (value: AppStatusDto | null) => void,
  setQueue: (value: QueueStatusDto | null) => void,
  setError: (value: string) => void,
): Promise<void> {
  const results = await Promise.allSettled([systemApi.paths(), systemApi.status(), systemApi.queue()])
  const [pathsResult, statusResult, queueResult] = results
  if (pathsResult.status === 'fulfilled') setPaths(pathsResult.value)
  if (statusResult.status === 'fulfilled') setStatus(statusResult.value)
  if (queueResult.status === 'fulfilled') setQueue(queueResult.value)
  const failed = results.find((result) => result.status === 'rejected')
  setError(failed && failed.status === 'rejected' ? getErrorMessage(failed.reason) : '')
}
