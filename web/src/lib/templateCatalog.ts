/**
 * 模板子系统的本地常量与纯工具（模板管理页 / 预设模板管理页 / 模板编辑器共用）。
 *
 * 定位（对应 ui-spec §3.7–§3.9、§8、`docs/reference/templates.md` §2/§7/§9）：
 * - 这里只有**纯函数与静态数据**：清单拆分、参数名规范化、占位符目录、码点映射、
 *   保存前的规范化、导出文件名…；
 * - **不做业务判断**：模板可用性、校验结果、命令预览的最终形态都来自服务端
 *   （`available` / `unavailableReason` / `/api/templates/validate` / `/api/tasks/preview`）。
 *   唯一的例外是编辑器的「命令预览」：契约里目前**没有**「用未保存的草稿构建命令行」的端点
 *   （§11 的 `/api/command/preview` 仍是「建议」），所以这里提供一个标注清楚的本地近似展开，
 *   等后端提供端点后应立刻替换（见 `buildCommandPreview` 的注释）。
 */

import type {
  CommandTemplate,
  EncoderType,
  HardwareAccelerationMode,
  TemplateIssue,
  TemplateListItem,
  TemplateParameter,
  TemplateParameterKind,
} from '../api/types'

/* ══════════════════════════════════════════════════════════════════════════
   枚举 → 中文（逐字来自 ui-spec §3.7–§3.9）
   ══════════════════════════════════════════════════════════════════════════ */

/** 编码器族 → 族显示名（`ffmpeg::encoder_catalog::family_display_name` 的前端投影） */
export const ENCODER_FAMILY_NAMES: Readonly<Record<string, string>> = {
  nvenc: 'NVIDIA NVENC',
  qsv: 'Intel Quick Sync',
  amf: 'AMD AMF',
  videotoolbox: 'Apple VideoToolbox',
  vaapi: 'Linux VA-API',
  vulkan: 'Vulkan Video',
  mf: 'Windows Media Foundation',
  software: '软件编码',
}

/** 编码器类型 → 族标识（`EncoderType::id()`） */
export const ENCODER_TYPE_FAMILIES: Readonly<Record<EncoderType, string>> = {
  Software: 'software',
  Nvenc: 'nvenc',
  Qsv: 'qsv',
  Amf: 'amf',
  VideoToolbox: 'videotoolbox',
  Vaapi: 'vaapi',
  Vulkan: 'vulkan',
  MediaFoundation: 'mf',
  Other: 'other',
}

/** 族显示名；未知族原样返回 */
export function encoderFamilyDisplayName(family: string | null | undefined): string {
  const value = (family ?? '').trim()
  if (!value) return ''
  return ENCODER_FAMILY_NAMES[value.toLowerCase()] ?? value
}

/** 硬件加速策略下拉项（逐字 §3.9 区块 7） */
export const HARDWARE_MODE_OPTIONS: readonly { value: HardwareAccelerationMode; label: string }[] = [
  { value: 'None', label: '不使用（纯软件编码）' },
  { value: 'Auto', label: '自动（有硬件就用，没有回退软件）' },
  { value: 'Required', label: '必须使用硬件（没有就失败）' },
]

/** 硬件加速策略 → 详情页长文案（逐字 §3.8 第 6 节） */
export const HARDWARE_MODE_DETAIL_TEXT: Readonly<Record<HardwareAccelerationMode, string>> = {
  Required: '必须使用硬件编码器（本机不可用时该模板不可用）',
  Auto: '自动：优先使用硬件编码器，没有可用硬件时回退到软件编码',
  None: '不使用硬件加速（使用模板中指定的编码器）',
}

/** 参数控件类型下拉项（逐字 §3.9 区块 6） */
export const PARAMETER_KIND_OPTIONS: readonly { value: TemplateParameterKind; label: string }[] = [
  { value: 'Select', label: '下拉选择' },
  { value: 'Number', label: '数字输入' },
  { value: 'Text', label: '文本输入' },
  { value: 'Boolean', label: '开关（是 / 否）' },
]

/** 参数控件类型中文（只读展示用，逐字 §3.8 第 4 节表格） */
export const PARAMETER_KIND_TEXT: Readonly<Record<TemplateParameterKind, string>> = {
  Select: '下拉选择',
  Number: '数字',
  Boolean: '开关',
  Text: '文本',
}

