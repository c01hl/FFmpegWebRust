/**
 * 首页 `/`（对应 ui-spec §4.1 `Home.razor`）。
 *
 * 四个区块：Hero → 📊 环境状态 → 🖥️ 硬件编码能力 → 🕒 最近任务。
 *
 * 与参考实现的差异（都是账号里没有等价接口的地方，见报告）：
 * - 状态网格里的 `.NET 版本` 换成 `运行时`（React 版没有运行时版本接口）；
 * - `路径来源` 用 `GET /api/settings` 的 `ffmpegPath` 是否为空判断（自动探测 / 设置中指定）；
 * - `数据目录可写` 用 `GET /api/system/paths` 的 `dataDirectoryWritable`；
 * - 首页不订阅任务事件，但「任务队列」一行读全局 store（外壳的 WebSocket 单例已维护）。
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { getErrorMessage } from '../api/client'
import { settingsApi, systemApi, tasksApi } from '../api/endpoints'
import type { AppStatusDto, HardwareReportDto, SystemPathsDto, TaskDto } from '../api/types'
import { TASK_STATUS_TEXT, TASK_STATUS_TONE } from '../api/types'
import { formatDateTimeMinute, formatDetectionDuration, formatDuration } from '../lib/format'
import { APP_VERSION, PAGE_TITLES, useDocumentTitle } from '../lib/title'
import { selectActiveCount, selectQueuedCount, useQueueStore } from '../store/queue'
import { toast } from '../store/toast'
import { Alert } from '../components/ui/Alert'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { EmptyState } from '../components/ui/EmptyState'
import { EncoderCard } from '../components/convert/EncoderCard'
import { revealDirectory } from '../components/files/revealDirectory'

/** 首页最近任务条数 */
const RECENT_TASK_LIMIT = 5
/** 检测耗时徽标的刷新间隔（参考实现用 200ms 定时器） */
const TICK_MS = 200

/** 未检测到 FFmpeg 时的安装指引（按平台，逐字来自 ui-spec §4.1） */
function platformInstallHint(platform: string | null | undefined): string {
  switch ((platform ?? '').toLowerCase()) {
    case 'windows':
      return '请先安装 FFmpeg：从 ffmpeg.org 下载压缩包后解压，把 bin 目录加入 PATH（或执行 winget install Gyan.FFmpeg），也可以在「设置」里直接指定 ffmpeg.exe 的完整路径。'
    case 'linux':
      return '请先安装 FFmpeg：Debian/Ubuntu 用 apt install ffmpeg，Fedora 用 dnf install ffmpeg，也可以在「设置」里直接指定 ffmpeg 的完整路径。'
    default:
      return '请先安装 FFmpeg：在终端执行 brew install ffmpeg（或从 ffmpeg.org 下载静态包），也可以在「设置」里直接指定 ffmpeg 的完整路径。'
  }
}

