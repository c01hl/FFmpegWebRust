/**
 * 设置页（对应 ui-spec §4.7 与参考实现 `Components/Pages/Settings.razor`）。
 *
 * 六个面板 + 底部粘性行动栏：
 * 1. 🩺 环境信息（`GET /api/system/status` + `/api/system/paths` + `/api/health`，可「重新检测」）
 * 2. 🚀 硬件加速能力（`GET /api/system/encoders`，首选编码器下拉 + 三个开关）
 * 3. 📁 FFmpeg 路径（ffmpeg/ffprobe 两个路径 + 验证按钮，`POST /api/system/ffmpeg/validate`）
 * 4. 📂 输出设置（默认输出目录 / 命名规则 / 后缀 / 文件已存在时的处理方式）
 * 5. ⚡ 性能与日志（并发数 / 日志行数 / 历史保留天数 / 历史清理 / 全局 FFmpeg 参数）
 * 6. 🎨 界面与通知（主题三档 / 完成后通知 / 自动打开浏览器 / 监听端口）
 * 7. 底部行动栏（`PUT /api/settings` 保存 + 服务端 warnings、`POST /api/settings/reset` 恢复默认）
 *
 * 与参考实现的两处**有意差异**（都写了注释）：
 * - 并发数表单范围是 **1 ~ 16**（本项目统一了参考实现「模型 1..16 / 表单 1..8」的两套限制，
 *   见 docs/api-contract.md §0.1 第 4 条与 src/models/settings.rs 的 MAX_CONCURRENT_TASKS_RANGE）；
 * - `.NET 版本` 换成了 Rust 版的运行环境信息（数据库路径 / 是否容器运行），数据来自 `/api/system/paths`。
 *
 * 主题：改主题**立即生效并单独记住**（只把 theme 字段写回服务端），不整份保存表单，
 * 也不会因此显示「有未保存的更改」——与参考实现 `OnThemeChangedAsync` 的语义一致。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { ChangeEvent, ReactNode } from 'react'
import { getErrorMessage } from '../api/client'
import { filesApi, historyApi, settingsApi, systemApi } from '../api/endpoints'
import type {
  ApiThemeMode,
  AppStatusDto,
  FileExistsAction,
  HardwareEncoderDto,
  HardwareReportDto,
  HealthDto,
  OutputNamingRule,
  SettingsDto,
  SettingsPortDto,
  SystemPathsDto,
} from '../api/types'
import { isHardwareEncoder } from '../api/types'
import { onWsEvent } from '../api/ws'
import { EMPTY_VALUE, formatDateTime } from '../lib/format'
import { APP_VERSION, PAGE_TITLES, useDocumentTitle } from '../lib/title'
import { THEME_OPTIONS, fromApiTheme, toApiTheme, useThemeStore } from '../store/theme'
import { toast } from '../store/toast'
import { Alert } from '../components/ui/Alert'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { ConfirmDialog } from '../components/ui/ConfirmDialog'
import { Field } from '../components/ui/Field'
import { Select } from '../components/ui/Select'

/** 并发数表单范围：本项目统一为 1 ~ 16（参考实现的表单只允许 1 ~ 8） */
const CONCURRENT_TASKS_MIN = 1
const CONCURRENT_TASKS_MAX = 16
/** 界面里用于展示「命名规则预览」的示例文件名（与参考实现一致） */
const PREVIEW_SAMPLE = 'video.mp4'

/** 平台显示名 → 安装指引（逐字来自 ui-spec §4.7 面板 1） */
const INSTALL_GUIDES: Record<string, readonly string[]> = {
  Windows: [
    '下载 FFmpeg 完整构建包（如 gyan.dev 或 BtbN 的 release），解压到 C:\\ffmpeg。',
    '把 C:\\ffmpeg\\bin 加入系统 PATH，或直接在下面的「FFmpeg 可执行文件」里填写 ffmpeg.exe 的完整路径。',
    '也可以用包管理器：winget install Gyan.FFmpeg / choco install ffmpeg / scoop install ffmpeg。',
    '改完 PATH 需要重启本应用，再点上面的「重新检测」。',
  ],
  macOS: [
    '推荐 Homebrew：brew install ffmpeg（会自动带上 ffprobe）。',
    'Apple 芯片装到 /opt/homebrew/bin，Intel 机器装到 /usr/local/bin。',
    '装完点上面的「重新检测」；若仍未找到，可在下面直接指定完整路径。',
  ],
  Linux: [
    'Debian/Ubuntu：sudo apt install ffmpeg',
    'Fedora/RHEL：sudo dnf install ffmpeg',
    'Arch：sudo pacman -S ffmpeg',
    '装完点上面的「重新检测」；若用 Docker，请确认容器镜像里也装了 FFmpeg。',
  ],
}

/** FFmpeg 查找顺序（逐字来自 ui-spec §4.7 面板 1 的「查找顺序说明」） */
const LOOKUP_ORDER: Record<string, readonly string[]> = {
  Windows: [
    '设置里填写的路径（优先）',
    '应用所在目录下的内置副本：ffmpeg.exe、ffmpeg/ffmpeg.exe、tools/ffmpeg/ffmpeg.exe',
    '系统 PATH',
    'C:\\ffmpeg\\bin\\ffmpeg.exe、C:\\ProgramData\\chocolatey\\bin\\ffmpeg.exe',
    '%LOCALAPPDATA%\\Microsoft\\WinGet\\Links\\ffmpeg.exe、%USERPROFILE%\\scoop\\shims\\ffmpeg.exe',
  ],
  macOS: [
    '设置里填写的路径（优先）',
    '应用所在目录下的内置副本：ffmpeg、ffmpeg/ffmpeg、tools/ffmpeg/ffmpeg',
    '系统 PATH',
    '/opt/homebrew/bin/ffmpeg（Apple 芯片 Homebrew）',
    '/usr/local/bin/ffmpeg、/opt/local/bin/ffmpeg（Intel Homebrew / MacPorts）',
  ],
  Linux: [
    '设置里填写的路径（优先）',
    '应用所在目录下的内置副本：ffmpeg、ffmpeg/ffmpeg、tools/ffmpeg/ffmpeg',
    '系统 PATH',
    '/usr/bin/ffmpeg、/usr/local/bin/ffmpeg',
    '/usr/lib/jellyfin-ffmpeg/ffmpeg（对 QSV/VAAPI 支持更完整）',
    '/snap/bin/ffmpeg、/var/lib/flatpak/exports/bin/ffmpeg',
  ],
}

/** 编码器族 → 中文组名（取 `ffmpeg/encoder_catalog.rs` 的 display_name） */
const FAMILY_LABELS: Record<string, string> = {
  nvenc: 'NVIDIA NVENC',
  qsv: 'Intel Quick Sync Video',
  amf: 'AMD AMF',
  videotoolbox: 'Apple VideoToolbox',
  vaapi: 'Linux VA-API',
  vulkan: 'Vulkan Video',
  mf: 'Windows Media Foundation',
  software: '软件编码（CPU）',
}

/** 族的固定排序（与参考实现 FamilyOrder 一致） */
const FAMILY_ORDER = ['nvenc', 'qsv', 'amf', 'videotoolbox', 'vaapi', 'vulkan', 'mf', 'software']

/** 文件名里不允许出现的字符（参考实现用 `Path.GetInvalidFileNameChars()`，这里用跨平台的固定集合） */
const INVALID_FILE_NAME_CHARS = ['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\u0000']

/** 路径文本归一化：去首尾空白与粘贴时带上的引号（与服务端 `normalize_path_field` 一致） */
function normalizePathText(value: string | null | undefined): string {
  const trimmed = (value ?? '').trim()
  if (trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"')) {
    return trimmed.slice(1, -1).trim()
  }
  return trimmed
}

/** 后缀里的非法字符（返回展示用文案；合法时返回 null） */
function invalidSuffixChar(value: string): string | null {
  for (const char of INVALID_FILE_NAME_CHARS) {
    if (value.includes(char)) return char === '\u0000' ? '（空字符）' : char
  }
  return null
}