/** 目标编码格式下拉项（逐字 §3.9 区块 7） */
export const CODEC_CHOICES: readonly { value: string; label: string }[] = [
  { value: 'h264', label: 'H.264 / AVC' },
  { value: 'hevc', label: 'H.265 / HEVC' },
  { value: 'av1', label: 'AV1' },
  { value: 'vp9', label: 'VP9' },
  { value: 'vp8', label: 'VP8' },
  { value: 'prores', label: 'ProRes' },
  { value: 'aac', label: 'AAC（音频）' },
  { value: 'mp3', label: 'MP3（音频）' },
  { value: 'opus', label: 'Opus（音频）' },
  { value: 'flac', label: 'FLAC（音频）' },
]

/** 「快速添加」预设参数（逐字 §3.9 区块 6） */
export interface ParameterPreset {
  key: string
  label: string
  kind: TemplateParameterKind
  displayLabel: string
  description: string
  defaultValue: string
  options: { value: string; label: string; description: string }[]
  min: number | null
  max: number | null
}

export const PARAMETER_PRESETS: readonly ParameterPreset[] = [
  {
    key: 'quality',
    label: '＋ 质量 quality',
    kind: 'Select',
    displayLabel: '质量',
    description: '数值越小画质越好、文件越大。留空则由当前编码器的最佳默认值决定。',
    defaultValue: '',
    options: [
      { value: '', label: '自动（推荐）', description: '' },
      { value: '18', label: '高画质', description: '' },
      { value: '23', label: '均衡', description: '' },
      { value: '28', label: '小体积', description: '' },
      { value: '32', label: '极小体积', description: '' },
    ],
    min: null,
    max: null,
  },
  {
    key: 'audio_bitrate',
    label: '＋ 音频码率 audio_bitrate',
    kind: 'Select',
    displayLabel: '音频码率',
    description: '',
    defaultValue: '128k',
    options: [
      { value: '96k', label: '96 kbps', description: '' },
      { value: '128k', label: '128 kbps', description: '' },
      { value: '192k', label: '192 kbps', description: '' },
      { value: '256k', label: '256 kbps', description: '' },
      { value: '320k', label: '320 kbps', description: '' },
    ],
    min: null,
    max: null,
  },
  {
    key: 'preset',
    label: '＋ 编码预设 preset',
    kind: 'Select',
    displayLabel: '编码预设',
    description: '',
    defaultValue: 'medium',
    options: [
      { value: 'ultrafast', label: 'ultrafast', description: '' },
      { value: 'fast', label: 'fast', description: '' },
      { value: 'medium', label: 'medium', description: '' },
      { value: 'slow', label: 'slow', description: '' },
      { value: 'veryslow', label: 'veryslow', description: '' },
    ],
    min: null,
    max: null,
  },
  {
    key: 'width',
    label: '＋ 宽度 width',
    kind: 'Number',
    displayLabel: '宽度',
    description: '按宽度等比缩放（高度自动取偶数）。',
    defaultValue: '',
    options: [],
    min: 160,
    max: 7680,
  },
  {
    key: 'watermark',
    label: '＋ 水印路径 watermark',
    kind: 'Text',
    displayLabel: '水印图片路径',
    description: '',
    defaultValue: '',
    options: [],
    min: null,
    max: null,
  },
]

/* ══════════════════════════════════════════════════════════════════════════
   内置占位符目录（逐字来自 ui-spec §3.9 区块 2 的表格）
   ══════════════════════════════════════════════════════════════════════════ */

export interface PlaceholderItem {
  /** 不含花括号 */
  name: string
  description: string
}

export interface PlaceholderGroup {
  title: string
  items: PlaceholderItem[]
}