export function HomePage() {
  useDocumentTitle(PAGE_TITLES.home)

  const [status, setStatus] = useState<AppStatusDto | null>(null)
  const [paths, setPaths] = useState<SystemPathsDto | null>(null)
  const [settingsFfmpegPath, setSettingsFfmpegPath] = useState<string | null>(null)
  const [report, setReport] = useState<HardwareReportDto | null>(null)
  const [recentTasks, setRecentTasks] = useState<TaskDto[]>([])
  const [isLoadingStatus, setIsLoadingStatus] = useState(true)
  const [isDetecting, setIsDetecting] = useState(false)
  const [detectNote, setDetectNote] = useState<string | null>(null)
  const [statusElapsed, setStatusElapsed] = useState<number | null>(null)
  const [detectElapsed, setDetectElapsed] = useState<number | null>(null)

  const activeCount = useQueueStore(selectActiveCount)
  const queuedCount = useQueueStore(selectQueuedCount)

  const mounted = useRef(true)
  const statusStartedAt = useRef(0)
  const detectStartedAt = useRef(0)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  /** 拉取环境状态（可选 force）+ 数据目录可写性 + 设置里的 FFmpeg 路径 */
  const loadStatus = useCallback(async (force: boolean) => {
    statusStartedAt.current = Date.now()
    const [statusResult, pathsResult, settingsResult] = await Promise.allSettled([
      systemApi.status(force),
      systemApi.paths(),
      settingsApi.get(),
    ])
    if (!mounted.current) return

    if (statusResult.status === 'fulfilled') setStatus(statusResult.value)
    else setStatus(null)
    if (pathsResult.status === 'fulfilled') setPaths(pathsResult.value)
    if (settingsResult.status === 'fulfilled') setSettingsFfmpegPath(settingsResult.value.ffmpegPath ?? null)

    if (statusResult.status === 'rejected') throw statusResult.reason
  }, [])

  const loadRecentTasks = useCallback(async () => {
    const result = await tasksApi.list({ limit: RECENT_TASK_LIMIT, offset: 0 })
    if (!mounted.current) return
    setRecentTasks(result.items ?? [])
  }, [])

  // 首屏：状态 + 最近任务（加载态初始值已经是 true，这里不重复 setState）
  useEffect(() => {
    void (async () => {
      try {
        await Promise.all([loadStatus(false), loadRecentTasks()])
      } catch (error) {
        if (mounted.current) {
          toast.warning(`无法获取系统状态：${getErrorMessage(error)}`)
        }
      } finally {
        if (mounted.current) {
          setIsLoadingStatus(false)
          setStatusElapsed(null)
        }
      }
    })()
  }, [loadRecentTasks, loadStatus])

  // 拉取状态期间每 200ms 刷新「已用 {时长}」（>0.05s 才显示）
  useEffect(() => {
    if (!isLoadingStatus) return
    const timer = setInterval(() => {
      const elapsed = (Date.now() - statusStartedAt.current) / 1000
      setStatusElapsed(elapsed > 0.05 ? elapsed : null)
    }, TICK_MS)
    return () => clearInterval(timer)
  }, [isLoadingStatus])

  // 硬件检测期间每 200ms 刷新耗时
  useEffect(() => {
    if (!isDetecting) return
    const timer = setInterval(() => {
      const elapsed = (Date.now() - detectStartedAt.current) / 1000
      setDetectElapsed(elapsed > 0.05 ? elapsed : null)
    }, TICK_MS)
    return () => clearInterval(timer)
  }, [isDetecting])

  /** 手动刷新：状态（force）+ 硬件报告 + 最近任务（§4.1 文案） */
  const handleRefreshStatus = async () => {
    setIsLoadingStatus(true)
    try {
      const [, reportResult] = await Promise.all([
        loadStatus(true),
        systemApi.encoders(false).catch(() => null),
      ])
      if (reportResult) setReport(reportResult)
      await loadRecentTasks()
      toast.success('环境状态已刷新。')
    } catch (error) {
      toast.error(
        `读取环境状态失败：${getErrorMessage(error)}（可以稍后重试，或先到「设置」检查 FFmpeg 路径）`,
      )
    } finally {
      if (mounted.current) {
        setIsLoadingStatus(false)
        setStatusElapsed(null)
      }
    }
  }

  /** 硬件检测（同步返回，首次可能耗时数十秒） */
  const handleDetect = async () => {
    setIsDetecting(true)
    setDetectNote(null)
    detectStartedAt.current = Date.now()
    try {
      const result = await systemApi.encoders(true)
      if (!mounted.current) return
      setReport(result)
      const seconds = (Date.now() - detectStartedAt.current) / 1000
      toast.success(`硬件检测完成，用时 ${formatDetectionDuration(seconds)}。`)
    } catch (error) {
      toast.error(
        `硬件检测失败：${getErrorMessage(error)}（可以稍后重试；转换会自动改用软件编码）`,
      )
    } finally {
      if (mounted.current) {
        setIsDetecting(false)
        setDetectElapsed(null)
      }
    }
  }

  const hardwareEncoders = (report?.encoders ?? []).filter(
    (encoder) => encoder.type !== 'Software' && encoder.isAvailable,
  )
  const unavailableHardware = (report?.encoders ?? []).filter(
    (encoder) => encoder.type !== 'Software' && !encoder.isAvailable,
  )
  const softwareEncoders = (report?.encoders ?? []).filter((encoder) => encoder.type === 'Software')
  const availableSoftware = softwareEncoders.filter((encoder) => encoder.isAvailable)

  const dataWritable = paths?.dataDirectoryWritable ?? null

  return (
    <div className="ffui-container">
      {/* 区块 0 · Hero */}
      <section className="flex flex-wrap items-center gap-4 mb-5">
        <div>
          <h1 className="m-0">🎬 FFmpeg WebUI</h1>
          <p className="m-0 text-secondary-text" style={{ fontSize: '0.95rem' }}>
            本地媒体文件转换工具 · v{APP_VERSION}
          </p>
        </div>
        <div className="flex flex-wrap gap-2 ml-auto" role="group" aria-label="快速开始">
          <Link className="ffui-btn ffui-btn-primary ffui-btn-lg" to="/convert">
            🔄 开始转换
          </Link>
          <Link className="ffui-btn ffui-btn-lg" to="/batch">
            📁 批量处理
          </Link>
          <Link className="ffui-btn ffui-btn-lg" to="/templates">
            📋 模板
          </Link>
        </div>
      </section>

      {/* 区块 1 · 环境状态 */}
      <section className="ffui-panel">
        <div className="flex flex-wrap items-center gap-2 mb-3">
          <h2 className="ffui-panel-title m-0">📊 环境状态</h2>
          {!isLoadingStatus && status ? (
            <Badge tone={status.ffmpegAvailable ? 'success' : 'danger'}>
              {status.ffmpegAvailable ? '环境正常' : 'FFmpeg 不可用'}
            </Badge>
          ) : null}
          <span className="top-spacer" />
          <Button disabled={isLoadingStatus} onClick={() => void handleRefreshStatus()}>
            {isLoadingStatus ? <span className="spinner" aria-hidden="true" /> : '🔄'} 刷新
          </Button>
        </div>

        {isLoadingStatus && !status ? (
          <div className="ffui-loading-row" role="status">
            <span className="spinner" aria-hidden="true" />
            <span>正在检测环境…首次检测需要试跑硬件编码器，可能需要几十秒。</span>
            {statusElapsed !== null ? <Badge>已用 {formatDetectionDuration(statusElapsed)}</Badge> : null}
          </div>
        ) : null}

        {status && !status.ffmpegAvailable ? (
          <Alert tone="danger" className="mb-3">
            <strong>未检测到可用的 FFmpeg</strong>
            <div className="mt-1">{platformInstallHint(status.platform)}</div>
            <div className="flex flex-wrap gap-2 mt-2">
              <Link className="ffui-btn ffui-btn-primary ffui-btn-sm" to="/settings">
                前往设置
              </Link>
              <a
                className="ffui-btn ffui-btn-sm"
                href="https://ffmpeg.org/download.html"
                target="_blank"
                rel="noreferrer"
              >
                下载 FFmpeg
              </a>
            </div>
          </Alert>
        ) : null}

        {status && status.ffmpegAvailable ? (
          <Alert tone="success" className="mb-3">
            <div>FFmpeg 已就绪 · {status.ffmpegVersion || '—'}</div>
            <div className="mono" style={{ fontSize: '0.85rem' }}>
              {status.ffmpegPath || '—'}
            </div>
          </Alert>
        ) : null}

        {status ? (
          <dl
            className="grid gap-x-4 gap-y-2 m-0"
            style={{ gridTemplateColumns: 'repeat(auto-fit, minmax(260px, 1fr))' }}
          >
            <div>
              <dt className="text-muted-text" style={{ fontSize: '0.8rem' }}>
                FFmpeg 版本
              </dt>
              <dd className="m-0">{status.ffmpegVersion || '—'}</dd>
            </div>
            <div className="min-w-0">
              <dt className="text-muted-text" style={{ fontSize: '0.8rem' }}>
                FFmpeg 路径
              </dt>
              <dd className="m-0 mono truncate" title={status.ffmpegPath || undefined}>
                {status.ffmpegPath || '—'}
              </dd>
            </div>
            <div>
              <dt className="text-muted-text" style={{ fontSize: '0.8rem' }}>
                路径来源
              </dt>
              <dd className="m-0">
                {settingsFfmpegPath === null ? (
                  '—'
                ) : (
                  <Badge>{settingsFfmpegPath.trim() === '' ? '自动探测' : '设置中指定'}</Badge>
                )}
              </dd>
            </div>
            <div>
              <dt className="text-muted-text" style={{ fontSize: '0.8rem' }}>
                平台
              </dt>
              <dd className="m-0">{status.platform || '—'}</dd>
            </div>
            <div>
              <dt className="text-muted-text" style={{ fontSize: '0.8rem' }}>
                运行时
              </dt>
              <dd className="m-0">Rust</dd>
            </div>
            <div className="min-w-0">
              <dt className="text-muted-text" style={{ fontSize: '0.8rem' }}>
                数据目录
              </dt>
              <dd className="m-0 flex flex-wrap items-center gap-2">
                <span className="mono truncate" title={status.dataDirectory || undefined}>
                  {status.dataDirectory || '—'}
                </span>
                {dataWritable === null ? null : dataWritable ? (
                  <Badge tone="success">可写</Badge>
                ) : (
                  <Badge tone="warning">不可写（设置无法保存）</Badge>
                )}
              </dd>
            </div>
            <div>
              <dt className="text-muted-text" style={{ fontSize: '0.8rem' }}>
                硬件解码方式
              </dt>
              <dd className="m-0">
                {(status.hwaccels ?? []).length === 0 ? (
                  <span className="text-muted-text">未检测到（FFmpeg 未启用或不可用）</span>
                ) : (
                  <span className="flex flex-wrap gap-1">
                    {status.hwaccels.map((item) => (
                      <span
                        key={item}
                        className="mono rounded-sm border border-line bg-surface-3 px-1.5 py-0.5"
                        style={{ fontSize: '0.78rem' }}
                      >
                        {item}
                      </span>
                    ))}
                  </span>
                )}
              </dd>
            </div>
            <div>
              <dt className="text-muted-text" style={{ fontSize: '0.8rem' }}>
                模板数量
              </dt>
              <dd className="m-0">
                {status.templateCount} 个 ·{' '}
                <Link to="/templates" style={{ fontSize: '0.85rem' }}>
                  管理模板
                </Link>
              </dd>
            </div>
            <div>
              <dt className="text-muted-text" style={{ fontSize: '0.8rem' }}>
                任务队列
              </dt>
              <dd className="m-0">
                运行中 {activeCount} 个 · 排队 {queuedCount} 个 ·{' '}
                <Link to="/tasks" style={{ fontSize: '0.85rem' }}>
                  查看任务
                </Link>
              </dd>
            </div>
          </dl>
        ) : null}

        {status && (status.notices ?? []).length > 0 ? (
          <div className="mt-3 flex flex-col gap-2">
            {status.notices.map((notice) => (
              <Alert key={notice} tone={status.ffmpegAvailable ? 'warning' : 'danger'}>
                {notice}
              </Alert>
            ))}
          </div>
        ) : null}
      </section>

      {/* 区块 2 · 硬件编码能力 */}
      <section className="ffui-panel">
        <div className="flex flex-wrap items-center gap-2 mb-3">
          <h2 className="ffui-panel-title m-0">🖥️ 硬件编码能力</h2>
          {report ? (
            <Badge tone={hardwareEncoders.length > 0 ? 'success' : 'warning'}>
              {hardwareEncoders.length > 0 ? `${hardwareEncoders.length} 个硬件编码器可用` : '没有可用的硬件编码器'}
            </Badge>
          ) : null}
          <span className="top-spacer" />
          <Button disabled={isDetecting} onClick={() => void handleDetect()}>
            {isDetecting ? <span className="spinner" aria-hidden="true" /> : '🔄'}
            {isDetecting ? '正在检测…' : '刷新检测（可能需要数秒）'}
          </Button>
        </div>

        {isDetecting ? (
          <div className="ffui-loading-row" role="status">
            <span className="spinner" aria-hidden="true" />
            <span>正在逐个试跑硬件编码器，可能需要数秒到一分钟，请不要关闭页面。</span>
            {detectElapsed !== null ? <Badge>已用 {formatDetectionDuration(detectElapsed)}</Badge> : null}
          </div>
        ) : null}

        {detectNote ? (
          <Alert tone="warning" role="status" className="mb-3">
            {detectNote}
          </Alert>
        ) : null}

        {!report && !isDetecting ? (
          <EmptyState>
            还没有检测结果。点击「刷新检测（可能需要数秒）」扫描本机可用的硬件编码器。
          </EmptyState>
        ) : null}

        {report ? (
          <>
            <p className="text-secondary-text mt-0">{report.summary?.trim() ? report.summary : '—'}</p>

            <div className="flex flex-wrap gap-1.5 mb-3">
              <Badge>FFmpeg {report.ffmpegVersion || '—'}</Badge>
              <Badge>检测时间 {formatDateTimeMinute(report.detectedAt)}</Badge>
              <Badge>硬件解码 {(report.hwaccels ?? []).length} 种</Badge>
            </div>

            <h3 className="flex flex-wrap items-center gap-2" style={{ fontSize: '1rem' }}>
              硬件编码器
              <Badge>
                {hardwareEncoders.length} 个可用 / 共探测 {hardwareEncoders.length + unavailableHardware.length} 个
              </Badge>
            </h3>

            {hardwareEncoders.length === 0 ? (
              <EmptyState>
                没有可用的硬件编码器，转换会自动使用软件编码（CPU）：速度慢一些，但结果稳定。如果本机确实有独立显卡或核显，请确认当前的 FFmpeg 构建启用了对应加速（NVENC / VideoToolbox / QSV / VA-API 等）。
              </EmptyState>
            ) : (
              <div
                className="grid"
                style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(260px, 1fr))' }}
              >
                {hardwareEncoders.map((encoder) => (
                  <EncoderCard key={encoder.id || encoder.name} encoder={encoder} platform={report.platform} />
                ))}
              </div>
            )}

            {unavailableHardware.length > 0 ? (
              <details className="mt-3">
                <summary className="cursor-pointer text-secondary-text" style={{ fontSize: '0.9rem' }}>
                  另有 {unavailableHardware.length} 个硬件编码器不可用（点开查看原因）
                </summary>
                <ul className="mt-2 pl-4 text-muted-text" style={{ fontSize: '0.85rem' }}>
                  {unavailableHardware.map((encoder) => (
                    <li key={encoder.id || encoder.name}>
                      <span className="mono">{encoder.name}</span>
                      {' — '}
                      {encoder.unavailableReason?.trim() ? encoder.unavailableReason : '未通过可用性试跑'}
                    </li>
                  ))}
                </ul>
              </details>
            ) : null}

            <details className="mt-3">
              <summary className="cursor-pointer text-secondary-text" style={{ fontSize: '0.9rem' }}>
                软件编码（CPU 兜底）· {availableSoftware.length} 个可用
              </summary>
              {softwareEncoders.length === 0 ? (
                <EmptyState>当前 FFmpeg 构建里没有检测到软件编码器。</EmptyState>
              ) : (
                <div
                  className="grid mt-2"
                  style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(260px, 1fr))' }}
                >
                  {softwareEncoders.map((encoder) => (
                    <EncoderCard key={encoder.id || encoder.name} encoder={encoder} platform={report.platform} />
                  ))}
                </div>
              )}
            </details>
          </>
        ) : null}
      </section>

      {/* 区块 3 · 最近任务 */}
      <section className="ffui-panel">
        <div className="flex flex-wrap items-center gap-2 mb-3">
          <h2 className="ffui-panel-title m-0">🕒 最近任务</h2>
          <span className="top-spacer" />
          <Link className="ffui-btn ffui-btn-sm" to="/tasks">
            查看全部任务
          </Link>
        </div>

        {recentTasks.length === 0 ? (
          <>
            <EmptyState>还没有任何任务，去「转换」页开始第一个吧。</EmptyState>
            <Link className="ffui-btn ffui-btn-primary ffui-btn-sm" to="/convert">
              开始转换
            </Link>
          </>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full" style={{ borderCollapse: 'collapse', fontSize: '0.88rem' }}>
              <thead>
                <tr className="text-muted-text text-left">
                  <th className="py-2">文件</th>
                  <th className="py-2">状态</th>
                  <th className="py-2">耗时</th>
                  <th className="py-2">创建时间</th>
                  <th className="py-2">操作</th>
                </tr>
              </thead>
              <tbody>
                {recentTasks.map((task) => {
                  const fileName = task.inputPath.replace(/\\/g, '/').split('/').pop() || task.inputPath
                  return (
                    <tr key={task.id} className="border-t border-line">
                      <td className="py-2 truncate" style={{ maxWidth: '280px' }} title={task.inputPath}>
                        {fileName}
                      </td>
                      <td className="py-2">
                        <Badge tone={TASK_STATUS_TONE[task.status]}>{TASK_STATUS_TEXT[task.status]}</Badge>
                      </td>
                      <td className="py-2 mono">
                        {task.durationSeconds === null || task.durationSeconds === undefined
                          ? '-'
                          : formatDuration(task.durationSeconds)}
                      </td>
                      <td className="py-2 mono">{formatDateTimeMinute(task.createdAt)}</td>
                      <td className="py-2">
                        {task.status === 'Completed' ? (
                          <Button
                            size="sm"
                            onClick={() => {
                              const directory = task.outputPath.replace(/\\/g, '/').split('/').slice(0, -1).join('/')
                              void revealDirectory(directory, {
                                successText: '已在文件管理器中打开输出目录。',
                              })
                            }}
                          >
                            打开目录
                          </Button>
                        ) : null}
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  )
}
