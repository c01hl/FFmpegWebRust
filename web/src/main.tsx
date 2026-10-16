import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App'
import { initTheme } from './store/theme'

// 挂载前先把主题写对：<html class="dark"> + data-theme + data-theme-mode。
// index.html 里的首屏内联脚本已经做过一次（防深色闪白），这里再校正一次，
// 并把 prefers-color-scheme 监听挂上（跟随系统时才同步）。
initTheme()

const container = document.getElementById('root')
if (!container) {
  throw new Error('找不到 #root 挂载点（index.html 被改动过？）')
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