export const PLACEHOLDER_GROUPS: readonly PlaceholderGroup[] = [
  {
    title: '路径类',
    items: [
      { name: 'input', description: '输入文件的完整路径' },
      { name: 'output', description: '输出文件的完整路径（由输入文件名 + 输出扩展名生成）' },
      { name: 'input_dir', description: '输入文件所在目录' },
      { name: 'input_name', description: '不带扩展名的输入文件名' },
      { name: 'input_filename', description: '含扩展名的输入文件名' },
      { name: 'input_ext', description: '输入扩展名（如 mp4，不含点）' },
      { name: 'output_dir', description: '输出文件所在目录' },
      { name: 'output_name', description: '不带扩展名的输出文件名' },
      { name: 'output_ext', description: '输出扩展名（如 mp4，不含点）' },
    ],
  },
  {
    title: '媒体信息类',
    items: [
      { name: 'duration', description: '输入文件时长（秒）' },
      { name: 'width', description: '输入视频宽度（像素）' },
      { name: 'height', description: '输入视频高度（像素）' },
      { name: 'fps', description: '输入视频帧率' },
      { name: 'vcodec', description: '输入视频编码格式名' },
      { name: 'acodec', description: '输入音频编码格式名' },
      { name: 'sample_rate', description: '音频采样率（没有音频信息时为 44100）' },
      { name: 'channels', description: '音频声道数（没有音频信息时为 2）' },
    ],
  },
  {
    title: '编码器类',
    items: [
      { name: 'encoder', description: '按目标格式与本机硬件自动选出的视频编码器名（如 h264_nvenc / libx264）' },
      { name: 'audio_encoder', description: '音频编码器名（默认 aac）' },
      { name: 'crf', description: '质量数值（等价于 quality，可被同名参数覆盖）' },
      { name: 'quality', description: '质量数值，通常由一个名为 quality 的参数驱动' },
      { name: 'q', description: '质量数值的简写' },
      {
        name: 'quality_args',
        description: '当前编码器的完整质量参数（-crf 23 / -cq 23 / -qp 23 …），需要配合 encoder 使用',
      },
      { name: 'preset_args', description: '当前编码器的预设参数（如 -preset medium），与 encoder 配套' },
      {
        name: 'hwupload_args',
        description: '显存上传滤镜（如 format=nv12,hwupload），VA-API 等需要；模板已写 -vf 时会自动并入',
      },
      {
        name: 'device_args',
        description: '编码器所需的设备初始化参数（如 -vaapi_device /dev/dri/renderD128），由系统自动插入',
      },
      { name: 'muxer_args', description: '容器兼容参数（如 HEVC 入 MP4 需要的 -tag:v hvc1），由系统自动插入' },
      { name: 'threads', description: '线程数（未设置时为空）' },
      { name: 'hwaccel', description: '硬件解码参数（如 -hwaccel videotoolbox，未启用时为空）' },
    ],
  },
  {
    title: '时间类',
    items: [
      { name: 'date', description: '当前日期 yyyyMMdd' },
      { name: 'time', description: '当前时间 HHmmss' },
      { name: 'datetime', description: '当前日期时间 yyyyMMdd_HHmmss' },
      { name: 'random', description: '随机 8 位十六进制字符串（避免输出文件重名）' },
    ],
  },
]

/** 内置占位符总数（`全部内置占位符说明（共 N 个）`） */
export const BUILT_IN_PLACEHOLDER_COUNT = PLACEHOLDER_GROUPS.reduce(
  (total, group) => total + group.items.length,
  0,
)

/** 内置占位符名集合（小写比较） */
export const BUILT_IN_PLACEHOLDER_NAMES: ReadonlySet<string> = new Set(
  PLACEHOLDER_GROUPS.flatMap((group) => group.items.map((item) => item.name)),
)

/** 占位符名 → 说明（找不到时用 §3.9 的兜底文案） */
export function placeholderDescription(name: string): string {
  for (const group of PLACEHOLDER_GROUPS) {
    const hit = group.items.find((item) => item.name.toLowerCase() === name.toLowerCase())
    if (hit) return hit.description
  }
  return '系统提供的占位符'
}

/** `{name}` 形式 */
export function braced(name: string): string {
  return `{${name}}`
}

/** 从命令参数里提取占位符名（等价 `CommandLineBuilder.ExtractPlaceholders` 的正则） */
export function extractPlaceholders(commandArgs: string | null | undefined): string[] {
  const text = commandArgs ?? ''
  const result: string[] = []
  const regex = /\{([A-Za-z_][A-Za-z0-9_]*)\}/g
  let match = regex.exec(text)
  while (match !== null) {
    result.push(match[1])
    match = regex.exec(text)
  }
  return result
}

/** 命令里是否用到了 encoder 类占位符（§4.5「含 encoder 类占位符」判定） */
export function usesEncoderPlaceholder(commandArgs: string | null | undefined): boolean {
  const text = commandArgs ?? ''
  return (['{encoder}', '{quality_args}', '{preset_args}', '{hwupload_args}'] as const).some((token) =>
    text.toLowerCase().includes(token),
  )
}

