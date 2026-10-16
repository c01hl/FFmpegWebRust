/**
 * 页面标题（ui-spec §1「页面标题」表，逐字）。
 * React 版没有 Blazor 的 `<PageTitle>`，统一用 `useDocumentTitle` 写到 `document.title`。
 *
 * 用法（页面组件里）：
 *   useDocumentTitle(PAGE_TITLES.home)      // 或 useDocumentTitle('首页')
 */

import { useEffect } from 'react'

/** 应用名（Services/AppInfo.cs:7 的等价常量） */
export const APP_NAME = 'FFmpeg WebUI'
/** 版本（Services/AppInfo.cs:10 的等价常量） */
export const APP_VERSION = '2.0.0'

/** 各页面标题原文（`页面名 - FFmpeg WebUI`） */
export const PAGE_TITLES = {
  /** 布局默认标题 */
  default: APP_NAME,
  home: `首页 - ${APP_NAME}`,
  convert: `转换 - ${APP_NAME}`,
  batch: `批量处理 - ${APP_NAME}`,
  templates: `模板管理 - ${APP_NAME}`,
  templatesAdmin: `预设模板管理 - ${APP_NAME}`,
  tasks: `任务列表 - ${APP_NAME}`,
  /** 注意：设置页标题硬编码，不是 @AppInfo.Name（Settings.razor:15） */
  settings: '设置 - FFmpeg WebUI',
  error: `出错了 - ${APP_NAME}`,
  notFound: `页面不存在 - ${APP_NAME}`,
} as const

export type PageTitleKey = keyof typeof PAGE_TITLES

/** 直接写 document.title（非组件环境也能用，例如路由守卫） */
export function setDocumentTitle(title: string): void {
  if (typeof document !== 'undefined') document.title = title
}

/** 页面组件用：挂载/标题变化时写 document.title */
export function useDocumentTitle(title: string): void {
  useEffect(() => {
    setDocumentTitle(title)
  }, [title])
}
