/**
 * 批量处理页 `/batch`（对应 ui-spec §4.3 `BatchProcess.razor` + §7 批量专题）。
 *
 * 关键行为：
 * - 文件/目录添加全部走服务端（原生对话框、`POST /api/media/scan`、`POST /api/files/check`）；
 * - 开始批量后**执行完全交给后台队列**，页面只负责「每 1 秒轮询 + WebSocket 实时事件」展示；
 * - 批次/任务数据来自 `GET /api/batches/{id}` 与 `GET /api/batches/{id}/tasks`；
 * - 单文件日志不在列表里（§7.3），但页面提供「任务日志」区块按需查看（有界渲染 + 自动滚动）。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { getErrorMessage } from '../api/client'
import { batchesApi, settingsApi, tasksApi } from '../api/endpoints'
import type { BatchDto, SettingsDto, TaskDto, TaskStatus } from '../api/types'
import { isTerminalStatus } from '../api/types'
import { onWsEvent } from '../api/ws'
import { PAGE_TITLES, useDocumentTitle } from '../lib/title'
import { toast } from '../store/toast'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Spinner } from '../components/ui/Spinner'
import { BatchFileList } from '../components/batch/BatchFileList'
import { BatchProgressPanel } from '../components/batch/BatchProgressPanel'
import { TaskLogViewer } from '../components/convert/TaskLogViewer'
import { FileSelector } from '../components/files/FileSelector'
import { revealDirectory } from '../components/files/revealDirectory'
import { TemplateParameterEditor } from '../components/template/TemplateParameterEditor'
import { TemplateSelector } from '../components/template/TemplateSelector'
import type { CommandTemplate } from '../api/types'

/** 批次刷新周期（规格：每 1 秒） */
export const BATCH_POLL_INTERVAL_MS = 1000

/** 文件名（兼容 Windows 分隔符） */
function fileNameOf(path: string): string {
  return path.replace(/\\/g, '/').split('/').pop() || path
}

/** 目录（兼容 Windows 分隔符） */
function directoryOf(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/')
  parts.pop()
  return parts.join('/')
}

/** 参数默认值 */
function defaultParameterValues(template: CommandTemplate | null): Record<string, string> {
  const result: Record<string, string> = {}
  for (const parameter of template?.parameters ?? []) {
    if ((parameter.name ?? '').trim() === '') continue
    result[parameter.name] = parameter.defaultValue ?? ''
  }
  return result
}