/* ══════════════════════════════════════════════════════════════════════════
   校验问题（§3.9 区块 5）
   ══════════════════════════════════════════════════════════════════════════ */

/** 严重度 → 展示样式（图标 / 标签 / Alert 色调） */
export const ISSUE_SEVERITY_STYLE: Readonly<
  Record<TemplateIssue['severity'], { icon: string; label: string; tone: 'danger' | 'warning' | 'info' }>
> = {
  Error: { icon: '❌', label: '错误', tone: 'danger' },
  Warning: { icon: '⚠️', label: '警告', tone: 'warning' },
  Info: { icon: 'ℹ️', label: '提示', tone: 'info' },
}

/** 严重度排序权重（Error 在前） */
export const ISSUE_SEVERITY_RANK: Readonly<Record<TemplateIssue['severity'], number>> = {
  Error: 0,
  Warning: 1,
  Info: 2,
}

/** 字段 → 中文标签（§3.9，服务端给的是 camelCase，这里按小写匹配） */
export const ISSUE_FIELD_LABELS: Readonly<Record<string, string>> = {
  name: '模板名称',
  commandargs: '命令参数',
  outputextension: '输出扩展名',
  category: '分类',
  parameters: '参数',
  requiredencoder: '所需编码器',
  requiredfilters: '所需滤镜',
}

/** 字段名 → 中文标签；没有映射时原样返回（用于高亮与提示） */
export function issueFieldLabel(field: string | null | undefined): string | null {
  if (!field) return null
  return ISSUE_FIELD_LABELS[field.toLowerCase()] ?? field
}

/** 按严重度降序排序（稳定：同严重度保持服务端顺序） */
export function sortIssues(issues: readonly TemplateIssue[]): TemplateIssue[] {
  return issues
    .map((issue, index) => ({ issue, index }))
    .sort((a, b) => {
      const left = ISSUE_SEVERITY_RANK[a.issue.severity] ?? 9
      const right = ISSUE_SEVERITY_RANK[b.issue.severity] ?? 9
      return left === right ? a.index - b.index : left - right
    })
    .map((entry) => entry.issue)
}

/** 统计各严重度数量 */
export function countIssues(issues: readonly TemplateIssue[]): { errors: number; warnings: number } {
  let errors = 0
  let warnings = 0
  for (const issue of issues) {
    if (issue.severity === 'Error') errors += 1
    else if (issue.severity === 'Warning') warnings += 1
  }
  return { errors, warnings }
}

/* ══════════════════════════════════════════════════════════════════════════
   清单文本 ↔ 数组（§3.9：分隔符集合与规范化）
   ══════════════════════════════════════════════════════════════════════════ */

/** 格式与标签通用的分隔符集合：`, ，、 空格 \t \r \n ; ；` */
const LIST_SEPARATOR = /[,，、;；\s]+/

/** 通用拆分：按分隔符切分、去空白、丢弃空项（保持原样的大小写） */
export function splitList(text: string | null | undefined): string[] {
  const value = (text ?? '').trim()
  if (!value) return []
  return value
    .split(LIST_SEPARATOR)
    .map((item) => item.trim())
    .filter((item) => item.length > 0)
}

/** 输入格式：去前导点、转小写、忽略大小写去重 */
export function normalizeFormatList(text: string | null | undefined): string[] {
  const seen = new Set<string>()
  const result: string[] = []
  for (const item of splitList(text)) {
    const value = item.replace(/^\.+/, '').toLowerCase()
    if (!value || seen.has(value)) continue
    seen.add(value)
    result.push(value)
  }
  return result
}

/** 标签：仅忽略大小写去重（保留原始大小写） */
export function normalizeTagList(text: string | null | undefined): string[] {
  const seen = new Set<string>()
  const result: string[] = []
  for (const item of splitList(text)) {
    const key = item.toLowerCase()
    if (seen.has(key)) continue
    seen.add(key)
    result.push(item)
  }
  return result
}

/** 数组 → 可编辑文本（`mp4, mkv`） */
export function listToText(items: readonly string[] | null | undefined): string {
  return (items ?? []).join(', ')
}

