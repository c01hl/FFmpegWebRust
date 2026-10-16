/**
 * 全站格式化（对应 ui-spec §4.11「全站格式化规则总表」）。
 *
 * 「不变文化（InvariantCulture）等价」规则：**不要**使用 `toLocaleString()` / `Intl`，
 * 它们会跟着浏览器区域设置变化（中文区域可能输出 `85,5` 这种非法 CSS 宽度）。
 * 这里一律用 `toFixed()` 与手工拼接：小数点永远是 `.`，数字永远是拉丁数字。
 *
 * 所有函数都是纯函数、不抛异常：入参为 null/undefined/NaN/±Infinity 时都返回占位文本。
 *
 * ── 自检（本项目没配前端测试框架，这里用注释保留等价断言）─────────────────
 * 需要人工核验时，可以把本文件编译成 JS 后跑下面几行：
 *
 *   tsc src/lib/format.ts --outDir /tmp/ffui --module esnext --target es2022 --skipLibCheck
 *   node --input-type=module -e "
 *     import * as f from '/tmp/ffui/format.js'
 *     const eq = (a, b) => { if (a !== b) throw new Error(JSON.stringify(a) + ' != ' + JSON.stringify(b)) }
 *     eq(f.formatFileSize(0), '0 B');          eq(f.formatFileSize(-1), '0 B')
 *     eq(f.formatFileSize(1536), '1.5 KB');    eq(f.formatFileSize(2359296), '2.25 MB')
 *     eq(f.formatFileSize(1879048192), '1.75 GB')
 *     eq(f.formatPercent(85.4567), '85.5%');   eq(f.formatPercent(123), '100.0%')
 *     eq(f.formatPercent(NaN), '0.0%');        eq(f.formatPercent(33.333, 2), '33.33%')
 *     eq(f.formatSpeed(1.2345), '1.23x');      eq(f.formatSpeed(null), '—')
 *     eq(f.formatDuration(0), '-');            eq(f.formatDuration(187), '03:07')
 *     eq(f.formatDuration(3779), '1:02:59');   eq(f.formatProgressTime(3779), '01:02:59')
 *     eq(f.formatDetectionDuration(3.4), '3.4 秒'); eq(f.formatDetectionDuration(72), '1:12')
 *     eq(f.formatTestDuration(2100), '2.1 秒'); eq(f.formatTestDuration(0), '—')
 *     eq(f.formatDateTime('2025-09-28T06:35:02Z').length, 19)   // yyyy-MM-dd HH:mm:ss
 *     eq(f.formatDateTimeMinute('2025-09-28T06:35:02Z').length, 16)
 *     eq(f.formatDateTimeShort('2025-09-28T06:35:02Z').length, 11)
 *     eq(f.formatFileSize(null), '0 B');  eq(f.formatDuration(null), '-')
 *     console.log('ok')
 *   "
 *   （时间类断言只校验形状，具体值随本机时区变化。）
 * ────────────────────────────────────────────────────────────────────────
 */

/** 空值占位（U+2014） */
export const EMPTY_VALUE = '—'
/** 无耗时的占位（半角连字符） */
export const NO_DURATION = '-'

/** 把数字压到 0..100；NaN/±Infinity → 0（进度条必须照抄这条） */
export function clampPercent(value: number | null | undefined): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) return 0
  if (value < 0) return 0
  if (value > 100) return 100
  return value
}

/**
 * `0.##` 风格的定点数字：最多 `digits` 位小数、去掉尾随 0、去掉 `-0`。
 * 例：`toFixedTrim(1.5) === '1.5'`、`toFixedTrim(2) === '2'`、`toFixedTrim(1.005, 2) === '1.01'`
 */
export function toFixedTrim(value: number, digits = 2): string {
  if (!Number.isFinite(value)) return '0'
  const fixed = value.toFixed(digits)
  // 去掉尾随 0 与小数点本身；'-0' / '-0.00' 归一成 '0'
  const trimmed = fixed.includes('.') ? fixed.replace(/0+$/, '').replace(/\.$/, '') : fixed
  return trimmed === '-0' ? '0' : trimmed
}

/** 百分比：clamp(0,100) + 固定 1 位小数 + `%` → `85.5%` */
export function formatPercent(value: number | null | undefined, fractionDigits = 1): string {
  return `${clampPercent(value).toFixed(fractionDigits)}%`
}

/** 批量汇总用的百分比（`0.##`，不 clamp 到 100 之上也不补位）→ `33.33%` */
export function formatPercentCompact(value: number | null | undefined, fractionDigits = 2): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '0%'
  return `${toFixedTrim(Math.max(0, value), fractionDigits)}%`
}

/** 速度：`0.##` + `x` → `1.23x`；无值 → `—` */
export function formatSpeed(speed: number | null | undefined): string {
  if (typeof speed !== 'number' || !Number.isFinite(speed) || speed <= 0) return EMPTY_VALUE
  return `${toFixedTrim(speed, 2)}x`
}

function toClampedSeconds(seconds: number | null | undefined): number {
  // 上限取 int.MaxValue 秒（约 68 年）：既够用，又不会让时长换算溢出
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds <= 0) return 0
  return Math.min(seconds, 2147483647)
}

/**
 * 时长（秒）→ 文本（§4.11）：`<= 0 → "-"`；≥1 小时 `H:MM:SS`（**小时不补零**）；
 * 否则 `MM:SS`。例：`03:07`、`1:02:59`。
 */