export function BatchPage() {
  useDocumentTitle(PAGE_TITLES.batch)

  const [settings, setSettings] = useState<SettingsDto | null>(null)
  const [files, setFiles] = useState<string[]>([])
  const [fileStatuses, setFileStatuses] = useState<Record<string, TaskStatus>>({})
  const [outputDirectory, setOutputDirectory] = useState('')
  const [template, setTemplate] = useState<CommandTemplate | null>(null)
  const [parameterValues, setParameterValues] = useState<Record<string, string>>({})

  const [batch, setBatch] = useState<BatchDto | null>(null)
  const [batchTasks, setBatchTasks] = useState<TaskDto[]>([])
  const [isProcessing, setIsProcessing] = useState(false)
  const [logTaskId, setLogTaskId] = useState<string>('')

  const mounted = useRef(true)
  const polling = useRef(false)
  const wasProcessing = useRef(false)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  // 首屏：设置（默认输出目录 + 并发数）
  useEffect(() => {
    void (async () => {
      try {
        const result = await settingsApi.get()
        if (!mounted.current) return
        setSettings(result)
        setOutputDirectory((current) => current || result.defaultOutputDirectory || '')
      } catch (error) {
        if (mounted.current) toast.error(`读取设置失败：${getErrorMessage(error)}`)
      }
    })()
  }, [])

  /** 刷新批次：读批次状态 + 任务列表，并按「全部结束」判定收尾（§4.3） */
  const refreshBatch = useCallback(
    async (batchId: string, options: { silent?: boolean } = {}) => {
      const { silent = false } = options
      try {
        const [detail, tasks] = await Promise.all([
          batchesApi.get(batchId),
          batchesApi.tasks(batchId),
        ])
        if (!mounted.current) return

        setBatch(detail)
        const ordered = [...(tasks ?? [])].sort(
          (left, right) => Date.parse(left.createdAt) - Date.parse(right.createdAt),
        )
        setBatchTasks(ordered)

        const statuses: Record<string, TaskStatus> = {}
        for (const task of ordered) statuses[task.inputPath] = task.status
        setFileStatuses(statuses)

        const finished =
          ordered.length > 0 && ordered.every((task) => isTerminalStatus(task.status))
        if (finished && wasProcessing.current) {
          wasProcessing.current = false
          setIsProcessing(false)

          const ok = ordered.filter((task) => task.status === 'Completed').length
          const bad = ordered.filter(
            (task) => task.status === 'Failed' || task.status === 'Cancelled',
          ).length
          if (bad === 0) toast.success(`✅ 批量处理完成，成功 ${ok} 个文件`)
          else toast.warning(`批量处理完成：成功 ${ok} 个，失败 ${bad} 个`)
        }
      } catch (error) {
        // 定时轮询期间静默；只有显式刷新才提示（§4.3）
        if (!silent && mounted.current) toast.error(`刷新批次失败：${getErrorMessage(error)}`)
      }
    },
    [],
  )

  /** 每 1 秒轮询（重入保护，与参考实现的 Interlocked 等价） */
  useEffect(() => {
    if (!isProcessing || !batch) return
    const timer = setInterval(() => {
      if (polling.current) return
      polling.current = true
      void refreshBatch(batch.id, { silent: true }).finally(() => {
        polling.current = false
      })
    }, BATCH_POLL_INTERVAL_MS)
    return () => clearInterval(timer)
  }, [batch, isProcessing, refreshBatch])

  // 任务状态变化：只关心本批次，触发一次刷新
  useEffect(() => {
    if (!batch) return
    const batchId = batch.id
    return onWsEvent('task.status', () => {
      void refreshBatch(batchId, { silent: true })
    })
  }, [batch, refreshBatch])

  // 模板变化时的参数重置规则（§4.3 ④ / §7.6）：参数字典为空、或含有模板已不存在的键时整体重置。
  // 用「渲染期调整状态」而不是 effect，避免级联渲染。
  if (template) {
    const names = new Set((template.parameters ?? []).map((parameter) => parameter.name))
    const keys = Object.keys(parameterValues)
    if (keys.length === 0 || keys.some((key) => !names.has(key))) {
      setParameterValues(defaultParameterValues(template))
    }
  }

  const canStart =
    !isProcessing &&
    files.length > 0 &&
    outputDirectory.trim() !== '' &&
    template !== null

  const cannotStartReason = (() => {
    if (files.length === 0) return '请先添加要处理的文件'
    if (!template) return '请选择转换模板'
    if (outputDirectory.trim() === '') return '请选择输出目录'
    return ''
  })()

  const failedCount = batchTasks.filter(
    (task) => task.status === 'Failed' || task.status === 'Cancelled',
  ).length
  const completedCount = batchTasks.filter((task) => task.status === 'Completed').length

  const startPolling = () => {
    wasProcessing.current = true
    setIsProcessing(true)
  }

  /** 开始批量处理 */
  const handleStart = async () => {
    if (!canStart || !template) return
    setIsProcessing(true)
    try {
      const result = await batchesApi.create({
        inputPaths: files,
        outputDirectory,
        templateId: template.id,
        parameterValues,
        startImmediately: true,
      })
      if (!mounted.current) return

      setBatch(result.batch)
      setBatchTasks(result.tasks ?? [])
      const statuses: Record<string, TaskStatus> = {}
      for (const file of files) statuses[file] = 'Pending'
      setFileStatuses(statuses)
      wasProcessing.current = true

      toast.success(`已提交 ${files.length} 个文件到处理队列`)
      await refreshBatch(result.batch.id, { silent: true })
    } catch (error) {
      const message = getErrorMessage(error)
      if (message.includes('不可写')) toast.error(message)
      else toast.error(`启动批量处理失败：${message}`)
      setIsProcessing(false)
      wasProcessing.current = false
    }
  }

  /** 取消全部（等待 + 运行中） */
  const handleCancelBatch = async () => {
    if (!batch) return
    try {
      await batchesApi.cancel(batch.id)
      toast.warning('已取消本批次中所有等待和运行中的任务。')
      await refreshBatch(batch.id, { silent: true })
    } catch (error) {
      toast.error(`取消失败：${getErrorMessage(error)}`)
    }
  }

  /** 重试全部失败项（失败 + 已取消，无二次确认） */
  const handleRetryAllFailed = async () => {
    if (!batch) return
    const failed = batchTasks.filter(
      (task) => task.status === 'Failed' || task.status === 'Cancelled',
    )
    if (failed.length === 0) {
      toast.show('没有需要重试的任务。')
      return
    }
    try {
      const result = await batchesApi.retryFailed(batch.id)
      if (result.queued > 0) {
        toast.success(`已重新排队 ${result.queued} 个任务`)
        startPolling()
      } else {
        toast.error('重试失败：没有任务被重新排队')
      }
      await refreshBatch(batch.id, { silent: true })
    } catch (error) {
      toast.error(`重试失败：${getErrorMessage(error)}`)
    }
  }

  /** 单任务取消：运行中优雅停止，否则强制取消 */
  const handleCancelTask = async (taskId: string) => {
    if (!batch) return
    const task = batchTasks.find((item) => item.id === taskId)
    const graceful = task?.status === 'Running'
    try {
      await tasksApi.cancel(taskId, graceful)
      toast.warning(graceful ? '已发送优雅停止指令。' : '已取消该任务。')
      await refreshBatch(batch.id, { silent: true })
    } catch {
      toast.show('该任务已经结束。')
    }
  }

  /** 单任务重试 */
  const handleRetryTask = async (taskId: string) => {
    if (!batch) return
    try {
      await tasksApi.retry(taskId)
      toast.success('已重新加入队列')
      startPolling()
      await refreshBatch(batch.id, { silent: true })
    } catch (error) {
      const message = getErrorMessage(error)
      if (message.includes('NOT_FOUND') || message.includes('不存在')) toast.error('找不到原任务，无法重试。')
      else toast.error(`重试失败：${message}`)
    }
  }

  /** 打开输出目录：优先第一个已完成任务的目录 */
  const handleOpenOutputDirectory = async () => {
    const firstCompleted = batchTasks.find((task) => task.status === 'Completed')
    if (!firstCompleted) {
      toast.show('还没有成功完成的文件。')
      return
    }
    await revealDirectory(directoryOf(firstCompleted.outputPath))
  }

  const logCandidates = useMemo(
    () =>
      batchTasks.map((task) => ({
        id: task.id,
        label: fileNameOf(task.inputPath),
      })),
    [batchTasks],
  )

  const showProgressPanel = batch !== null || batchTasks.length > 0 || isProcessing

  return (
    <div className="ffui-container">
      <div className="flex flex-wrap items-center gap-2 mb-5">
        <h1 className="m-0">📁 批量处理</h1>
        {settings && settings.maxConcurrentTasks > 1 ? (
          <Badge tone="info">并发 {settings.maxConcurrentTasks} 个任务</Badge>
        ) : null}
      </div>

      <div className="grid gap-5 mb-5 max-[1000px]:grid-cols-1" style={{ gridTemplateColumns: 'minmax(0, 1.05fr) minmax(0, 1fr)' }}>
        <section className="ffui-panel">
          <h2 className="ffui-panel-title">① 选择文件</h2>
          <BatchFileList
            files={files}
            onChange={setFiles}
            fileStatuses={fileStatuses}
            isProcessing={isProcessing}
          />

          <div className="mt-4 pt-4 border-t border-line">
            <h2 className="ffui-panel-title">② 输出目录</h2>
            <FileSelector
              label="输出目录"
              value={outputDirectory}
              onChange={setOutputDirectory}
              placeholder="转换结果的保存目录"
              isDirectory
              validateExists={false}
            />
            <p className="m-0 text-muted-text" style={{ fontSize: '0.8rem' }}>
              批量处理时，输出文件名由模板的扩展名 + 全局命名规则生成。
              同名文件按「设置 → 文件已存在时的处理方式」处理，不会卡住等待确认。
            </p>
          </div>
        </section>

        <section className="ffui-panel">
          <h2 className="ffui-panel-title">③ 选择模板</h2>
          <TemplateSelector
            selectedTemplate={template}
            onChange={(next) => {
              setTemplate(next)
              setParameterValues(defaultParameterValues(next))
            }}
            inputPath={files.length > 0 ? files[0] : null}
          />
        </section>
      </div>

      {template && (template.parameters ?? []).length > 0 ? (
        <section className="ffui-panel">
          <h2 className="ffui-panel-title flex flex-wrap items-center gap-2">
            ④ 模板参数
            <Badge>{template.name}</Badge>
            <span className="text-muted-text" style={{ fontSize: '0.85rem', fontWeight: 400 }}>
              对本批次所有文件生效
            </span>
          </h2>
          <TemplateParameterEditor
            template={template}
            values={parameterValues}
            onChange={setParameterValues}
            disabled={isProcessing}
          />
        </section>
      ) : null}

      <div className="flex flex-wrap items-center gap-3 mb-5">
        <Button variant="primary" size="lg" disabled={!canStart} onClick={() => void handleStart()}>
          {isProcessing ? <Spinner /> : null}
          {isProcessing ? '正在处理…' : '🚀 开始批量处理'}
        </Button>

        {isProcessing && batch ? (
          <Button variant="danger" onClick={() => void handleCancelBatch()}>
            ❌ 取消全部
          </Button>
        ) : null}

        {files.length > 0 && template && !isProcessing ? (
          <span className="text-muted-text">将处理 {files.length} 个文件，输出格式 .{template.outputExtension}</span>
        ) : !canStart && !isProcessing && cannotStartReason !== '' ? (
          <span className="text-muted-text">{cannotStartReason}</span>
        ) : null}
      </div>

      {showProgressPanel ? (
        <section className="ffui-panel">
          <div className="flex flex-wrap items-center gap-2 mb-3">
            <h2 className="ffui-section-title m-0">📊 处理进度</h2>
            {batch ? <Badge>{batch.name}</Badge> : null}
            <Button size="sm" onClick={() => batch && void refreshBatch(batch.id)} disabled={!batch}>
              刷新
            </Button>
            {!isProcessing && completedCount > 0 ? (
              <Button size="sm" onClick={() => void handleOpenOutputDirectory()}>
                打开输出目录
              </Button>
            ) : null}
            {!isProcessing && failedCount > 0 ? (
              <Button size="sm" onClick={() => void handleRetryAllFailed()}>
                重试全部失败项 ({failedCount})
              </Button>
            ) : null}
          </div>

          <BatchProgressPanel
            tasks={batchTasks}
            isProcessing={isProcessing}
            onRetry={(taskId) => void handleRetryTask(taskId)}
            onCancel={(taskId) => void handleCancelTask(taskId)}
            onOpenOutputFolder={(outputPath) => void revealDirectory(directoryOf(outputPath))}
          />
        </section>
      ) : null}

      {/* 任务日志：按需展开，最多保留 store 上限（800 行/任务）+ 渲染窗口截断 */}
      {batchTasks.length > 0 ? (
        <section className="ffui-panel">
          <div className="flex flex-wrap items-center gap-2 mb-3">
            <h2 className="ffui-panel-title m-0">📜 任务日志</h2>
            <label className="visually-hidden" htmlFor="batch-log-task">
              选择要查看日志的任务
            </label>
            <select
              id="batch-log-task"
              style={{ width: 'auto' }}
              value={logTaskId || logCandidates[0]?.id || ''}
              onChange={(event) => setLogTaskId(event.target.value)}
            >
              {logCandidates.map((candidate) => (
                <option key={candidate.id} value={candidate.id}>
                  {candidate.label}
                </option>
              ))}
            </select>
          </div>

          <TaskLogViewer taskId={logTaskId || logCandidates[0]?.id || ''} maxHeight="320px" />
        </section>
      ) : null}
    </div>
  )
}