/** 参数名规范化：Trim → 去 `{}` → 转小写 → `-`/`.`/空格 → `_` → 只留 `a-z0-9_` → 去首尾 `_` */
export function normalizeParameterName(text: string | null | undefined): string {
  const value = (text ?? '').trim().replace(/[{}]/g, '').toLowerCase()
  const replaced = value.replace(/[-.\s]+/g, '_').replace(/[^a-z0-9_]/g, '')
  return replaced.replace(/^_+/, '').replace(/_+$/, '')
}

/** 参数名是否合法（小写字母/数字/下划线且不以数字开头） */
export function isValidParameterName(name: string | null | undefined): boolean {
  return /^[a-z_][a-z0-9_]*$/.test((name ?? '').trim())
}

/** 参数显示名（Label 为空时回退 Name） */
export function parameterDisplayLabel(parameter: TemplateParameter): string {
  const label = (parameter.label ?? '').trim()
  if (label) return label
  return (parameter.name ?? '').trim()
}

/** `质量、音频码率 等 3 项`（§3.7：前 3 个用 `、` 连接，超过 3 个追加 ` 等 N 项`） */
export function parameterLabelsText(parameters: readonly TemplateParameter[] | null | undefined): string {
  const labels = (parameters ?? [])
    .filter((parameter) => (parameter.name ?? '').trim() || (parameter.label ?? '').trim())
    .map((parameter) => parameterDisplayLabel(parameter))
  if (labels.length === 0) return ''
  const head = labels.slice(0, 3).join('、')
  return labels.length > 3 ? `${head} 等 ${labels.length} 项` : head
}

/** 可见参数（名字非空） */
export function visibleParameters(template: CommandTemplate | null | undefined): TemplateParameter[] {
  return (template?.parameters ?? []).filter((parameter) => (parameter.name ?? '').trim().length > 0)
}

/** 参数默认值字典（键忽略大小写；空名参数不进字典，§2.3） */
export function parameterDefaultValues(
  template: CommandTemplate | null | undefined,
): Record<string, string> {
  const values: Record<string, string> = {}
  for (const parameter of visibleParameters(template)) {
    values[parameter.name] = parameter.defaultValue ?? ''
  }
  return values
}

/** 忽略大小写读取参数值（不存在时回退 defaultValue） */
export function parameterValue(
  parameter: TemplateParameter,
  values: Readonly<Record<string, string>>,
): string {
  const key = Object.keys(values).find((item) => item.toLowerCase() === parameter.name.toLowerCase())
  if (key !== undefined) return values[key] ?? ''
  return parameter.defaultValue ?? ''
}

/** Boolean 参数的「真」判定（§2.1，仅影响复选框初始状态） */
export function isBooleanTrue(parameter: TemplateParameter, value: string): boolean {
  if (!value) return false
  const trueValue = parameter.trueValue ?? ''
  if (trueValue && value.toLowerCase() === trueValue.toLowerCase()) return true
  return ['1', 'true', 'yes', 'on'].includes(value.toLowerCase())
}

/** 参数是否被命令引用（§3.9 区块 6 的「已使用」判定） */
export function isParameterUsed(parameter: TemplateParameter, commandArgs: string): boolean {
  const name = (parameter.name ?? '').trim().toLowerCase()
  if (!name) return false
  const placeholders = new Set(extractPlaceholders(commandArgs).map((item) => item.toLowerCase()))
  if (placeholders.has(name)) return true
  if (name === 'quality' || name === 'q' || name === 'crf') {
    return ['quality_args', 'crf', 'quality', 'q'].some((item) => placeholders.has(item))
  }
  return false
}

/* ══════════════════════════════════════════════════════════════════════════
   列表筛选 / 列展示（§4.5、§4.6、§8.1）
   ══════════════════════════════════════════════════════════════════════════ */

function contains(source: string | null | undefined, search: string): boolean {
  if (!source) return false
  return source.toLowerCase().includes(search)
}

/**
 * 搜索匹配（忽略大小写）：名称 / 描述 / 分类 / 输出扩展名 / 所需编码器 / 标签。
 * 模板管理页用服务端的 `?q=` 过滤；预设管理页按 §4.6 在前端即时过滤。
 */
export function matchesTemplateSearch(
  template: CommandTemplate,
  search: string,
  extra: readonly (string | null | undefined)[] = [],
): boolean {
  const needle = search.trim().toLowerCase()
  if (!needle) return true
  if (contains(template.name, needle)) return true
  if (contains(template.description, needle)) return true
  if (contains(template.category, needle)) return true
  if (contains(template.outputExtension, needle)) return true
  if (contains(template.requiredEncoder, needle)) return true
  if (extra.some((value) => contains(value, needle))) return true
  return (template.tags ?? []).some((tag) => contains(tag, needle))
}

