/**
 * 主题 store（zustand）：light | dark | system。
 *
 * 参考实现里主题是「Cookie + html[data-theme]」（app.js:8-41、App.razor:41-45）。
 * React 版保留同样的语义，但把持久化换成 `localStorage['ffmpeg-theme']`（index.html 的首屏
 * 内联脚本也读这个键，避免深色系统下白色闪屏）：
 *
 *   写 <html class="dark">            —— Tailwind 的 `dark:` 变体靠它生效
 *   写 <html data-theme="light|dark">—— 设计令牌的深色块选择器之一（另一个是 .dark）
 *   写 <html data-theme-mode="light|dark|system"> —— 用户的三档原始选择
 *
 * 三档取值**小写**（`'system' | 'light' | 'dark'`），与 index.html 首屏脚本一致；
 * 而后端 `SettingsDto.theme` 用的是枚举名 `'System' | 'Light' | 'Dark'`，
 * 两者互转见 `toApiTheme` / `fromApiTheme`。
 *
 * 另外：跟随系统时监听 `prefers-color-scheme` 变化并同步（app.js:32-41 的 watchSystemTheme）。
 */

import { create } from 'zustand'
import type { ApiThemeMode } from '../api/types'

/** 本地三档取值（小写） */
export type ThemeMode = 'light' | 'dark' | 'system'
/** 解析后的实际主题 */
export type ResolvedTheme = 'light' | 'dark'

/** localStorage 键（index.html 首屏脚本硬编码了同一个键，改名必须同步） */
export const THEME_STORAGE_KEY = 'ffmpeg-theme'
/** data-theme-mode 的属性名 */
export const THEME_MODE_ATTR = 'data-theme-mode'
/** data-theme 的属性名 */
export const THEME_ATTR = 'data-theme'

const DARK_QUERY = '(prefers-color-scheme: dark)'

/** 后端枚举名 → 本地小写 */
export function fromApiTheme(value: string | null | undefined): ThemeMode {
  switch ((value ?? '').toLowerCase()) {
    case 'light':
      return 'light'
    case 'dark':
      return 'dark'
    default:
      return 'system'
  }
}

/** 本地小写 → 后端枚举名（`SettingsDto.theme`） */
export function toApiTheme(mode: ThemeMode): ApiThemeMode {
  if (mode === 'light') return 'Light'
  if (mode === 'dark') return 'Dark'
  return 'System'
}

/** 三档取值的中文选项文案（设置页下拉框，逐字） */
export const THEME_OPTIONS: ReadonlyArray<{ value: ThemeMode; label: string }> = [
  { value: 'system', label: '跟随系统' },
  { value: 'light', label: '浅色' },
  { value: 'dark', label: '深色' },
]

function prefersDark(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false
  return window.matchMedia(DARK_QUERY).matches
}

/** 读取已保存的取值；非法值回退 `system` */
export function readStoredTheme(): ThemeMode {
  if (typeof localStorage === 'undefined') return 'system'
  try {
    return fromApiTheme(localStorage.getItem(THEME_STORAGE_KEY))
  } catch {
    return 'system'
  }
}

/** 把解析结果写到 <html>（class + data-theme + data-theme-mode） */
export function applyThemeToDocument(mode: ThemeMode, resolved: ResolvedTheme): void {
  if (typeof document === 'undefined') return
  const root = document.documentElement
  root.classList.toggle('dark', resolved === 'dark')
  root.setAttribute(THEME_ATTR, resolved)
  root.setAttribute(THEME_MODE_ATTR, mode)
}

export interface ThemeState {
  /** 用户的三档选择 */
  mode: ThemeMode
  /** 解析后的实际主题（跟随系统时会随系统变化） */
  resolved: ResolvedTheme
  /** 设置并立即生效 + 持久化 */
  setMode: (mode: ThemeMode) => void
  /** 在 light/dark 之间切换（跟随系统时切到当前解析值的反面色） */
  toggle: () => void
  /** 用后端设置里的主题初始化（App 启动时调用一次；不改写 localStorage 以外的任何东西） */
  syncFromSettings: (apiTheme: string | null | undefined) => void
}

/** 解析三档取值 → 实际主题 */
export function resolveTheme(mode: ThemeMode): ResolvedTheme {
  if (mode === 'light') return 'light'
  if (mode === 'dark') return 'dark'
  return prefersDark() ? 'dark' : 'light'
}

const initialMode = readStoredTheme()

export const useThemeStore = create<ThemeState>((set, get) => ({
  mode: initialMode,
  resolved: resolveTheme(initialMode),

  setMode: (mode) => {
    const resolved = resolveTheme(mode)
    try {
      localStorage.setItem(THEME_STORAGE_KEY, mode)
    } catch {
      // 隐私模式下可能写不进去；不影响本次会话生效
    }
    applyThemeToDocument(mode, resolved)
    set({ mode, resolved })
  },

  toggle: () => {
    const next: ThemeMode = get().resolved === 'dark' ? 'light' : 'dark'
    get().setMode(next)
  },

  syncFromSettings: (apiTheme) => {
    const mode = fromApiTheme(apiTheme)
    // 与本地一致时不重写，避免多一次 DOM 抖动
    if (mode === get().mode) {
      applyThemeToDocument(mode, get().resolved)
      return
    }
    get().setMode(mode)
  },
}))

let watching = false

/**
 * 启动主题系统：应用一次当前取值，并注册 `prefers-color-scheme` 监听。
 * 幂等，`main.tsx` 里调用一次即可。
 */
export function initTheme(): void {
  const { mode } = useThemeStore.getState()
  useThemeStore.getState().setMode(mode) // 顺手把 class / data-theme 写对（首屏脚本可能没跑）
  if (watching) return
  watching = true
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
  const media = window.matchMedia(DARK_QUERY)
  const onChange = () => {
    const state = useThemeStore.getState()
    // 仅当处于「跟随系统」时才同步（app.js:32-41 的语义）
    if (state.mode !== 'system') return
    const resolved = resolveTheme('system')
    applyThemeToDocument('system', resolved)
    useThemeStore.setState({ resolved })
  }
  if (typeof media.addEventListener === 'function') media.addEventListener('change', onChange)
  else if (typeof media.addListener === 'function') media.addListener(onChange)
}

/** 非组件环境读当前解析结果 */
export function currentResolvedTheme(): ResolvedTheme {
  return useThemeStore.getState().resolved
}