/** 只比较会持久化的字段（`id` 由服务端沿用，不参与「有未保存的更改」判定） */
function equalSettings(left: SettingsDto | null, right: SettingsDto | null): boolean {
  if (left === null || right === null) return left === right
  return (
    left.ffmpegPath === right.ffmpegPath &&
    left.ffprobePath === right.ffprobePath &&
    left.defaultOutputDirectory === right.defaultOutputDirectory &&
    left.outputNaming === right.outputNaming &&
    left.outputSuffix === right.outputSuffix &&
    left.fileExistsAction === right.fileExistsAction &&
    left.preferHardwareAcceleration === right.preferHardwareAcceleration &&
    left.preferHardwareDecoding === right.preferHardwareDecoding &&
    left.preferredHardwareEncoder === right.preferredHardwareEncoder &&
    left.autoFallbackToSoftware === right.autoFallbackToSoftware &&
    left.maxConcurrentTasks === right.maxConcurrentTasks &&
    left.maxLogLines === right.maxLogLines &&
    left.taskHistoryRetentionDays === right.taskHistoryRetentionDays &&
    left.theme === right.theme &&
    left.globalFfmpegArguments === right.globalFfmpegArguments &&
    left.notifyOnCompletion === right.notifyOnCompletion &&
    left.autoOpenBrowser === right.autoOpenBrowser &&
    left.listenPort === right.listenPort
  )
}

/** 「文件已存在时的处理方式」的四条帮助文案（逐字，ui-spec §4.7 面板 4） */
const FILE_EXISTS_HELP: Record<FileExistsAction, string> = {
  Ask: '转换前在界面询问你；批量处理时自动改为重命名，避免卡住等你确认。',
  Overwrite: '直接覆盖同名文件（已有输出的内容会丢失）。命令里会自动加 -y，不会等待确认。',
  Skip: '保留已有文件、跳过该文件的转换，任务会标记为「已跳过」。',
  Rename: '自动换成不冲突的新名字（如 video (1).mp4），原文件和已有输出都保留。',
}

/** 编码器分组（按族聚合，供首选编码器下拉与分组展示） */
interface EncoderGroupView {
  key: string
  label: string
  encoders: HardwareEncoderDto[]
  availableCount: number
  totalCount: number
}

/* ── 小型展示组件（只在本页复用）────────────────────────────────────────── */

/** 等宽路径胶囊 */
function CodePath({ value }: { value: string }) {
  return (
    <span
      className="mono break-all rounded-sm px-1 py-0.5 text-xs"
      style={{ background: 'var(--code-bg)', color: 'var(--text-primary)' }}
    >
      {value}
    </span>
  )
}

/** 环境信息的「键 + 值」两列网格行（父级是 grid，所以这里返回的是两个兄弟节点） */
function KvRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <span style={{ color: 'var(--text-muted)' }}>{label}</span>
      <span className="flex flex-wrap items-center gap-1.5 break-all">{children}</span>
    </>
  )
}

/** 弱化小字 */
function Muted({ children }: { children: ReactNode }) {
  return (
    <span className="text-xs" style={{ color: 'var(--text-muted)' }}>
      {children}
    </span>
  )
}

/** 「复选框 + 粗体标签 + 说明」三件套（`.check-field`） */
function CheckField({
  id,
  label,
  help,
  checked,
  disabled = false,
  onChange,
}: {
  id: string
  label: string
  help: ReactNode
  checked: boolean
  disabled?: boolean
  onChange: (checked: boolean) => void
}) {
  return (
    <div className="mt-3.5 flex items-start gap-2.5">
      <input
        id={id}
        type="checkbox"
        className="mt-1.5 flex-none"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <div className="min-w-0 flex-1">
        <label htmlFor={id} className="font-medium">
          {label}
        </label>
        <span className="mt-1 block text-xs" style={{ color: 'var(--text-muted)' }}>
          {help}
        </span>
      </div>
    </div>
  )
}

/** 路径输入 + 可选的「浏览…」按钮（参考实现的 FileSelector 的最简等价物） */
function PathField({
  id,
  label,
  value,
  placeholder,
  help,
  error,
  onChange,
  onBrowse,
}: {
  id: string
  label: string
  value: string
  placeholder: string
  help?: ReactNode
  error?: ReactNode
  onChange: (value: string) => void
  onBrowse?: () => void
}) {
  return (
    <Field label={label} htmlFor={id} help={help} error={error}>
      <div className="flex items-center gap-2">
        <input
          id={id}
          type="text"
          className="flex-1"
          value={value}
          placeholder={placeholder}
          onChange={(event) => onChange(event.target.value)}
        />
        {onBrowse ? (
          <Button size="sm" onClick={onBrowse} aria-label={`浏览并选择${label}`}>
            浏览…
          </Button>
        ) : null}
      </div>
    </Field>
  )
}

/** 路径测试结果（`ok` / `bad` 两种颜色） */
interface PathTestResult {
  /** 测试时输入框里的路径（一改输入即作废，见 ui-spec §4.7 的「结果过期规则」） */
  path: string
  ok: boolean
  message: string
}