/** 预设管理页「编码器」列 / 卡片文案（§4.6 单元格内容） */
export function templateEncoderColumnText(template: CommandTemplate): string {
  if (template.hardwareMode === 'None') return '不使用硬件加速'
  const mode = template.hardwareMode === 'Required' ? '必须' : '自动'
  const required = (template.requiredEncoder ?? '').trim()
  const label = required ? encoderFamilyDisplayName(required) : '任意可用编码器'
  return `${mode}：${label}`
}

/** 分类显示名（空 → `未分类`） */
export function categoryDisplayName(category: string | null | undefined): string {
  const value = (category ?? '').trim()
  return value || '未分类'
}

/** 导出文件名 `ffmpegwebui-templates-yyyyMMdd-HHmmss.json`（§4.5） */
export function exportFileName(date: Date = new Date()): string {
  const pad = (value: number) => String(value).padStart(2, '0')
  const stamp =
    `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}` +
    `-${pad(date.getHours())}${pad(date.getMinutes())}${pad(date.getSeconds())}`
  return `ffmpegwebui-templates-${stamp}.json`
}

/** 触发一次本地下载（Blob + 隐藏 `<a download>`，§8.7） */
export function downloadTextFile(fileName: string, text: string, mime = 'application/json'): boolean {
  if (typeof document === 'undefined') return false
  let url: string | null = null
  try {
    const blob = new Blob([text], { type: `${mime};charset=utf-8` })
    url = URL.createObjectURL(blob)
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = fileName
    anchor.rel = 'noopener'
    anchor.style.display = 'none'
    document.body.appendChild(anchor)
    anchor.click()
    anchor.remove()
    return true
  } catch {
    return false
  } finally {
    if (url !== null) {
      // 让浏览器有时间开始下载再释放
      setTimeout(() => URL.revokeObjectURL(url as string), 10_000)
    }
  }
}

/* ══════════════════════════════════════════════════════════════════════════
   草稿：深拷贝 / 规范化（§3.9「保存前规范化」）
   ══════════════════════════════════════════════════════════════════════════ */

/** 深拷贝模板（编辑器一律在副本上编辑，绝不修改传入对象） */
export function cloneTemplate(template: CommandTemplate): CommandTemplate {
  return {
    ...template,
    supportedInputFormats: [...(template.supportedInputFormats ?? [])],
    requiredFilters: [...(template.requiredFilters ?? [])],
    tags: [...(template.tags ?? [])],
    parameters: (template.parameters ?? []).map((parameter) => ({
      ...parameter,
      options: (parameter.options ?? []).map((option) => ({ ...option })),
    })),
  }
}

/** 新建草稿（用户模板 / 系统预设） */
export function createEmptyTemplate(options: {
  type?: CommandTemplate['type']
  sortOrder?: number
} = {}): CommandTemplate {
  return {
    id: '',
    name: '',
    description: '',
    commandArgs: '',
    type: options.type ?? 'User',
    category: '',
    supportedInputFormats: [],
    outputExtension: '',
    targetCodec: '',
    requiresHardwareAcceleration: false,
    hardwareMode: 'None',
    requiredEncoder: null,
    parameters: [],
    requiredFilters: [],
    tags: [],
    isRecommended: false,
    presetVersion: 0,
    createdAt: '',
    updatedAt: '',
    sortOrder: options.sortOrder ?? 0,
  }
}

