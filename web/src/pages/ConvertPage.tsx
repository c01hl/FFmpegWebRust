/**
 * 转换页 `/convert`（对应 ui-spec §4.2 `Convert.razor` + §5 命令预览专题）。
 *
 * 核心约定：
 * - **命令预览、输出路径冲突解析、编码器选择、覆盖策略全部来自 `POST /api/tasks/preview`**，
 *   前端不拼任何 ffmpeg 命令、不做任何本地推断（输入/输出/模板/参数任一变化都会
 *   以 250 ms 防抖重新请求预览）；
 * - 输入文件由 `POST /api/files/dialog` 拉起服务端原生对话框选择（无拖放）；
 * - 媒体信息由 `POST /api/media/info` 探测；
 * - 实时进度/日志来自全局 WebSocket store（外壳维护单例连接），页面不再自己开连接。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { getErrorMessage } from '../api/client'
import { mediaApi, settingsApi, systemApi, tasksApi, templatesApi } from '../api/endpoints'
import type {
  AppStatusDto,
  CommandTemplate,
  HardwareReportDto,
  MediaInfoDto,
  TaskDto,
  TaskPreviewDto,
  TemplateIssue,
  TemplateTestResultDto,
} from '../api/types'
import { TASK_STATUS_TEXT, TASK_STATUS_TONE, isTerminalStatus } from '../api/types'
import { onWsEvent } from '../api/ws'
import { formatDateTimeShort, formatDuration, toFixedTrim } from '../lib/format'
import { PAGE_TITLES, useDocumentTitle } from '../lib/title'
import { selectActiveCount, useQueueStore } from '../store/queue'
import { toast } from '../store/toast'
import { Alert } from '../components/ui/Alert'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { EmptyState } from '../components/ui/EmptyState'
import { ProgressBar } from '../components/ui/ProgressBar'
import { Spinner } from '../components/ui/Spinner'
import { CommandPreview } from '../components/convert/CommandPreview'
import { TaskLogViewer } from '../components/convert/TaskLogViewer'
import { familyDisplayName } from '../components/convert/hardwareLabels'
import { FileSelector } from '../components/files/FileSelector'
import { revealDirectory } from '../components/files/revealDirectory'
import { TemplateParameterEditor } from '../components/template/TemplateParameterEditor'
import { TemplateSelector } from '../components/template/TemplateSelector'

/** 预览请求防抖（毫秒） */
export const PREVIEW_DEBOUNCE_MS = 250
/** 「最近任务」条数 */
const RECENT_TASK_LIMIT = 8

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

/** 参数默认值（模板 Parameters → DefaultValue） */
function defaultParameterValues(template: CommandTemplate | null): Record<string, string> {
  const result: Record<string, string> = {}
  for (const parameter of template?.parameters ?? []) {
    if ((parameter.name ?? '').trim() === '') continue
    result[parameter.name] = parameter.defaultValue ?? ''
  }
  return result
}

/** 自动文件名提示（只是占位提示；真正的路径由预览接口给出） */
function autoNameHint(
  namingRule: string | null,
  suffix: string | null,
  inputPath: string,
  extension: string,
): string {
  const base = inputPath.trim() === '' ? '输入文件名' : fileNameOf(inputPath).replace(/\.[^.]+$/, '')
  const ext = extension.trim() === '' ? 'mp4' : extension.replace(/^\./, '')
  if (namingRule === 'Suffix') return `${base}${suffix ?? ''}.${ext}`
  return `${base}.${ext}`
}