/** 页面主体 */
export function SettingsPage() {
  useDocumentTitle(PAGE_TITLES.settings)

  const themeMode = useThemeStore((state) => state.mode)
  const setThemeMode = useThemeStore((state) => state.setMode)

  /* ── 表单 ─────────────────────────────────────────────────────────── */
  const [form, setForm] = useState<SettingsDto | null>(null)
  /** 服务端「已保存」快照（用于「有未保存的更改 / ✓ 已保存」判定） */
  const [saved, setSaved] = useState<SettingsDto | null>(null)
  const [isReady, setIsReady] = useState(false)
  const [loadFailed, setLoadFailed] = useState(false)
  const [isSaving, setIsSaving] = useState(false)
  const [isResetting, setIsResetting] = useState(false)
  const [isReloading, setIsReloading] = useState(false)
  /** `PUT /api/settings` 返回的服务端修正提示（逐字展示） */
  const [warnings, setWarnings] = useState<string[]>([])
  const [directoryError, setDirectoryError] = useState<{ directory: string; message: string } | null>(null)
  const [resetConfirmOpen, setResetConfirmOpen] = useState(false)
  const [clearConfirmOpen, setClearConfirmOpen] = useState(false)

  /* ── 环境信息 / 硬件 / 端口 ──────────────────────────────────────── */
  const [status, setStatus] = useState<AppStatusDto | null>(null)
  const [paths, setPaths] = useState<SystemPathsDto | null>(null)
  const [health, setHealth] = useState<HealthDto | null>(null)
  const [isChecking, setIsChecking] = useState(false)

  const [report, setReport] = useState<HardwareReportDto | null>(null)
  const [isScanning, setIsScanning] = useState(true)
  const [scanElapsedSeconds, setScanElapsedSeconds] = useState(0)
  const scanStartedAtRef = useRef(0)

  const [port, setPort] = useState<SettingsPortDto | null>(null)

  /* ── 路径测试 / 历史清理 ─────────────────────────────────────────── */
  const [ffmpegTest, setFfmpegTest] = useState<PathTestResult | null>(null)
  const [ffprobeTest, setFfprobeTest] = useState<PathTestResult | null>(null)
  const [isTestingFFmpeg, setIsTestingFFmpeg] = useState(false)
  const [isTestingFFprobe, setIsTestingFFprobe] = useState(false)

  const [isCleaningHistory, setIsCleaningHistory] = useState(false)
  const [isClearingHistory, setIsClearingHistory] = useState(false)
  const [historyMessage, setHistoryMessage] = useState<string | null>(null)

  /* ── 加载 ─────────────────────────────────────────────────────────── */

  const loadPort = useCallback(async () => {
    try {
      setPort(await settingsApi.port())
    } catch {
      // 端口信息只是辅助展示，读不到就不显示（保存后还会再试）
      setPort(null)
    }
  }, [])

  const applyThemeFromSettings = useCallback((theme: string) => {
    // 主题偏好：后端是 `System/Light/Dark`，store 用小写；立即生效 + 写 localStorage
    setThemeMode(fromApiTheme(theme))
  }, [setThemeMode])

  const loadSettings = useCallback(
    async (notify: boolean) => {
      try {
        const result = await settingsApi.get()
        setForm(result)
        setSaved(result)
        setLoadFailed(false)
        applyThemeFromSettings(result.theme)
        if (notify) toast.success('设置已重新读取')
      } catch (error) {
        setLoadFailed(true)
        toast.error(`读取设置失败：${getErrorMessage(error)}`)
      } finally {
        setIsReady(true)
      }
    },
    [applyThemeFromSettings],
  )

  /** 「重新读取设置」按钮：读取失败后才出现（成功 / 失败都有可见反馈） */
  const reloadSettings = useCallback(async () => {
    setIsReloading(true)
    try {
      await loadSettings(true)
    } finally {
      setIsReloading(false)
    }
  }, [loadSettings])

  const loadEnvironment = useCallback(
    async (force: boolean, notify: boolean) => {
      setIsChecking(true)
      try {
        const [statusResult, pathsResult, healthResult] = await Promise.all([
          systemApi.status(force),
          systemApi.paths(),
          systemApi.health(),
        ])
        setStatus(statusResult)
        setPaths(pathsResult)
        setHealth(healthResult)
        if (notify) toast.success('环境信息已重新检测')
      } catch (error) {
        toast.error(`环境检测失败：${getErrorMessage(error)}`)
      } finally {
        setIsChecking(false)
      }
    },
    [],
  )

  const loadHardware = useCallback(async (force: boolean) => {
    setIsScanning(true)
    scanStartedAtRef.current = Date.now()
    setScanElapsedSeconds(0)
    try {
      const result = await systemApi.encoders(force)
      setReport(result)
      if (force) {
        const seconds = ((Date.now() - scanStartedAtRef.current) / 1000).toFixed(1)
        const count = result?.encoders.filter((encoder) => encoder.isAvailable).length ?? 0
        toast.success(`硬件检测完成：${count} 个可用硬件编码器（耗时 ${seconds} 秒）`)
        if (count === 0) {
          toast.warning('本机没有检测到可用的硬件编码器，转换会使用 CPU 软件编码。')
        }
      }
    } catch (error) {
      toast.error(`硬件编码器检测失败：${getErrorMessage(error)}`)
    } finally {
      setIsScanning(false)
    }
  }, [])

  useEffect(() => {
    // 读取设置 / 环境信息属于「与外部系统同步」：setState 只发生在 await 之后，
    // 不会在 effect 同步阶段触发级联渲染，故显式关闭该启发式规则。
    // oxlint-disable-next-line react/set-state-in-effect
    void loadSettings(false)
    void loadEnvironment(false, false)
    void loadPort()
  }, [loadSettings, loadEnvironment, loadPort])

  // 硬件检测首次可能耗时数十秒：首屏之后再做，期间面板给「检测中…」反馈
  useEffect(() => {
    // 同上：硬件检测是异步的（首次可能数十秒）
    // oxlint-disable-next-line react/set-state-in-effect
    void loadHardware(false)
  }, [loadHardware])

  // 扫描期间每 0.4 秒刷新一次已用时长（参考实现同样每 400ms 走一次）
  useEffect(() => {
    if (!isScanning) return
    const timer = setInterval(() => {
      setScanElapsedSeconds((Date.now() - scanStartedAtRef.current) / 1000)
    }, 400)
    return () => clearInterval(timer)
  }, [isScanning])

  /* ── 实时事件（共享 WS 事件总线）──────────────────────────────────── */

  // 硬件检测结果（其它页触发的检测也会推过来）
  useEffect(() => onWsEvent('encoders.detected', (event) => setReport(event.data)), [])

  const dirty = !equalSettings(form, saved)

  // 设置被别处改动：表单没有未保存改动时同步过来（有改动时不订阅，避免覆盖正在编辑的内容）
  useEffect(() => {
    if (dirty) return
    return onWsEvent('settings.changed', (event) => {
      setForm(event.data)
      setSaved(event.data)
      applyThemeFromSettings(event.data.theme)
      void loadPort()
    })
  }, [dirty, applyThemeFromSettings, loadPort])

  /* ── 校验 ─────────────────────────────────────────────────────────── */

  const validationErrors = useMemo(() => {
    if (!form) return [] as string[]
    const errors: string[] = []

    if (form.outputNaming === 'Suffix' && form.outputSuffix.trim().length === 0) {
      errors.push('输出文件后缀：选择「添加后缀」时不能为空。')
    } else {
      const bad = invalidSuffixChar(form.outputSuffix)
      if (bad !== null) errors.push(`输出文件后缀：包含文件名不允许的字符「${bad}」。`)
    }

    // 参考实现的表单限制是 1 ~ 8；本项目统一采用模型层的 1 ~ 16（见文件头注释）
    if (form.maxConcurrentTasks < CONCURRENT_TASKS_MIN || form.maxConcurrentTasks > CONCURRENT_TASKS_MAX) {
      errors.push(`同时执行的转换任务数：必须在 ${CONCURRENT_TASKS_MIN} ~ ${CONCURRENT_TASKS_MAX} 之间。`)
    }
    if (form.maxLogLines < 50 || form.maxLogLines > 20000) {
      errors.push('单个任务保留的日志行数：必须在 50 ~ 20000 之间。')
    }
    if (form.taskHistoryRetentionDays < 0 || form.taskHistoryRetentionDays > 3650) {
      errors.push('任务历史保留天数：必须在 0 ~ 3650 之间。')
    }
    if (form.listenPort < 0 || form.listenPort > 65535) {
      errors.push('监听端口：必须在 0 ~ 65535 之间（0 表示自动选择空闲端口）。')
    }
    if (form.theme !== 'System' && form.theme !== 'Light' && form.theme !== 'Dark') {
      errors.push('主题：取值无效，请重新选择。')
    }
    if (
      directoryError !== null &&
      directoryError.directory === form.defaultOutputDirectory.trim()
    ) {
      errors.push(`默认输出目录：${directoryError.message}`)
    }

    return errors
  }, [form, directoryError])

  const canSave = isReady && !loadFailed && !isSaving && form !== null && validationErrors.length === 0

  /* ── 保存 / 恢复默认 ─────────────────────────────────────────────── */

  const saveSettings = useCallback(async () => {
    if (!form || !canSave) return
    setIsSaving(true)
    try {
      // 默认输出目录可写性放在保存时检查（服务端会尝试创建并写测试文件，不适合每次输入都调用）
      const directory = form.defaultOutputDirectory.trim()
      if (directory.length > 0) {
        let writable = true
        try {
          const check = await filesApi.check({ path: directory, kind: 'folder' })
          writable = Boolean(check?.writable)
        } catch (error) {
          // 目录检查接口本身不可用时不要把保存整个卡住：以服务端规范化后的结果为准
          console.warn('[settings] 默认输出目录可写性检查失败：', getErrorMessage(error))
        }
        if (!writable) {
          setDirectoryError({
            directory,
            message: `不可写或无法创建：${directory}。请换一个目录，或先创建它并授予写权限。`,
          })
          toast.error('默认输出目录不可写，设置未保存。')
          return
        }
      }
      setDirectoryError(null)

      const result = await settingsApi.update(form)
      // 服务端会 Normalize()：以返回的设置重新同步表单，避免界面与实际不一致
      setForm(result.settings)
      setSaved(result.settings)
      const corrected = result.warnings ?? []
      setWarnings(corrected)
      if (corrected.length > 0) {
        toast.warning(`设置已保存，服务端修正了 ${corrected.length} 项，请核对提示。`)
      } else {
        toast.success('设置已保存')
      }
      applyThemeFromSettings(result.settings.theme)
      await loadPort()
    } catch (error) {
      toast.error(`保存失败：${getErrorMessage(error)}`)
    } finally {
      setIsSaving(false)
    }
  }, [form, canSave, applyThemeFromSettings, loadPort])

  const resetToDefaults = useCallback(async () => {
    setIsResetting(true)
    try {
      const result = await settingsApi.reset()
      setForm(result)
      setSaved(result)
      setWarnings([])
      setFfmpegTest(null)
      setFfprobeTest(null)
      setDirectoryError(null)
      setHistoryMessage(null)
      applyThemeFromSettings(result.theme)
      await loadPort()
      toast.success('已恢复默认设置')
    } catch (error) {
      toast.error(`恢复默认失败：${getErrorMessage(error)}`)
    } finally {
      setIsResetting(false)
      setResetConfirmOpen(false)
    }
  }, [applyThemeFromSettings, loadPort])

  /* ── 主题（立即生效 + 只记住 theme 字段）──────────────────────────── */

  const changeTheme = useCallback(
    async (next: ApiThemeMode) => {
      setThemeMode(fromApiTheme(next))
      setForm((previous) => (previous ? { ...previous, theme: next } : previous))

      try {
        const stored = await settingsApi.get()
        if (stored.theme !== next) {
          // 只写回主题：不整份保存表单，避免把用户尚未保存的其它改动一起写进去
          const result = await settingsApi.update({ ...stored, theme: next })
          setSaved(result.settings)
          setForm((previous) => (previous ? { ...previous, theme: result.settings.theme } : previous))
        }
      } catch (error) {
        toast.warning(`主题已在本页生效，但没能记住（保存设置时会一起写入）：${getErrorMessage(error)}`)
      }
    },
    [setThemeMode],
  )

  /* ── 路径测试 ─────────────────────────────────────────────────────── */

  const testFFmpeg = useCallback(async () => {
    if (!form) return
    const typed = normalizePathText(form.ffmpegPath)
    setIsTestingFFmpeg(true)
    setFfmpegTest(null)
    try {
      const result = await systemApi.validateFFmpeg(typed)
      const source = typed.length > 0 ? `路径「${typed}」` : '系统 PATH'
      if (result?.valid && result.version) {
        setFfmpegTest({ path: typed, ok: true, message: `✓ 可用（${source}）：${result.version}` })
        toast.success('FFmpeg 可用')
      } else {
        setFfmpegTest({
          path: typed,
          ok: false,
          message:
            typed.length > 0
              ? `✗ 不可用：无法执行「${typed}」（文件不存在、没有执行权限，或不是 FFmpeg 可执行文件）。`
              : '✗ 不可用：系统 PATH 中没有找到可执行的 ffmpeg，请在上面填写完整路径。',
        })
        toast.error('FFmpeg 测试未通过')
      }
    } catch (error) {
      setFfmpegTest({ path: typed, ok: false, message: `✗ 测试出错：${getErrorMessage(error)}` })
      toast.error(`测试 FFmpeg 出错：${getErrorMessage(error)}`)
    } finally {
      setIsTestingFFmpeg(false)
    }
  }, [form])

  const testFFprobe = useCallback(async () => {
    if (!form) return
    const typed = normalizePathText(form.ffprobePath)
    const resolved = status?.ffprobePath ?? ''
    const target = typed.length > 0 ? typed : resolved
    setIsTestingFFprobe(true)
    setFfprobeTest(null)
    try {
      // 契约里只有 ffmpeg 的版本校验；ffprobe 只做「文件是否存在且可读」（与参考实现一致）
      const check = target.length > 0 ? await filesApi.check({ path: target, kind: 'file' }) : null
      if (check?.exists && check?.accessible) {
        setFfprobeTest({
          path: typed,
          ok: true,
          message: `✓ 文件存在且可读：${target}（只检查了文件是否可访问，没有真的执行 ffprobe）`,
        })
      } else {
        const targetNote =
          typed.length > 0 ? `「${typed}」不是可读取的文件` : '输入框为空，应用也还没有解析出 ffprobe 路径'
        setFfprobeTest({
          path: typed,
          ok: false,
          message:
            `✗ 未找到可用的 ffprobe：${targetNote}。` +
            '（本次只做了文件检查，没有执行 ffprobe；ffprobe 通常与 ffmpeg 一起安装，可填写 ffmpeg 同目录下的 ffprobe）',
        })
      }
    } catch (error) {
      setFfprobeTest({ path: typed, ok: false, message: `✗ 检查出错：${getErrorMessage(error)}` })
      toast.error(`测试 ffprobe 出错：${getErrorMessage(error)}`)
    } finally {
      setIsTestingFFprobe(false)
    }
  }, [form, status])

  /** 用服务端的原生选择器挑路径（`POST /api/files/dialog`） */
  const browsePath = useCallback(
    async (
      kind: 'file' | 'folder',
      title: string,
      current: string,
      apply: (path: string) => void,
    ) => {
      try {
        const result = await filesApi.dialog({
          kind,
          title,
          initialDirectory: normalizePathText(current) || undefined,
        })
        if (result?.path) apply(result.path)
        else if (result?.reason === 'unsupported') toast.warning('当前环境不支持原生文件选择器，请手动填写路径。')
        else if (result?.reason === 'timeout') toast.warning('文件选择器等待超时，请重试或手动填写路径。')
        // cancelled / 空路径：用户主动取消，不打扰
      } catch (error) {
        toast.error(`打开文件选择器失败：${getErrorMessage(error)}`)
      }
    },
    [],
  )

  /* ── 通知权限 ─────────────────────────────────────────────────────── */

  const requestNotificationPermission = useCallback(async () => {
    if (typeof Notification === 'undefined') {
      toast.warning('当前浏览器不支持系统通知：勾选会保留，但转换完成时不会弹出提醒。')
      return
    }
    try {
      const permission = await Notification.requestPermission()
      if (permission === 'granted') {
        toast.success('浏览器已允许通知：任务完成后会弹出提醒。')
      } else {
        toast.warning(
          '浏览器没有授予通知权限：勾选会保留，但转换完成时不会弹出系统通知。可在浏览器地址栏的站点设置里重新允许。',
        )
      }
    } catch (error) {
      toast.error(`请求通知权限失败：${getErrorMessage(error)}`)
    }
  }, [])

  /* ── 历史清理 ─────────────────────────────────────────────────────── */

  const cleanupHistory = useCallback(async () => {
    const days = form?.taskHistoryRetentionDays ?? 0
    if (days <= 0) {
      toast.warning('保留天数为 0，没有可清理的时间范围；请先填写天数，或使用「清空全部历史」。')
      return
    }
    setIsCleaningHistory(true)
    try {
      const threshold = new Date(Date.now() - days * 24 * 60 * 60 * 1000)
      const result = await historyApi.cleanup(days)
      const deleted = result?.deletedTasks ?? 0
      setHistoryMessage(
        `已清理 ${formatDateTime(threshold)} 之前的记录：删除 ${deleted} 条（保留天数 ${days} 天）。`,
      )
      toast.success(deleted > 0 ? `已清理 ${deleted} 条历史记录` : '没有需要清理的历史记录')
    } catch (error) {
      setHistoryMessage(null)
      toast.error(`清理历史失败：${getErrorMessage(error)}`)
    } finally {
      setIsCleaningHistory(false)
    }
  }, [form])

  const clearHistory = useCallback(async () => {
    setIsClearingHistory(true)
    try {
      const result = await historyApi.clear()
      const deleted = result?.deletedTasks ?? 0
      setHistoryMessage(`已清空历史记录，共删除 ${deleted} 条；运行中和等待中的任务已保留。`)
      toast.success(`已清空 ${deleted} 条历史记录`)
    } catch (error) {
      toast.error(`清空历史失败：${getErrorMessage(error)}`)
    } finally {
      setIsClearingHistory(false)
      setClearConfirmOpen(false)
    }
  }, [])

  /* ── 派生展示值 ───────────────────────────────────────────────────── */

  const platform = status?.platform ?? paths?.platform ?? ''
  const installGuide = INSTALL_GUIDES[platform] ?? INSTALL_GUIDES.Linux
  const lookupOrder = LOOKUP_ORDER[platform] ?? LOOKUP_ORDER.Linux

  const availableEncoders = useMemo(() => {
    if (!report) return [] as HardwareEncoderDto[]
    return report.encoders
      .filter((encoder) => encoder.isAvailable && isHardwareEncoder(encoder))
      .sort(
        (left, right) =>
          familyRank(left.family) - familyRank(right.family) ||
          left.displayName.localeCompare(right.displayName),
      )
  }, [report])

  const encoderGroups = useMemo<EncoderGroupView[]>(() => {
    if (!report) return []
    const buckets = new Map<string, HardwareEncoderDto[]>()
    for (const encoder of report.encoders) {
      const key = (encoder.family || 'other').toLowerCase()
      const list = buckets.get(key) ?? []
      list.push(encoder)
      buckets.set(key, list)
    }

    return [...buckets.entries()]
      .map(([key, encoders]) => {
        const sorted = [...encoders].sort(
          (left, right) =>
            Number(right.isAvailable) - Number(left.isAvailable) ||
            left.displayName.localeCompare(right.displayName),
        )
        return {
          key,
          label: key === 'other' ? '其他编码器' : (FAMILY_LABELS[key] ?? key),
          encoders: sorted,
          availableCount: sorted.filter((encoder) => encoder.isAvailable).length,
          totalCount: sorted.length,
        }
      })
      .sort(
        (left, right) =>
          Number(right.availableCount > 0) - Number(left.availableCount > 0) ||
          familyRank(left.key) - familyRank(right.key) ||
          left.label.localeCompare(right.label),
      )
  }, [report])

  const hasAvailableHardware = availableEncoders.length > 0
  const preferredEncoderUnavailable =
    (form?.preferredHardwareEncoder ?? '').trim().length > 0 &&
    !availableEncoders.some(
      (encoder) =>
        encoder.name.toLowerCase() === (form?.preferredHardwareEncoder ?? '').trim().toLowerCase(),
    )

  const suffixPreview =
    form?.outputNaming === 'Original'
      ? `${PREVIEW_SAMPLE} → ${PREVIEW_SAMPLE}（保持原名）`
      : `${PREVIEW_SAMPLE} → video${form?.outputSuffix ?? ''}.mp4`

  const ffmpegTestShown =
    ffmpegTest !== null && normalizePathText(form?.ffmpegPath) === ffmpegTest.path ? ffmpegTest : null
  const ffprobeTestShown =
    ffprobeTest !== null && normalizePathText(form?.ffprobePath) === ffprobeTest.path ? ffprobeTest : null

  /* ── 渲染 ─────────────────────────────────────────────────────────── */

  return (
    <div className="ffui-container mx-auto flex flex-col gap-5" style={{ maxWidth: '1020px' }}>
      <h1 className="m-0">⚙️ 设置</h1>
      <p className="-mt-3 mb-0 text-sm" style={{ color: 'var(--text-muted)' }}>
        所有设置都保存在本机数据目录的数据库里，不会上传；修改后需要点击页面底部的「保存设置」才会写入。
      </p>

      {!isReady ? (
        <div className="ffui-loading-row">
          <span className="spinner" aria-hidden="true" /> 正在读取设置…
        </div>
      ) : (
        <>
          {/* ── 1 · 🩺 环境信息 ─────────────────────────────────────── */}
          <section className="ffui-panel">
            <h2 className="ffui-panel-title flex flex-wrap items-center gap-2">
              🩺 环境信息
              {isChecking ? <span className="spinner" aria-hidden="true" /> : null}
            </h2>

            <div className="flex flex-wrap items-center gap-3">
              <Button loading={isChecking} onClick={() => void loadEnvironment(true, true)}>
                {isChecking ? '检测中…' : '重新检测'}
              </Button>
              <Muted>重新读取 FFmpeg、ffprobe、硬件解码能力与数据目录状态（可能需要几秒）。</Muted>
            </div>

            {status === null && paths === null ? (
              <div className="ffui-loading-row">
                <span className="spinner" aria-hidden="true" /> 正在检测环境…
              </div>
            ) : (
              <div
                className="mt-3.5"
                style={{
                  display: 'grid',
                  gridTemplateColumns: 'max-content minmax(0, 1fr)',
                  gap: '0.45rem 1rem',
                  fontSize: '0.9rem',
                }}
              >
                <KvRow label="应用版本">v{health?.version ?? APP_VERSION}</KvRow>
                <KvRow label="平台">{platform || EMPTY_VALUE}</KvRow>

                <KvRow label="数据目录">
                  <CodePath value={paths?.dataDirectory ?? EMPTY_VALUE} />
                  {paths ? (
                    <Badge tone={paths.dataDirectoryWritable ? 'success' : 'danger'}>
                      {paths.dataDirectoryWritable ? '可写' : '不可写'}
                    </Badge>
                  ) : null}
                  {paths && !paths.dataDirectoryWritable ? (
                    <Muted>
                      设置与任务历史可能无法保存，请检查目录权限（可用环境变量 FFMPEGWEBUI_DATA_DIR 换一个位置）。
                    </Muted>
                  ) : null}
                </KvRow>

                <KvRow label="数据库路径">
                  <CodePath value={paths?.databasePath ?? EMPTY_VALUE} />
                </KvRow>

                {paths?.isContainer ? (
                  <KvRow label="运行环境">
                    <Badge tone="info">容器内运行</Badge>
                  </KvRow>
                ) : null}

                <KvRow label="FFmpeg">
                  {status?.ffmpegAvailable ? (
                    <>
                      <Badge tone="success">可用</Badge>
                      <span>{status.ffmpegVersion}</span>
                    </>
                  ) : (
                    <>
                      <Badge tone="danger">未找到</Badge>
                      <Muted>转换功能不可用，请先安装 FFmpeg（见下方指引）。</Muted>
                    </>
                  )}
                </KvRow>

                <KvRow label="FFmpeg 路径">
                  <CodePath value={status?.ffmpegPath || '尚未解析到 ffmpeg'} />
                  <Badge tone={(form?.ffmpegPath ?? '').trim().length > 0 ? 'info' : 'default'}>
                    {(form?.ffmpegPath ?? '').trim().length > 0 ? '来自设置' : '自动探测'}
                  </Badge>
                </KvRow>

                <KvRow label="ffprobe 路径">
                  <CodePath value={status?.ffprobePath || '尚未解析到 ffprobe'} />
                  {status && !status.ffmpegAvailable ? (
                    <Muted>FFmpeg 不可用时无法确认是否真的存在。</Muted>
                  ) : null}
                </KvRow>

                <KvRow label="硬件解码方式">
                  {status === null || status.hwaccels.length === 0 ? (
                    <Muted>当前 FFmpeg 未报告可用的硬件解码方式。</Muted>
                  ) : (
                    status.hwaccels.map((hwaccel) => (
                      <Badge key={hwaccel} tone="info">
                        {hwaccel}
                      </Badge>
                    ))
                  )}
                </KvRow>

                {status?.lastHardwareDetection ? (
                  <KvRow label="上次硬件检测">
                    <span>{formatDateTime(status.lastHardwareDetection)}</span>
                  </KvRow>
                ) : null}
              </div>
            )}

            {status ? (
              <div className="mt-3.5 flex flex-col gap-2">
                {status.notices.length === 0 ? (
                  <Alert tone="success">一切正常：FFmpeg 可用，未发现需要注意的问题。</Alert>
                ) : (
                  status.notices.map((notice, index) => (
                    <Alert key={index} tone={status.ffmpegAvailable ? 'warning' : 'danger'}>
                      {notice}
                    </Alert>
                  ))
                )}
              </div>
            ) : null}

            {status && !status.ffmpegAvailable ? (
              <div className="mt-3.5">
                <Alert tone="warning">
                  <strong>安装 FFmpeg（{platform || EMPTY_VALUE}）</strong>
                  <ol className="mt-1 list-decimal pl-5">
                    {installGuide.map((item) => (
                      <li key={item}>{item}</li>
                    ))}
                  </ol>
                  <div className="mt-2">
                    <span>会按顺序在这些位置查找：</span>
                    <ul className="mt-1 list-disc pl-5">
                      {lookupOrder.map((item) => (
                        <li key={item}>{item}</li>
                      ))}
                    </ul>
                  </div>
                </Alert>
              </div>
            ) : null}
          </section>

          {/* ── 2 · 🚀 硬件加速能力 ─────────────────────────────────── */}
          <section className="ffui-panel">
            <h2 className="ffui-panel-title flex flex-wrap items-center gap-2">
              🚀 硬件加速能力
              {report !== null && !isScanning ? (
                <Badge tone={hasAvailableHardware ? 'success' : 'warning'}>
                  {availableEncoders.length} 个可用硬件编码器
                </Badge>
              ) : null}
            </h2>

            <div className="mt-3 flex flex-wrap items-center gap-3">
              <Button
                variant="primary"
                loading={isScanning}
                onClick={() => void loadHardware(true)}
              >
                {isScanning ? '检测中…' : '重新检测硬件编码器'}
              </Button>
              {isScanning ? (
                <Muted>已用时 {scanElapsedSeconds.toFixed(1)} 秒，请勿刷新页面…</Muted>
              ) : report ? (
                <Muted>上次检测：{formatDateTime(report.detectedAt)}</Muted>
              ) : null}
            </div>
            <div className="mt-1.5">
              <Muted>
                检测会逐个真实试跑本机的硬件编码器（每个约 1~3 秒），通常需要几秒到十几秒，期间可以继续操作其它设置。
              </Muted>
            </div>

            {report === null ? (
              <div className="ffui-loading-row">
                <span className="spinner" aria-hidden="true" /> 正在读取硬件编码器列表…
              </div>
            ) : (
              <>
                {report.summary.trim().length > 0 ? (
                  <div className="mt-3.5">
                    <Alert tone="info">
                      {report.summary}
                      <br />
                      <Muted>
                        检测时间 {formatDateTime(report.detectedAt)}
                        {report.ffmpegVersion ? ` · FFmpeg ${report.ffmpegVersion}` : ''}
                      </Muted>
                    </Alert>
                  </div>
                ) : null}

                <div className="mt-3.5">
                  <Select
                    id="preferred-encoder"
                    label="首选硬件编码器"
                    value={form?.preferredHardwareEncoder ?? ''}
                    disabled={!hasAvailableHardware || form === null}
                    options={[
                      { value: '', label: '自动（按编码格式选择，推荐）' },
                      ...availableEncoders.map((encoder) => ({
                        value: encoder.name,
                        label: `${encoder.displayName}（${encoder.name}）`,
                      })),
                      ...(preferredEncoderUnavailable
                        ? [
                            {
                              value: form?.preferredHardwareEncoder ?? '',
                              label: `${form?.preferredHardwareEncoder ?? ''}（当前不可用）`,
                            },
                          ]
                        : []),
                    ]}
                    help={
                      hasAvailableHardware
                        ? '只影响「优先使用硬件编码」的模板；留空时由应用按 h264/hevc/av1 等编码格式自动挑选。'
                        : '本机没有可用的硬件编码器（或检测尚未完成），因此暂不能指定。转换会使用 CPU 软件编码。'
                    }
                    onChange={(event: ChangeEvent<HTMLSelectElement>) =>
                      setForm((previous) =>
                        previous ? { ...previous, preferredHardwareEncoder: event.target.value } : previous,
                      )
                    }
                  />
                </div>

                {preferredEncoderUnavailable ? (
                  <div className="mt-2 flex flex-wrap items-center gap-2">
                    <span className="text-sm" style={{ color: 'var(--danger-color)' }} role="alert">
                      ⚠ 已保存的首选编码器「{form?.preferredHardwareEncoder}」当前不可用（可能驱动未安装或 FFmpeg 未编译该编码器）。
                    </span>
                    <Button
                      size="sm"
                      onClick={() =>
                        setForm((previous) =>
                          previous ? { ...previous, preferredHardwareEncoder: '' } : previous,
                        )
                      }
                    >
                      改为「自动」
                    </Button>
                  </div>
                ) : null}
              </>
            )}

            {/* 下面三项是设置本身，与硬件检测是否完成无关，因此始终显示 */}
            <h3 className="ffui-section-title mt-4">硬件加速开关</h3>
            <CheckField
              id="prefer-hardware"
              label="优先使用硬件编码（GPU 编码）"
              checked={form?.preferHardwareAcceleration ?? false}
              disabled={form === null}
              onChange={(checked) =>
                setForm((previous) =>
                  previous ? { ...previous, preferHardwareAcceleration: checked } : previous,
                )
              }
              help="模板没有指定编码器时，自动选择本机可用的硬件编码器（NVENC / QSV / AMF / VideoToolbox / VA-API 等），速度更快、CPU 占用更低。"
            />
            <CheckField
              id="prefer-hwdecode"
              label="优先使用硬件解码（输入侧）"
              checked={form?.preferHardwareDecoding ?? false}
              disabled={form === null}
              onChange={(checked) =>
                setForm((previous) =>
                  previous ? { ...previous, preferHardwareDecoding: checked } : previous,
                )
              }
              help="硬件解码可以减轻 CPU 负担，但个别老驱动可能不稳定；如果转换出现花屏、卡住或崩溃，请关闭此项。"
            />
            <CheckField
              id="auto-fallback"
              label="硬件编码失败时自动改用软件编码重试"
              checked={form?.autoFallbackToSoftware ?? false}
              disabled={form === null}
              onChange={(checked) =>
                setForm((previous) =>
                  previous ? { ...previous, autoFallbackToSoftware: checked } : previous,
                )
              }
              help="硬件编码失败时自动改用软件编码重试，避免任务直接失败。只有在 FFmpeg 还没有写出有效数据时才会回退，因此不会产出损坏的文件。"
            />

            {report !== null && !hasAvailableHardware && report.encoders.length > 0 ? (
              <div className="mt-3.5">
                <Alert tone="warning">
                  未检测到可用的硬件编码器，转换会使用 CPU 软件编码（较慢但兼容性最好）。
                </Alert>
              </div>
            ) : null}

            {encoderGroups.length > 0 ? (
              <div className="mt-4 flex flex-col gap-4">
                {encoderGroups.map((group) => (
                  <div key={group.key}>
                    <h3 className="mb-1.5 flex flex-wrap items-center gap-2 text-base">
                      {group.label}
                      <Badge tone={group.availableCount > 0 ? 'success' : 'default'}>
                        {group.availableCount} / {group.totalCount} 可用
                      </Badge>
                    </h3>
                    <div className="flex flex-wrap gap-1.5">
                      {group.encoders.map((encoder) => (
                        <Badge
                          key={encoder.name}
                          tone={encoder.isAvailable ? 'success' : 'default'}
                          title={encoder.unavailableReason ?? encoder.name}
                        >
                          {encoder.displayName}
                          {encoder.isAvailable ? '' : '（不可用）'}
                        </Badge>
                      ))}
                    </div>
                  </div>
                ))}
              </div>
            ) : null}
          </section>

          {/* ── 3 · 📁 FFmpeg 路径 ──────────────────────────────────── */}
          <section className="ffui-panel">
            <h2 className="ffui-panel-title">📁 FFmpeg 路径</h2>

            <PathField
              id="ffmpeg-path"
              label="FFmpeg 可执行文件"
              value={form?.ffmpegPath ?? ''}
              placeholder="留空则自动探测（系统 PATH / 常见安装位置 / 内置副本）"
              help="示例：C:\ffmpeg\bin\ffmpeg.exe 或 /opt/homebrew/bin/ffmpeg。留空时会测试系统 PATH 中的 ffmpeg。"
              onChange={(value) =>
                setForm((previous) => (previous ? { ...previous, ffmpegPath: value } : previous))
              }
              onBrowse={() =>
                void browsePath('file', '选择 FFmpeg 可执行文件', form?.ffmpegPath ?? '', (path) =>
                  setForm((previous) => (previous ? { ...previous, ffmpegPath: path } : previous)),
                )
              }
            />

            <div className="mt-2 flex flex-wrap items-center gap-2 text-sm">
              <span style={{ color: 'var(--text-muted)' }}>当前实际使用：</span>
              <CodePath value={status?.ffmpegPath || '尚未解析到 ffmpeg'} />
              <Muted>（保存或「重新检测」后更新）</Muted>
            </div>

            <div className="mt-3 flex flex-wrap items-center gap-3">
              <Button loading={isTestingFFmpeg} onClick={() => void testFFmpeg()}>
                {isTestingFFmpeg ? '测试中…' : '测试 FFmpeg'}
              </Button>
              {ffmpegTestShown ? (
                <span
                  className="text-sm"
                  role="status"
                  style={{ color: ffmpegTestShown.ok ? 'var(--success-color)' : 'var(--danger-color)' }}
                >
                  {ffmpegTestShown.message}
                </span>
              ) : null}
            </div>

            <div
              className="my-4"
              style={{ height: '1px', background: 'var(--border-color)' }}
              aria-hidden="true"
            />

            <PathField
              id="ffprobe-path"
              label="ffprobe 可执行文件"
              value={form?.ffprobePath ?? ''}
              placeholder="留空则自动探测（通常与 ffmpeg 同目录）"
              help="读取输入文件的时长、分辨率等媒体信息需要 ffprobe（用 ffmpeg 本身无法可靠地拿到）。这里只能检查「文件是否存在且可读 / 是否在 PATH 中」，不会真的执行 ffprobe —— 如需确认真实可用，请用上面的「测试 FFmpeg」确认版本信息能正常解析。"
              onChange={(value) =>
                setForm((previous) => (previous ? { ...previous, ffprobePath: value } : previous))
              }
              onBrowse={() =>
                void browsePath('file', '选择 ffprobe 可执行文件', form?.ffprobePath ?? '', (path) =>
                  setForm((previous) => (previous ? { ...previous, ffprobePath: path } : previous)),
                )
              }
            />

            <div className="mt-2 flex flex-wrap items-center gap-2 text-sm">
              <span style={{ color: 'var(--text-muted)' }}>当前实际使用：</span>
              <CodePath value={status?.ffprobePath || '尚未解析到 ffprobe'} />
              <Muted>（保存或「重新检测」后更新）</Muted>
            </div>

            <div className="mt-3 flex flex-wrap items-center gap-3">
              <Button loading={isTestingFFprobe} onClick={() => void testFFprobe()}>
                {isTestingFFprobe ? '检查中…' : '测试 ffprobe'}
              </Button>
              {ffprobeTestShown ? (
                <span
                  className="text-sm"
                  role="status"
                  style={{ color: ffprobeTestShown.ok ? 'var(--success-color)' : 'var(--danger-color)' }}
                >
                  {ffprobeTestShown.message}
                </span>
              ) : null}
            </div>
          </section>

          {/* ── 4 · 📂 输出设置 ─────────────────────────────────────── */}
          <section className="ffui-panel">
            <h2 className="ffui-panel-title">📂 输出设置</h2>

            <PathField
              id="output-directory"
              label="默认输出目录"
              value={form?.defaultOutputDirectory ?? ''}
              placeholder="例如 /Users/you/Videos 或 D:\Output"
              help="新建转换/批量任务时预填的输出目录；留空时转换页面会要求你手动选择。保存时会检查这个目录能否创建并写入，不可写则拒绝保存。"
              error={
                directoryError !== null && directoryError.directory === (form?.defaultOutputDirectory ?? '').trim()
                  ? `✗ ${directoryError.message}`
                  : undefined
              }
              onChange={(value) =>
                setForm((previous) =>
                  previous ? { ...previous, defaultOutputDirectory: value } : previous,
                )
              }
              onBrowse={() =>
                void browsePath(
                  'folder',
                  '选择默认输出目录',
                  form?.defaultOutputDirectory ?? '',
                  (path) =>
                    setForm((previous) =>
                      previous ? { ...previous, defaultOutputDirectory: path } : previous,
                    ),
                )
              }
            />

            <div className="mt-4">
              <Select
                id="output-naming"
                label="输出文件命名规则"
                value={form?.outputNaming ?? 'Suffix'}
                disabled={form === null}
                options={[
                  { value: 'Original', label: '保持原名' },
                  { value: 'Suffix', label: '添加后缀' },
                ]}
                help={`示例：${suffixPreview}`}
                onChange={(event: ChangeEvent<HTMLSelectElement>) =>
                  setForm((previous) =>
                    previous
                      ? { ...previous, outputNaming: event.target.value as OutputNamingRule }
                      : previous,
                  )
                }
              />
            </div>

            {form?.outputNaming === 'Suffix' ? (
              <div className="mt-4">
                <Field
                  label="输出文件后缀"
                  htmlFor="output-suffix"
                  help={`常用值：_converted、_out、-1080p。预览：${suffixPreview}`}
                  error={
                    form.outputSuffix.trim().length === 0 ? (
                      '✗ 选择「添加后缀」时后缀不能为空，否则输出文件会与原文件同名。'
                    ) : invalidSuffixChar(form.outputSuffix) !== null ? (
                      `✗ 后缀包含文件名不允许的字符：${invalidSuffixChar(form.outputSuffix)}（不能包含 / \\ : * ? " < > | 等）`
                    ) : undefined
                  }
                >
                  <input
                    id="output-suffix"
                    type="text"
                    placeholder="_converted"
                    value={form.outputSuffix}
                    onChange={(event) =>
                      setForm((previous) =>
                        previous ? { ...previous, outputSuffix: event.target.value } : previous,
                      )
                    }
                  />
                </Field>
              </div>
            ) : null}

            <div className="mt-4">
              <Select
                id="exists-action"
                label="文件已存在时的处理方式"
                value={form?.fileExistsAction ?? 'Ask'}
                disabled={form === null}
                options={[
                  { value: 'Ask', label: '询问用户' },
                  { value: 'Overwrite', label: '覆盖' },
                  { value: 'Skip', label: '跳过' },
                  { value: 'Rename', label: '重命名' },
                ]}
                help={
                  form
                    ? (FILE_EXISTS_HELP[form.fileExistsAction] ?? '未知的处理方式，实际会按「重命名」执行。')
                    : ''
                }
                onChange={(event: ChangeEvent<HTMLSelectElement>) =>
                  setForm((previous) =>
                    previous
                      ? { ...previous, fileExistsAction: event.target.value as FileExistsAction }
                      : previous,
                  )
                }
              />
            </div>
          </section>

          {/* ── 5 · ⚡ 性能与日志 ───────────────────────────────────── */}
          <section className="ffui-panel">
            <h2 className="ffui-panel-title">⚡ 性能与日志</h2>

            <div className="mt-4">
              <Select
                id="max-concurrent"
                label="同时执行的转换任务数"
                value={String(form?.maxConcurrentTasks ?? 1)}
                disabled={form === null}
                options={Array.from(
                  { length: CONCURRENT_TASKS_MAX - CONCURRENT_TASKS_MIN + 1 },
                  (_, index) => ({
                    value: String(CONCURRENT_TASKS_MIN + index),
                    label: String(CONCURRENT_TASKS_MIN + index),
                  }),
                )}
                help="默认 1（串行）。大于 1 会同时跑多个 FFmpeg 进程：显卡/CPU 的编码器通常无法真正并行，多个任务容易互相抢占资源，反而每个都变慢；只有在批量处理大量小文件时可能略快。"
                onChange={(event: ChangeEvent<HTMLSelectElement>) =>
                  setForm((previous) =>
                    previous
                      ? { ...previous, maxConcurrentTasks: Number(event.target.value) }
                      : previous,
                  )
                }
              />
            </div>

            <div className="mt-4">
              <Field
                label="单个任务保留的日志行数"
                htmlFor="max-log-lines"
                help="允许范围 50 ~ 20000；超出后丢弃最早的行，数值越大占用内存越多。"
              >
                <input
                  id="max-log-lines"
                  type="number"
                  min={50}
                  max={20000}
                  step={50}
                  value={form?.maxLogLines ?? 500}
                  onChange={(event) =>
                    setForm((previous) =>
                      previous
                        ? { ...previous, maxLogLines: Number.parseInt(event.target.value, 10) || 0 }
                        : previous,
                    )
                  }
                />
              </Field>
            </div>

            <div className="mt-4">
              <Field
                label="任务历史保留天数"
                htmlFor="history-retention"
                help="应用每次启动时按这个天数清理一次历史记录（0 表示启动时不自动清理）。"
              >
                <input
                  id="history-retention"
                  type="number"
                  min={0}
                  max={3650}
                  value={form?.taskHistoryRetentionDays ?? 30}
                  onChange={(event) =>
                    setForm((previous) =>
                      previous
                        ? {
                            ...previous,
                            taskHistoryRetentionDays: Number.parseInt(event.target.value, 10) || 0,
                          }
                        : previous,
                    )
                  }
                />
              </Field>
            </div>

            <div className="mt-3 flex flex-wrap items-center gap-3">
              <Button
                loading={isCleaningHistory}
                disabled={isCleaningHistory || (form?.taskHistoryRetentionDays ?? 0) <= 0}
                onClick={() => void cleanupHistory()}
              >
                {isCleaningHistory ? '清理中…' : '立即清理历史'}
              </Button>
              <Button variant="danger" disabled={isClearingHistory} onClick={() => setClearConfirmOpen(true)}>
                清空全部历史
              </Button>
              {(form?.taskHistoryRetentionDays ?? 0) <= 0 ? (
                <Muted>保留天数填的是 0（不按天数清理），要清空请用「清空全部历史」。</Muted>
              ) : null}
            </div>
            <div className="mt-1.5">
              <Muted>「立即清理历史」按上面填写的天数执行一次（使用当前输入值，不需要先保存设置）。</Muted>
            </div>

            {historyMessage !== null ? (
              <div className="mt-3">
                <Alert tone="info">{historyMessage}</Alert>
              </div>
            ) : null}

            <details
              className="mt-4 rounded-sm px-3 py-2.5"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--border-color)' }}
            >
              <summary className="cursor-pointer font-medium">全局 FFmpeg 参数（高级）</summary>
              <div className="mt-3">
                <Field
                  label="附加到每条 FFmpeg 命令的参数"
                  htmlFor="global-args"
                  help="留空表示不追加。参数会被插入到输出文件之前，对每一次转换都生效。"
                >
                  <input
                    id="global-args"
                    type="text"
                    placeholder="-threads 4"
                    value={form?.globalFfmpegArguments ?? ''}
                    onChange={(event) =>
                      setForm((previous) =>
                        previous ? { ...previous, globalFfmpegArguments: event.target.value } : previous,
                      )
                    }
                  />
                </Field>
              </div>
              <div className="mt-3">
                <Alert tone="warning">
                  这些参数会附加到每一条命令，写错会让所有转换都失败；请先确认语法。不要在这里写 -y / -n，覆盖策略由「文件已存在时的处理方式」自动处理。
                </Alert>
              </div>
            </details>
          </section>

          {/* ── 6 · 🎨 界面与通知 ───────────────────────────────────── */}
          <section className="ffui-panel">
            <h2 className="ffui-panel-title">🎨 界面与通知</h2>

            <div className="mt-4">
              <Select
                id="theme-select"
                label="主题"
                value={themeMode}
                options={THEME_OPTIONS.map((option) => ({ value: option.value, label: option.label }))}
                help="切换后立即生效并自动记住，下次打开时保持（不需要点「保存设置」，也不需要刷新页面）。"
                onChange={(event: ChangeEvent<HTMLSelectElement>) =>
                  void changeTheme(toApiTheme(event.target.value as 'light' | 'dark' | 'system'))
                }
              />
            </div>

            <CheckField
              id="notify-completion"
              label="任务完成后发送浏览器通知"
              checked={form?.notifyOnCompletion ?? false}
              disabled={form === null}
              onChange={(checked) => {
                setForm((previous) => (previous ? { ...previous, notifyOnCompletion: checked } : previous))
                if (checked) void requestNotificationPermission()
              }}
              help="勾选时浏览器会弹出权限请求；如果权限被拒绝或在设置里被关闭，通知不会出现（可在浏览器地址栏的站点设置里重新允许）。"
            />

            <CheckField
              id="auto-open-browser"
              label="完成后自动打开浏览器"
              checked={form?.autoOpenBrowser ?? false}
              disabled={form === null}
              onChange={(checked) =>
                setForm((previous) => (previous ? { ...previous, autoOpenBrowser: checked } : previous))
              }
              help="应用启动时自动打开浏览器窗口；如果浏览器已经在运行，可能只是切到已有窗口。"
            />

            <div className="mt-4">
              <Field
                label="监听端口"
                htmlFor="listen-port"
                help="修改后需要重启应用才会生效；0 表示自动挑选一个空闲端口。实际端口的优先级为：命令行 --port > 环境变量 FFMPEGWEBUI_PORT > appsettings.json 的 FFmpegWebUI:ListenPort > 这里保存的值。只要上面几项都没有显式设置，这里改的就是实际使用的端口。"
              >
                <input
                  id="listen-port"
                  type="number"
                  min={0}
                  max={65535}
                  value={form?.listenPort ?? 0}
                  onChange={(event) =>
                    setForm((previous) =>
                      previous
                        ? { ...previous, listenPort: Number.parseInt(event.target.value, 10) || 0 }
                        : previous,
                    )
                  }
                />
              </Field>
            </div>

            {/* 已保存 vs 当前生效端口（`GET /api/settings/port`）；重启提示写在上面的字段帮助里 */}
            <div className="mt-2 flex flex-wrap items-center gap-2">
              {port !== null ? (
                <Muted>
                  已保存端口 {port.savedListenPort} · 当前生效端口 {port.effectiveListenPort}
                  {port.restartRequired ? '（重启应用后生效）' : ''}
                </Muted>
              ) : (
                <Muted>端口信息读取中…</Muted>
              )}
            </div>
          </section>

          {/* ── 7 · 底部行动栏 ─────────────────────────────────────── */}
          <div
            className="sticky bottom-0 z-20 mt-6 flex flex-col gap-3 rounded-md p-4"
            role="group"
            aria-label="设置操作"
            style={{
              background: 'var(--card-bg)',
              border: '1px solid var(--border-color)',
              borderRadius: 'var(--radius-md)',
              boxShadow: 'var(--shadow-md)',
            }}
          >
            {loadFailed ? (
              <Alert tone="danger">
                ⛔ 设置读取失败，为避免覆盖已有配置，保存已禁用。请先点击「重新读取设置」。
              </Alert>
            ) : validationErrors.length > 0 ? (
              <Alert tone="danger">
                <strong>有 {validationErrors.length} 项需要修正，保存已禁用：</strong>
                <ul className="mt-1 list-disc pl-5">
                  {validationErrors.map((message) => (
                    <li key={message}>{message}</li>
                  ))}
                </ul>
              </Alert>
            ) : null}

            {warnings.length > 0 ? (
              <Alert tone="warning">
                <strong>设置已保存，服务端做了以下修正：</strong>
                <ul className="mt-1 list-disc pl-5">
                  {warnings.map((message) => (
                    <li key={message}>{message}</li>
                  ))}
                </ul>
              </Alert>
            ) : null}

            <div className="flex flex-wrap items-center gap-3">
              <Button
                size="lg"
                variant="primary"
                loading={isSaving}
                disabled={!canSave}
                onClick={() => void saveSettings()}
              >
                {isSaving ? '保存中…' : '💾 保存设置'}
              </Button>
              <Button
                size="lg"
                loading={isResetting}
                disabled={isSaving || isResetting}
                onClick={() => setResetConfirmOpen(true)}
              >
                🔄 恢复默认
              </Button>
              {loadFailed ? (
                <Button loading={isReloading} onClick={() => void reloadSettings()}>
                  重新读取设置
                </Button>
              ) : null}

              {!loadFailed ? (
                dirty ? (
                  <Badge tone="warning">● 有未保存的更改</Badge>
                ) : (
                  <Badge tone="success">✓ 已保存</Badge>
                )
              ) : null}

              <Muted>设置只写在本机数据目录，不会上传。</Muted>
            </div>
          </div>
        </>
      )}

      <ConfirmDialog
        open={resetConfirmOpen}
        onOpenChange={setResetConfirmOpen}
        title="恢复默认设置"
        message="确定把全部设置恢复为默认值吗？"
        detail="FFmpeg 路径、输出规则、硬件加速、性能与界面设置都会被覆盖；任务历史不受影响。"
        confirmText="恢复默认"
        danger
        busy={isResetting}
        onConfirm={() => void resetToDefaults()}
        onCancel={() => setResetConfirmOpen(false)}
      />

      <ConfirmDialog
        open={clearConfirmOpen}
        onOpenChange={setClearConfirmOpen}
        title="清空全部历史"
        message="确定删除所有已结束的任务记录吗？"
        detail="删除后无法恢复；正在运行和排队中的任务会保留。"
        confirmText="清空历史"
        danger
        busy={isClearingHistory}
        onConfirm={() => void clearHistory()}
        onCancel={() => setClearConfirmOpen(false)}
      />
    </div>
  )
}

/** 族名 → 固定排序（未知族排在最后） */
function familyRank(family: string): number {
  const index = FAMILY_ORDER.indexOf((family || '').toLowerCase())
  return index < 0 ? FAMILY_ORDER.length : index
}