export function formatDuration(seconds: number | null | undefined): string {
  const total = toClampedSeconds(seconds)
  if (total <= 0) return NO_DURATION
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const secs = Math.floor(total % 60)
  const mm = String(minutes).padStart(2, '0')
  const ss = String(secs).padStart(2, '0')
  return hours > 0 ? `${hours}:${mm}:${ss}` : `${mm}:${ss}`
}

/**
 * 进度条专用时长：与 ProgressBar.razor 的 `FormatTime` 一致——小时 > 0 时**补零两位**
 * 的 `H:MM:SS`，否则 `MM:SS`（即 `00:00`、`01:02:59`）。
 * （§4.11 的通用 `formatDuration` 小时不补零，两处不同是刻意的。）
 */
export function formatProgressTime(seconds: number | null | undefined): string {
  const total = toClampedSeconds(seconds)
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const secs = Math.floor(total % 60)
  const mm = String(minutes).padStart(2, '0')
  const ss = String(secs).padStart(2, '0')
  return hours > 0 ? `${String(hours).padStart(2, '0')}:${mm}:${ss}` : `${mm}:${ss}`
}

/** 已用 / 总时长 → `mm:ss / mm:ss` */
export function formatDurationPair(
  current: number | null | undefined,
  total: number | null | undefined,
): string {
  return `${formatProgressTime(current)} / ${formatProgressTime(total)}`
}

const FILE_SIZE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const

/**
 * 文件大小：`<= 0 → "0 B"`；按 1024 进位到 B/KB/MB/GB/TB/PB，保留**最多 2 位小数**。
 * 例：`0 B`、`1.5 KB`、`2.25 MB`、`1.75 GB`。
 */
export function formatFileSize(bytes: number | null | undefined): string {
  if (typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes <= 0) return '0 B'
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < FILE_SIZE_UNITS.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${toFixedTrim(value, 2)} ${FILE_SIZE_UNITS[unit]}`
}

/** 解析 RFC3339 时间串；非法值返回 null（服务端可能给 null） */
function parseDate(value: string | number | Date | null | undefined): Date | null {
  if (value === null || value === undefined || value === '') return null
  const date = value instanceof Date ? value : new Date(value)
  return Number.isNaN(date.getTime()) ? null : date
}

function pad2(value: number): string {
  return String(value).padStart(2, '0')
}

/** 详情用的本地时间 `yyyy-MM-dd HH:mm:ss` */
export function formatDateTime(value: string | number | Date | null | undefined): string {
  const date = parseDate(value)
  if (!date) return EMPTY_VALUE
  return (
    `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())} ` +
    `${pad2(date.getHours())}:${pad2(date.getMinutes())}:${pad2(date.getSeconds())}`
  )
}

/** 列表用的本地时间 `MM-dd HH:mm` */
export function formatDateTimeShort(value: string | number | Date | null | undefined): string {
  const date = parseDate(value)
  if (!date) return EMPTY_VALUE
  return `${pad2(date.getMonth() + 1)}-${pad2(date.getDate())} ${pad2(date.getHours())}:${pad2(date.getMinutes())}`
}

/** 表格/检测标记用的本地时间 `yyyy-MM-dd HH:mm` */
export function formatDateTimeMinute(value: string | number | Date | null | undefined): string {
  const date = parseDate(value)
  if (!date) return EMPTY_VALUE
  return (
    `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())} ` +
    `${pad2(date.getHours())}:${pad2(date.getMinutes())}`
  )
}

/** 硬件检测耗时：`< 60 秒 → "3.4 秒"`；`>= 60 秒 → "1:12"` */
export function formatDetectionDuration(seconds: number | null | undefined): string {
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds < 0) return EMPTY_VALUE
  if (seconds < 60) return `${seconds.toFixed(1)} 秒`
  return formatDuration(seconds)
}

/** 模板试运行耗时（后端给的是毫秒）：`<= 0 → "—"`；否则 `"2.1 秒"` */
export function formatTestDuration(durationMs: number | null | undefined): string {
  if (typeof durationMs !== 'number' || !Number.isFinite(durationMs) || durationMs <= 0) {
    return EMPTY_VALUE
  }
  return `${(durationMs / 1000).toFixed(1)} 秒`
}

/**
 * 长路径只显示文件名（配合 CSS 省略号 + `title` 全文）。
 * 同时兼容 Windows 的 `\` 分隔符（服务端可能返回 Windows 路径）。
 */
export function formatFileName(path: string | null | undefined): string {
  if (!path) return EMPTY_VALUE
  const normalized = path.replace(/\\/g, '/')
  const index = normalized.lastIndexOf('/')
  const name = index >= 0 ? normalized.slice(index + 1) : normalized
  return name || EMPTY_VALUE
}

/** 日志下载文件名用的本地时间戳 `yyyyMMdd-HHmmss` */
export function formatFileStamp(value: string | number | Date | null | undefined): string {
  const date = parseDate(value) ?? new Date()
  return (
    `${date.getFullYear()}${pad2(date.getMonth() + 1)}${pad2(date.getDate())}-` +
    `${pad2(date.getHours())}${pad2(date.getMinutes())}${pad2(date.getSeconds())}`
  )
}

/** 把 `mm:ss` / `H:MM:SS` 之类的格式化时长重新解析成秒（仅用于展示层排序，不做业务推断） */
export function parseClockText(text: string): number {
  const parts = text.split(':').map((part) => Number(part))
  if (parts.some((part) => !Number.isFinite(part))) return 0
  return parts.reduce((total, part) => total * 60 + part, 0)
}