export function ConvertPage() {
  useDocumentTitle(PAGE_TITLES.convert)
  const [searchParams] = useSearchParams()
  const presetTemplateId = searchParams.get('template')

  const [status, setStatus] = useState<AppStatusDto | null>(null)
  const [report, setReport] = useState<HardwareReportDto | null>(null)
  const [isRedetecting, setIsRedetecting] = useState(false)

  const [inputPath, setInputPath] = useState('')
  const [outputDirectory, setOutputDirectory] = useState('')
  const [outputFileName, setOutputFileName] = useState('')
  const [namingRule, setNamingRule] = useState<string | null>(null)
  const [namingSuffix, setNamingSuffix] = useState<string | null>(null)
  const [showFormatHelp, setShowFormatHelp] = useState(false)

  const [template, setTemplate] = useState<CommandTemplate | null>(null)
  const [parameterValues, setParameterValues] = useState<Record<string, string>>({})

  const [mediaInfo, setMediaInfo] = useState<MediaInfoDto | null>(null)
  const [isProbing, setIsProbing] = useState(false)

  const [preview, setPreview] = useState<TaskPreviewDto | null>(null)
  const [previewError, setPreviewError] = useState<string | null>(null)
  const [isPreviewing, setIsPreviewing] = useState(false)
  const [overwriteExisting, setOverwriteExisting] = useState(false)

  const [isStarting, setIsStarting] = useState(false)
  const [isTesting, setIsTesting] = useState(false)
  const [testResult, setTestResult] = useState<TemplateTestResultDto | null>(null)

  const [recentTasks, setRecentTasks] = useState<TaskDto[]>([])
  const [expandedTaskId, setExpandedTaskId] = useState<string | null>(null)

  const activeCount = useQueueStore(selectActiveCount)
  const runningTasks = useQueueStore((state) => state.runningTasks)
  const pendingTasks = useQueueStore((state) => state.pendingTasks)
  const progressMap = useQueueStore((state) => state.progress)

  const mounted = useRef(true)
  const previewTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const previewGeneration = useRef(0)
  const lastProbedPath = useRef<string | null>(null)
  const problemTemplateId = useRef<string | null>(null)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      if (previewTimer.current !== null) clearTimeout(previewTimer.current)
    }
  }, [])

  /** 首屏：环境状态 + 硬件报告 + 设置（默认输出目录 / 命名规则）+ 最近任务 */
  useEffect(() => {
    void (async () => {
      const [statusResult, reportResult, settingsResult, taskResult] = await Promise.allSettled([
        systemApi.status(),
        systemApi.encoders(),
        settingsApi.get(),
        tasksApi.list({ limit: RECENT_TASK_LIMIT, offset: 0 }),
      ])
      if (!mounted.current) return
      if (statusResult.status === 'fulfilled') setStatus(statusResult.value)
      if (reportResult.status === 'fulfilled') setReport(reportResult.value)
      if (settingsResult.status === 'fulfilled') {
        setOutputDirectory((current) => current || settingsResult.value.defaultOutputDirectory || '')
        setNamingRule(settingsResult.value.outputNaming ?? null)
        setNamingSuffix(settingsResult.value.outputSuffix ?? null)
      }
      if (taskResult.status === 'fulfilled') setRecentTasks(taskResult.value.items ?? [])
      if (statusResult.status === 'rejected') {
        toast.warning(`无法获取系统状态：${getErrorMessage(statusResult.reason)}`)
      }
    })()
  }, [])

  // `?template=<id>` 预选模板（模板页「选择」按钮跳转时使用）
  useEffect(() => {
    if (!presetTemplateId) return
    void (async () => {
      try {
        const preset = await templatesApi.get(presetTemplateId)
        if (!mounted.current) return
        setTemplate(preset)
        setParameterValues(defaultParameterValues(preset))
      } catch (error) {
        toast.warning(`无法预选模板：${getErrorMessage(error)}`)
      }
    })()
  }, [presetTemplateId])

  /** 探测媒体信息（只在路径真的变化时执行） */
  const probeMedia = useCallback(async (path: string) => {
    if (path.trim() === '') {
      setMediaInfo(null)
      lastProbedPath.current = null
      return
    }
    if (lastProbedPath.current === path) return
    lastProbedPath.current = path

    setIsProbing(true)
    try {
      const result = await mediaApi.info(path)
      if (!mounted.current) return
      if (!result.info) {
        setMediaInfo(null)
        toast.warning('无法读取该文件的媒体信息，可能是格式不受支持或文件已损坏。')
      } else {
        setMediaInfo(result.info)
      }
    } catch (error) {
      if (!mounted.current) return
      setMediaInfo(null)
      toast.error(`读取媒体信息失败：${getErrorMessage(error)}`)
    } finally {
      if (mounted.current) setIsProbing(false)
    }
  }, [])

  /** 重新请求预览（后端算，前端渲染） */
  const runPreview = useCallback(
    async (options?: { overwrite?: boolean }) => {
      const overwrite = options?.overwrite ?? overwriteExisting
      if (!template || inputPath.trim() === '' || outputDirectory.trim() === '') {
        setPreview(null)
        return
      }
      const generation = ++previewGeneration.current
      setIsPreviewing(true)
      try {
        const result = await tasksApi.preview({
          inputPath,
          outputDirectory,
          outputFileNameTemplate: outputFileName.trim() === '' ? null : outputFileName.trim(),
          outputExtension: null,
          templateId: template.id,
          parameterValues,
          overwriteExisting: overwrite,
          allowRename: true,
        })
        if (generation !== previewGeneration.current || !mounted.current) return
        setPreview(result)
        setPreviewError(null)
      } catch (error) {
        if (generation !== previewGeneration.current || !mounted.current) return
        setPreview(null)
        setPreviewError(`生成命令预览失败：${getErrorMessage(error)}`)
      } finally {
        if (generation === previewGeneration.current && mounted.current) setIsPreviewing(false)
      }
    },
    [inputPath, outputDirectory, outputFileName, overwriteExisting, parameterValues, template],
  )

  // 输入/输出/文件名/模板/参数任一变化 → 250ms 防抖重建预览
  useEffect(() => {
    if (previewTimer.current !== null) clearTimeout(previewTimer.current)
    previewTimer.current = setTimeout(() => {
      previewTimer.current = null
      void runPreview()
    }, PREVIEW_DEBOUNCE_MS)
    return () => {
      if (previewTimer.current !== null) clearTimeout(previewTimer.current)
    }
  }, [runPreview])

  /** 拉取最近任务 */
  const loadRecentTasks = useCallback(async () => {
    try {
      const result = await tasksApi.list({ limit: RECENT_TASK_LIMIT, offset: 0 })
      if (mounted.current) setRecentTasks(result.items ?? [])
    } catch (error) {
      console.warn(`[convert] 读取最近任务失败：${getErrorMessage(error)}`)
    }
  }, [])

  // 任务状态变化：Toast 播报 + 刷新列表（§4.2 实时行为表）
  useEffect(
    () =>
      onWsEvent('task.status', (event) => {
        void loadRecentTasks()
        const name = event.data.task ? fileNameOf(event.data.task.inputPath) : ''
        switch (event.data.newStatus) {
          case 'Completed':
            toast.success(`✅ 转换完成：${name}`)
            break
          case 'Failed':
            toast.error(`❌ 转换失败：${name} — ${event.data.errorMessage ?? '未知错误'}`)
            break
          case 'Cancelled':
            toast.warning(`已取消：${name}`)
            break
          case 'Skipped':
            toast.warning(`已跳过：${name}（输出文件已存在）`)
            break
          default:
            break
        }
      }),
    [loadRecentTasks],
  )

  // 模板存在 Error 级问题：提示一次即可（结果来自预览接口）
  const errorIssues: TemplateIssue[] = (preview?.issues ?? []).filter(
    (issue) => issue.severity === 'Error',
  )
  useEffect(() => {
    if (!template || errorIssues.length === 0) return
    if (problemTemplateId.current === template.id) return
    problemTemplateId.current = template.id
    toast.error('当前模板存在校验问题，无法执行，请改用其他模板。')
  }, [errorIssues.length, template])

  const canStart =
    !isStarting &&
    Boolean(status?.ffmpegAvailable) &&
    inputPath.trim() !== '' &&
    outputDirectory.trim() !== '' &&
    Boolean(template) &&
    errorIssues.length === 0 &&
    preview?.templateRunnable !== false

  const cannotStartReason = (() => {
    if (canStart) return ''
    if (!status?.ffmpegAvailable) return 'FFmpeg 不可用，请先到设置里指定路径'
    if (inputPath.trim() === '') return '请选择输入文件'
    if (!template) return '请选择转换模板'
    if (outputDirectory.trim() === '') return '请选择输出目录'
    if (errorIssues.length > 0) return '当前模板存在问题，请改用其他模板'
    if (preview?.templateRunnable === false) {
      return '当前模板要求的硬件编码器在本机不可用，请改用「推荐」分类下的模板'
    }
    return ''
  })()

  const handleTemplateSelected = (next: CommandTemplate) => {
    setTemplate(next)
    setParameterValues(defaultParameterValues(next))
    setTestResult(null)
    setPreview(null)
  }

  const handleParameterChange = (values: Record<string, string>) => {
    setParameterValues(values)
  }

  const handleResetParameters = () => {
    setParameterValues(defaultParameterValues(template))
    setTestResult(null)
    void runPreview()
  }

  const handleConflictChoice = (overwrite: boolean) => {
    setOverwriteExisting(overwrite)
    void runPreview({ overwrite })
  }

  /** 开始转换：输出路径与覆盖策略都来自预览结果 */
  const handleStart = async () => {
    if (!template || !canStart) return
    setIsStarting(true)
    try {
      const outputPath = preview?.resolvedOutputPath
      if (!outputPath) {
        toast.error('无法确定输出路径，请检查输出目录设置。')
        return
      }
      if (preview && !preview.outputDirectoryWritable) {
        toast.error(`输出目录不可写：${outputDirectory}。请换一个目录或检查文件权限。`)
        return
      }

      const result = await tasksApi.create({
        inputPath,
        outputPath,
        templateId: template.id,
        templateName: template.name,
        parameterValues,
        outputPathResolved: true,
        overwriteExisting,
        startImmediately: true,
      })
      if (!mounted.current) return

      if (!result.enqueued) {
        toast.warning('任务已创建但未能立即调度，请到「任务列表」查看。')
      } else {
        toast.success(`已开始转换：${fileNameOf(inputPath)}`)
        setExpandedTaskId(result.task.id)
      }
      await loadRecentTasks()
    } catch (error) {
      const message = getErrorMessage(error)
      if (message.includes('不可写')) toast.error(message)
      else toast.error(`创建任务失败：${message}`)
    } finally {
      if (mounted.current) setIsStarting(false)
    }
  }

  /** 试运行模板（同步，最长约 30s） */
  const handleTest = async () => {
    if (!template) return
    setIsTesting(true)
    try {
      const result = await templatesApi.test(template.id, parameterValues)
      if (!mounted.current) return
      setTestResult(result)
      if (result.success) toast.success('模板试运行成功，该模板在本机可用')
      else toast.warning('模板试运行失败，请看下方详情')
    } catch (error) {
      toast.error(`试运行出错：${getErrorMessage(error)}`)
    } finally {
      if (mounted.current) setIsTesting(false)
    }
  }

  const handleRedetect = async () => {
    setIsRedetecting(true)
    try {
      const result = await systemApi.status(true)
      if (!mounted.current) return
      setStatus(result)
      toast.success('已重新检测环境')
    } catch (error) {
      toast.error(`重新检测失败：${getErrorMessage(error)}`)
    } finally {
      if (mounted.current) setIsRedetecting(false)
    }
  }

  /** 取消任务：运行中优雅停止，否则强制取消（§4.2 分支文案） */
  const handleCancel = async (task: TaskDto, gracefulRequested: boolean) => {
    // 只有运行中的任务才谈得上「优雅停止」；其它状态一律强制取消
    const graceful = gracefulRequested && task.status === 'Running' && !isTerminalStatus(task.status)
    try {
      await tasksApi.cancel(task.id, graceful)
      if (graceful) toast.warning('已发送优雅停止指令，FFmpeg 会写完文件尾再退出。')
      else toast.warning('已取消任务，未完成的输出文件会被清理。')
      await loadRecentTasks()
    } catch (error) {
      const message = getErrorMessage(error)
      if (task.status === 'Running') toast.warning('该任务已经不在运行中。')
      else toast.warning(message.includes('结束') ? '该任务已经结束。' : `取消失败：${message}`)
    }
  }

  const handleRetry = async (taskId: string) => {
    try {
      await tasksApi.retry(taskId)
      toast.success('已重新加入队列')
      await loadRecentTasks()
    } catch (error) {
      const message = getErrorMessage(error)
      if (message.includes('NOT_FOUND') || message.includes('不存在')) toast.error('找不到原任务，无法重试。')
      else toast.error(`重试失败：${message}`)
    }
  }

  const activeList = useMemo(
    () =>
      [...runningTasks, ...pendingTasks].sort(
        (left, right) => Date.parse(right.createdAt) - Date.parse(left.createdAt),
      ),
    [pendingTasks, runningTasks],
  )

  const hardwareCount = report
    ? report.encoders.filter((encoder) => encoder.isAvailable && encoder.type !== 'Software').length
    : null

  const autoOutputHint = autoNameHint(
    namingRule,
    namingSuffix,
    inputPath,
    template?.outputExtension ?? 'mp4',
  )

  return (
    <div className="ffui-container">
      <div className="flex flex-wrap items-center gap-2 mb-5">
        <h1 className="m-0">🔄 媒体转换</h1>
        {status?.ffmpegAvailable === false ? (
          <Badge tone="danger">FFmpeg 不可用</Badge>
        ) : status ? (
          <>
            <Badge tone="success">FFmpeg {status.ffmpegVersion || '可用'}</Badge>
            {hardwareCount === null ? null : hardwareCount > 0 ? (
              <Badge tone="info">🚀 {hardwareCount} 个硬件编码器可用</Badge>
            ) : (
              <Badge>🧮 使用软件编码</Badge>
            )}
          </>
        ) : null}
      </div>

      {status?.ffmpegAvailable === false ? (
        <Alert tone="danger" className="mb-5">
          <div>
            <strong>未找到可用的 FFmpeg，无法开始转换。</strong>
          </div>
          <div className="mono mt-1" style={{ fontSize: '0.85rem' }}>
            当前使用的路径：{status.ffmpegPath || '—'}
          </div>
          <div className="flex flex-wrap gap-2 mt-2">
            <Link className="ffui-btn ffui-btn-primary ffui-btn-sm" to="/settings">
              前往设置指定 FFmpeg 路径
            </Link>
            <Button size="sm" disabled={isRedetecting} onClick={() => void handleRedetect()}>
              {isRedetecting ? <Spinner /> : null} 重新检测
            </Button>
          </div>
        </Alert>
      ) : null}

      <div className="grid grid-cols-2 gap-5 max-[1000px]:grid-cols-1">
        {/* 面板 1 · 输入与输出 */}
        <section className="ffui-panel">
          <h2 className="ffui-panel-title">1 · 输入与输出</h2>

          <FileSelector
            label="输入文件"
            value={inputPath}
            onChange={(next) => {
              setInputPath(next)
              void probeMedia(next)
            }}
            placeholder="输入视频或音频文件的完整路径"
            validateExists
            showFileInfo
          />

          {isProbing ? (
            <div className="ffui-loading-row" role="status">
              <Spinner />
              正在读取媒体信息…
            </div>
          ) : mediaInfo ? (
            <div className="flex flex-wrap gap-1.5 mb-3">
              <Badge tone="info">{mediaInfo.formatDisplay || mediaInfo.format || '未知格式'}</Badge>
              <Badge>时长 {formatDuration(mediaInfo.duration)}</Badge>
              {mediaInfo.hasVideo ? (
                <>
                  <Badge>
                    {mediaInfo.width}×{mediaInfo.height}
                  </Badge>
                  <Badge>{toFixedTrim(mediaInfo.frameRate, 2)} fps</Badge>
                  <Badge>{mediaInfo.videoCodec ?? '未知'}</Badge>
                </>
              ) : null}
              {mediaInfo.hasAudio ? (
                <Badge>
                  {mediaInfo.audioCodec ?? '未知'} · {mediaInfo.audioChannels} 声道
                </Badge>
              ) : (
                <Badge tone="warning">无音轨</Badge>
              )}
              {mediaInfo.subtitleStreamCount > 0 ? <Badge>{mediaInfo.subtitleStreamCount} 条字幕</Badge> : null}
            </div>
          ) : null}

          <FileSelector
            label="输出目录"
            value={outputDirectory}
            onChange={setOutputDirectory}
            placeholder="转换结果的保存目录"
            isDirectory
            validateExists={false}
          />

          <div className="ffui-field mb-4">
            <label className="ffui-field-label" htmlFor="convert-output-name">
              输出文件名
            </label>
            <input
              id="convert-output-name"
              type="text"
              value={outputFileName}
              placeholder={autoOutputHint}
              onChange={(event) => setOutputFileName(event.target.value)}
            />
            <div className="ffui-field-help">
              留空则按「设置 → 输出文件命名规则」自动生成。支持 <code>{'{filename}'}</code>{' '}
              <code>{'{date}'}</code> <code>{'{now:yyMMdd_HHmm}'}</code> <code>{'{random:6}'}</code>。
              <button
                type="button"
                className="ml-1 underline cursor-pointer bg-transparent border-0 text-primary"
                style={{ fontSize: 'inherit' }}
                onClick={() => setShowFormatHelp((value) => !value)}
              >
                {showFormatHelp ? '收起全部占位符' : '查看全部占位符'}
              </button>
            </div>

            {showFormatHelp ? (
              <div
                className="mt-1.5 flex flex-col gap-1 rounded-sm border border-line bg-surface-3 p-2 text-muted-text"
                style={{ fontSize: '0.78rem' }}
                role="note"
              >
                <span>
                  <code>{'{filename}'}</code> 原文件名（不含扩展名）
                </span>
                <span>
                  <code>{'{ext}'}</code> 原扩展名
                </span>
                <span>
                  <code>{'{outputext}'}</code> 输出扩展名
                </span>
                <span>
                  <code>{'{dir}'}</code> 原文件所在文件夹名
                </span>
                <span>
                  <code>{'{date}'}</code> 日期 yyyyMMdd
                </span>
                <span>
                  <code>{'{time}'}</code> 时间 HHmmss
                </span>
                <span>
                  <code>{'{datetime}'}</code> yyyyMMdd_HHmmss
                </span>
                <span>
                  <code>{'{year}'}</code> <code>{'{month}'}</code> <code>{'{day}'}</code>{' '}
                  <code>{'{hour}'}</code> <code>{'{minute}'}</code> <code>{'{second}'}</code>
                </span>
                <span>
                  <code>{'{now:格式}'}</code> 自定义日期格式，如 <code>{'{now:yyMMdd_HHmm}'}</code>
                </span>
                <span>
                  <code>{'{random}'}</code> 8 位随机串
                </span>
                <span>
                  <code>{'{random:N}'}</code> N 位随机串，如 <code>{'{random:6}'}</code>
                </span>
              </div>
            ) : null}
          </div>
        </section>

        {/* 面板 2 · 选择转换模板 */}
        <section className="ffui-panel">
          <h2 className="ffui-panel-title">2 · 选择转换模板</h2>
          <TemplateSelector
            selectedTemplate={template}
            onChange={handleTemplateSelected}
            inputPath={inputPath || null}
          />
        </section>
      </div>

      {/* 面板 3 · 模板参数 */}
      {template ? (
        <section className="ffui-panel">
          <h2 className="ffui-panel-title flex flex-wrap items-center gap-2">
            3 · 模板参数
            <Badge>{template.name}</Badge>
          </h2>

          {errorIssues.length > 0 ? (
            <Alert tone="danger" className="mb-3">
              <div>
                <strong>⛔ 该模板存在问题，无法执行：</strong>
                <ul className="m-0 mt-1 pl-4">
                  {errorIssues.map((issue) => (
                    <li key={issue.message}>{issue.message}</li>
                  ))}
                </ul>
              </div>
            </Alert>
          ) : null}

          {(template.parameters ?? []).filter((parameter) => (parameter.name ?? '').trim() !== '').length === 0 ? (
            <p className="text-muted-text m-0">该模板没有可调参数，将直接使用模板预设值。</p>
          ) : (
            <TemplateParameterEditor
              template={template}
              values={parameterValues}
              onChange={handleParameterChange}
              disabled={isStarting}
            />
          )}
        </section>
      ) : null}

      {/* 面板 4 · 确认并开始 */}
      {inputPath.trim() !== '' && template ? (
        <section className="ffui-panel">
          <h2 className="ffui-panel-title">4 · 确认并开始</h2>

          <div className="flex flex-col gap-1 mb-3">
            <div className="flex gap-2 items-baseline">
              <span className="text-muted-text shrink-0" style={{ width: '56px' }}>
                输入
              </span>
              <span className="mono truncate" title={inputPath}>
                {inputPath}
              </span>
            </div>
            <div className="flex gap-2 items-baseline">
              <span className="text-muted-text shrink-0" style={{ width: '56px' }}>
                输出
              </span>
              <span className="mono truncate" title={preview?.resolvedOutputPath ?? undefined}>
                {preview?.resolvedOutputPath ?? '尚未确定'}
              </span>
            </div>
            <div className="flex gap-2 items-center">
              <span className="text-muted-text shrink-0" style={{ width: '56px' }}>
                编码器
              </span>
              {!preview?.encoder ? (
                <span className="text-muted-text">由模板内部指定</span>
              ) : (
                <>
                  <Badge tone={preview.encoder.isHardware ? 'success' : 'default'}>
                    {preview.encoder.isHardware ? '🚀 硬件加速' : '🧮 软件编码'} · {preview.encoder.name}
                  </Badge>
                  <span className="text-muted-text">{familyDisplayName(preview.encoder.family)}</span>
                </>
              )}
            </div>
          </div>

          {preview?.conflictNotice || preview?.pathsEqualWarning ? (
            <Alert tone="warning" role="status" className="mb-3">
              <div>{preview.pathsEqualWarning ?? preview.conflictNotice}</div>
              <div className="flex flex-wrap gap-2 mt-2">
                <Button size="sm" onClick={() => handleConflictChoice(true)}>
                  覆盖已有文件
                </Button>
                <Button size="sm" onClick={() => handleConflictChoice(false)}>
                  保留两者（自动改名）
                </Button>
              </div>
            </Alert>
          ) : null}

          {preview?.blockingError ? (
            <Alert tone="danger" className="mb-3">
              {preview.blockingError}
            </Alert>
          ) : null}

          {(preview?.warnings ?? []).map((warning) => (
            <Alert key={warning} tone="warning" role="status" className="mb-3">
              <span aria-hidden="true">💡</span> {warning}
            </Alert>
          ))}

          <CommandPreview
            command={preview?.commandPreview ?? ''}
            loading={isPreviewing}
            error={previewError}
          />

          <div className="flex flex-wrap items-center gap-3 mt-3">
            <Button
              variant="primary"
              size="lg"
              disabled={!canStart}
              onClick={() => void handleStart()}
            >
              {isStarting ? <Spinner /> : null}
              {isStarting ? '正在启动…' : '🚀 开始转换'}
            </Button>

            <Button disabled={isTesting} onClick={() => void handleTest()}>
              {isTesting ? <Spinner /> : null}
              {isTesting ? '试运行中…' : '🧪 试运行模板'}
            </Button>

            {(template.parameters ?? []).length > 0 ? (
              <Button disabled={isStarting} onClick={handleResetParameters}>
                重置参数
              </Button>
            ) : null}

            {!canStart && !isStarting && cannotStartReason !== '' ? (
              <span className="text-muted-text">{cannotStartReason}</span>
            ) : null}
          </div>

          {testResult ? (
            <div className="mt-3">
              <Alert tone={testResult.success ? 'success' : 'danger'} role="status">
                <div>{testResult.message}</div>
                {!testResult.success && testResult.output ? (
                  <details className="mt-2">
                    <summary className="cursor-pointer">查看 FFmpeg 输出</summary>
                    <pre
                      className="mono mt-1"
                      style={{ maxHeight: '240px', overflow: 'auto', fontSize: '0.78rem' }}
                    >
                      {testResult.output}
                    </pre>
                  </details>
                ) : null}
                <div className="flex justify-end mt-2">
                  <Button size="sm" onClick={() => setTestResult(null)}>
                    关闭
                  </Button>
                </div>
              </Alert>
            </div>
          ) : null}
        </section>
      ) : null}

      {/* 进行中的转换（实时数据来自 WebSocket store） */}
      {activeCount > 0 && activeList.length > 0 ? (
        <section className="ffui-panel">
          <div className="flex flex-wrap items-center gap-2 mb-3">
            <h2 className="ffui-panel-title m-0">⏱️ 进行中的转换</h2>
            <Badge>{activeList.length}</Badge>
            <span className="top-spacer" />
            <Button size="sm" onClick={() => void loadRecentTasks()}>
              刷新
            </Button>
          </div>

          <div className="flex flex-col gap-3">
            {activeList.map((task) => {
              const live = progressMap[task.id]
              const expanded = expandedTaskId === task.id
              return (
                <div key={task.id} className="rounded-md border border-line bg-surface-3 p-3">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="truncate" title={task.inputPath}>
                      {fileNameOf(task.inputPath)}
                    </span>
                    <Badge tone={TASK_STATUS_TONE[task.status]}>{TASK_STATUS_TEXT[task.status]}</Badge>
                    <span className="text-muted-text" style={{ fontSize: '0.82rem' }}>
                      {task.templateName}
                    </span>
                  </div>

                  <div className="mt-2">
                    <ProgressBar
                      progress={live?.progress ?? task.progress}
                      speed={live?.speed ?? task.processingSpeed}
                      eta={live?.eta ?? task.estimatedTimeRemaining}
                      currentTime={live?.currentTime ?? task.currentTime}
                      totalDuration={live?.totalDuration ?? task.totalDuration}
                      status={task.status}
                    />
                  </div>

                  <div className="flex flex-wrap gap-1.5 mt-2">
                    {task.status === 'Running' ? (
                      <>
                        <Button
                          size="sm"
                          title="发送 q 让 FFmpeg 写完文件尾再退出，已产出的文件仍然可用"
                          onClick={() => void handleCancel(task, true)}
                        >
                          ⏏️ 优雅停止
                        </Button>
                        <Button size="sm" variant="danger" onClick={() => void handleCancel(task, false)}>
                          ❌ 强制终止
                        </Button>
                      </>
                    ) : (
                      <Button size="sm" variant="danger" onClick={() => void handleCancel(task, false)}>
                        取消排队
                      </Button>
                    )}
                    <Button
                      size="sm"
                      onClick={() => setExpandedTaskId(expanded ? null : task.id)}
                      aria-expanded={expanded}
                    >
                      {expanded ? '▲ 收起日志' : '▼ 查看日志'}
                    </Button>
                  </div>

                  {expanded ? (
                    <div className="mt-2">
                      <TaskLogViewer taskId={task.id} maxHeight="260px" />
                    </div>
                  ) : null}
                </div>
              )
            })}
          </div>
        </section>
      ) : null}

      {/* 最近任务 */}
      {recentTasks.length > 0 ? (
        <section className="ffui-panel">
          <div className="flex flex-wrap items-center gap-2 mb-3">
            <h2 className="ffui-panel-title m-0">📜 最近任务</h2>
            <span className="top-spacer" />
            <Link className="ffui-btn ffui-btn-sm" to="/tasks">
              查看全部
            </Link>
          </div>

          <div className="overflow-x-auto">
            <table className="w-full" style={{ borderCollapse: 'collapse', fontSize: '0.88rem' }}>
              <thead>
                <tr className="text-muted-text text-left">
                  <th className="py-2">输入文件</th>
                  <th className="py-2">状态</th>
                  <th className="py-2">耗时</th>
                  <th className="py-2">创建时间</th>
                  <th className="py-2">操作</th>
                </tr>
              </thead>
              <tbody>
                {recentTasks.map((task) => (
                  <tr key={task.id} className="border-t border-line">
                    <td className="py-2 truncate" style={{ maxWidth: '260px' }} title={task.inputPath}>
                      {fileNameOf(task.inputPath)}
                    </td>
                    <td className="py-2">
                      <Badge tone={TASK_STATUS_TONE[task.status]}>{TASK_STATUS_TEXT[task.status]}</Badge>
                    </td>
                    <td className="py-2 mono">
                      {task.durationSeconds === null || task.durationSeconds === undefined
                        ? '-'
                        : formatDuration(task.durationSeconds)}
                    </td>
                    <td className="py-2 mono">{formatDateTimeShort(task.createdAt)}</td>
                    <td className="py-2">
                      <div className="flex flex-wrap gap-1.5">
                        <Button size="sm" onClick={() => void handleRetry(task.id)}>
                          重试
                        </Button>
                        {task.status === 'Completed' ? (
                          <Button
                            size="sm"
                            onClick={() => void revealDirectory(directoryOf(task.outputPath))}
                          >
                            打开目录
                          </Button>
                        ) : null}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      ) : (
        <section className="ffui-panel">
          <EmptyState>还没有任何任务，去上方「输入文件」开始第一个转换吧。</EmptyState>
        </section>
      )}

    </div>
  )
}