/** 保存前规范化（§3.9：名称去 `{}`、扩展名去点转小写、清单重新拆分、参数名规范化…） */
export function normalizeTemplateDraft(draft: CommandTemplate): CommandTemplate {
  const template = cloneTemplate(draft)
  const requiredEncoder = (template.requiredEncoder ?? '').trim()

  template.name = (template.name ?? '').trim().replace(/^\{+/, '').replace(/\}+$/, '').trim()
  template.description = (template.description ?? '').trim()
  template.commandArgs = (template.commandArgs ?? '').trim()
  template.category = (template.category ?? '').trim()
  template.outputExtension = (template.outputExtension ?? '').trim().replace(/^\.+/, '').toLowerCase()
  template.targetCodec = (template.targetCodec ?? '').trim()
  template.requiredEncoder = requiredEncoder.length > 0 ? requiredEncoder : null
  template.requiresHardwareAcceleration = template.hardwareMode === 'Required'

  if (template.hardwareMode === 'None') template.requiredEncoder = null

  template.parameters = (template.parameters ?? []).map((parameter) => {
    const next: TemplateParameter = {
      ...parameter,
      name: normalizeParameterName(parameter.name),
      label: (parameter.label ?? '').trim(),
      description: (parameter.description ?? '').trim(),
      defaultValue: parameter.defaultValue ?? '',
      trueValue: parameter.trueValue ?? '1',
      falseValue: parameter.falseValue ?? '',
      min: parameter.kind === 'Number' ? parameter.min : null,
      max: parameter.kind === 'Number' ? parameter.max : null,
      options:
        parameter.kind === 'Select'
          ? (parameter.options ?? []).map((option) => ({
              value: (option.value ?? '').trim(),
              label: (option.label ?? '').trim(),
              description: (option.description ?? '').trim(),
            }))
          : [],
    }
    next.label = next.label || next.name
    // 显示名为空的选项用值填充（§3.9 保存行为）
    next.options = next.options.map((option) => ({
      ...option,
      label: option.label || option.value,
    }))
    if (next.kind !== 'Select') next.options = []
    return next
  })

  template.sortOrder = Number.isFinite(template.sortOrder) ? Math.trunc(template.sortOrder) : 0
  return template
}

/** 重名检测（`excludeId` 用于编辑时排除自身） */
export function findDuplicateName(
  templates: readonly CommandTemplate[],
  name: string,
  excludeId?: string | null,
): CommandTemplate | null {
  const value = name.trim().toLowerCase()
  if (!value) return null
  return (
    templates.find(
      (template) =>
        template.id !== excludeId && (template.name ?? '').trim().toLowerCase() === value,
    ) ?? null
  )
}

/** 未声明占位符（命令里用到、但不是内置、也没有参数声明的名字；保持出现顺序、去重） */
export function undeclaredPlaceholders(
  commandArgs: string,
  parameters: readonly TemplateParameter[],
): string[] {
  const declared = new Set(parameters.map((parameter) => (parameter.name ?? '').trim().toLowerCase()))
  const result: string[] = []
  const seen = new Set<string>()
  for (const name of extractPlaceholders(commandArgs)) {
    const key = name.toLowerCase()
    if (BUILT_IN_PLACEHOLDER_NAMES.has(key) || declared.has(key) || seen.has(key)) continue
    seen.add(key)
    result.push(name)
  }
  return result
}

/* ══════════════════════════════════════════════════════════════════════════
   编辑器命令预览（本地近似展开，见文件头注释）
   ══════════════════════════════════════════════════════════════════════════ */

/** 预览用的占位路径（§3.8 / §5.3：尖括号是字面文本） */
export const PREVIEW_INPUT_PATH = '<输入文件>'
export const PREVIEW_OUTPUT_PATH = '<输出文件>'

/** 分词：按空白切分，双引号内的内容保持为同一个 token，引号本身丢弃（§4.3 的等价近似） */
function tokenizeCommandArgs(input: string): string[] {
  const tokens: string[] = []
  let current = ''
  let quoted = false
  let started = false
  for (const char of input) {
    if (char === '"') {
      quoted = !quoted
      started = true
      continue
    }
    if (!quoted && /\s/.test(char)) {
      if (started) tokens.push(current)
      current = ''
      started = false
      continue
    }
    current += char
    started = true
  }
  if (started) tokens.push(current)
  return tokens
}

/** 展示用引号：含空格的 token 用双引号包起来（§4.4 的等价近似） */
function quoteForDisplay(token: string): string {
  if (token === '') return '""'
  return /[\s"]/.test(token) ? `"${token.replace(/"/g, '\\"')}"` : token
}

function withoutExtension(fileName: string): string {
  const index = fileName.lastIndexOf('.')
  return index > 0 ? fileName.slice(0, index) : fileName
}

function extensionOf(fileName: string): string {
  const index = fileName.lastIndexOf('.')
  return index > 0 ? fileName.slice(index + 1) : ''
}

function directoryOf(path: string): string {
  const index = path.lastIndexOf('/')
  return index > 0 ? path.slice(0, index) : ''
}

/**
 * 编辑器内的「命令预览」（ui-spec §5.3）。
 *
 * ⚠️ **本地近似展开**：契约里没有「用未保存的草稿构建命令行」的端点
 * （`/api/command/preview` 目前只是 ui-spec §11 的「建议」），因此这里按 §5.3 的规则做最小展开：
 * - 参数值 = 各参数的 `defaultValue`（可由 `values` 覆盖）；
 * - `{input}` / `{output}` 等路径占位符 → 占位路径派生的假值；
 * - `{date}` / `{time}` / `{datetime}` / `{random}` → 当前时间；
 * - 其余内置占位符（`{encoder}` / `{quality_args}` …）**原样保留**，它们最终由后端在真实
 *   上下文里展开；
 * - 整个 token 展开成空字符串时该 token 消失（与 §2.2 一致）。
 *
 * 一旦后端提供 `/api/command/preview`，**必须**换成服务端结果（前端不重复实现业务逻辑）。
 */
export function buildCommandPreview(
  template: CommandTemplate,
  values?: Readonly<Record<string, string>>,
): string {
  const args = (template.commandArgs ?? '').trim()
  if (!args) return ''

  const parameterValues: Record<string, string> = {}
  for (const parameter of template.parameters ?? []) {
    const name = (parameter.name ?? '').trim()
    if (!name) continue
    parameterValues[name.toLowerCase()] = parameterValue(parameter, values ?? {})
  }

  const inputPath = PREVIEW_INPUT_PATH
  const outputPath = PREVIEW_OUTPUT_PATH
  const outputExtension = (template.outputExtension ?? '').trim().replace(/^\.+/, '') || 'mp4'
  const outputFileName = `${withoutExtension(outputPath)}.${outputExtension}`
  const overwritten = outputPath === PREVIEW_OUTPUT_PATH
  const now = new Date()
  const pad = (value: number) => String(value).padStart(2, '0')
  const resolvers: Record<string, string> = {
    input: inputPath,
    output: overwritten ? outputPath : outputFileName,
    input_dir: directoryOf(inputPath),
    input_name: withoutExtension(inputPath),
    input_filename: inputPath.slice(inputPath.lastIndexOf('/') + 1),
    input_ext: extensionOf(inputPath),
    output_dir: directoryOf(outputPath),
    output_name: withoutExtension(outputPath),
    output_ext: outputExtension,
    date: `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}`,
    time: `${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`,
    datetime:
      `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}_` +
      `${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`,
    random: Math.floor(Math.random() * 0xffffffff)
      .toString(16)
      .padStart(8, '0'),
  }

  const rendered: string[] = []
  for (const token of tokenizeCommandArgs(args)) {
    const whole = /^\{([A-Za-z_][A-Za-z0-9_]*)\}$/.exec(token)
    if (whole) {
      const name = whole[1].toLowerCase()
      if (name in resolvers) {
        const value = resolvers[name]
        if (value === '') continue // 整段消失
        rendered.push(quoteForDisplay(value))
        continue
      }
      if (name in parameterValues) {
        const value = parameterValues[name]
        if (value === '') continue
        rendered.push(quoteForDisplay(value))
        continue
      }
      rendered.push(token) // 内置占位符：交由后端展开
      continue
    }

    const substituted = token.replace(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g, (match, rawName: string) => {
      const name = rawName.toLowerCase()
      if (name in resolvers) return resolvers[name]
      if (name in parameterValues) return parameterValues[name]
      return match
    })
    rendered.push(quoteForDisplay(substituted))
  }

  return rendered.filter((token) => token !== '""').join(' ')
}

/** 模板详情/卡片用的「硬件徽标」文案与配色（§3.7 第 1 条） */
export interface TemplateAvailabilityBadge {
  text: string
  tone: 'success' | 'danger' | 'default'
  title: string
}

/** 可用性徽标：完全由服务端给出的 `available` / `unavailableReason` 推导（前端不算可用性） */
export function availabilityBadge(template: TemplateListItem): TemplateAvailabilityBadge {
  if (template.available) {
    return {
      text: '🚀 硬件可用',
      tone: 'success',
      title: template.requiredEncoder
        ? `将使用 ${encoderFamilyDisplayName(template.requiredEncoder)} 硬件编码`
        : '本机已检测到该模板可用的硬件编码器',
    }
  }
  return {
    text: '⚠ 硬件不可用',
    tone: 'danger',
    title: template.unavailableReason || '本机缺少该模板所需的硬件编码器',
  }
}
