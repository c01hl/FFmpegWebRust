/**
 * 命令预览区块（ui-spec §5.1 / §5.6）。
 *
 * - 整行命令 = `ffmpeg ` + 预览命令（半角空格），命令内容**完全来自后端**
 *   （`POST /api/tasks/preview` 的 `commandPreview`），前端绝不自己拼 ffmpeg 命令；
 * - 默认 **自动换行**（`pre-wrap + break-all`），按钮文案 `不换行`；切到不换行后按钮文案 `自动换行`；
 * - 复制成功后按钮文案 2 秒内变 `✓ 已复制`；
 * - 复制失败文案逐字来自 ui-spec §5.1（`COPY_FAILURE_MESSAGES.command`）。
 */

import { useEffect, useRef, useState } from 'react'
import { COPY_FAILURE_MESSAGES, copyAndToast } from '../../lib/clipboard'
import { Button } from '../ui/Button'
import { Spinner } from '../ui/Spinner'

export interface CommandPreviewProps {
  /** 预览命令（不含 `ffmpeg ` 前缀）；空值时不渲染命令框 */
  command: string | null | undefined
  /** 区块标题，默认 `完整命令（可直接复制到终端执行）` */
  title?: string
  /** 正在向后端请求预览 */
  loading?: boolean
  /** 外部错误文案（例如 `生成命令预览失败：…`），会以红色提示条展示 */
  error?: string | null
}

export function CommandPreview({
  command,
  title = '完整命令（可直接复制到终端执行）',
  loading = false,
  error = null,
}: CommandPreviewProps) {
  const [wrap, setWrap] = useState(true)
  const [copied, setCopied] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    return () => {
      if (timer.current !== null) clearTimeout(timer.current)
    }
  }, [])

  const handleCopy = async () => {
    const ok = await copyAndToast(command ? `ffmpeg ${command}` : '', {
      successText: '命令已复制到剪贴板',
      emptyText: '还没有可复制的命令。',
      failureText: COPY_FAILURE_MESSAGES.command,
    })
    if (!ok) return
    setCopied(true)
    if (timer.current !== null) clearTimeout(timer.current)
    timer.current = setTimeout(() => setCopied(false), 2000)
  }

  return (
    <div className="rounded-md border border-line bg-code overflow-hidden">
      <div className="flex flex-wrap items-center justify-between gap-2 px-3 py-2 border-b border-line bg-surface-3">
        <span style={{ fontSize: '0.88rem' }}>{title}</span>
        <div className="flex items-center gap-1.5">
          {loading ? <Spinner label="正在生成命令预览" /> : null}
          <Button size="sm" onClick={() => void handleCopy()} disabled={loading || !command}>
            {copied ? '✓ 已复制' : '📋 复制'}
          </Button>
          <Button size="sm" onClick={() => setWrap((value) => !value)}>
            {wrap ? '不换行' : '自动换行'}
          </Button>
        </div>
      </div>

      {error ? (
        <div className="ffui-alert ffui-alert-danger m-3" role="alert">
          <span aria-hidden="true">❌</span>
          <div>{error}</div>
        </div>
      ) : null}

      {command ? (
        <pre
          className="m-0 px-3 py-3 mono"
          style={{
            fontSize: '0.82rem',
            lineHeight: 1.55,
            background: 'transparent',
            border: 'none',
            whiteSpace: wrap ? 'pre-wrap' : 'pre',
            wordBreak: wrap ? 'break-all' : 'normal',
            overflowX: wrap ? 'visible' : 'auto',
          }}
        >
          {`ffmpeg ${command}`}
        </pre>
      ) : (
        <div className="px-3 py-3 text-muted-text" style={{ fontSize: '0.85rem' }}>
          {loading ? '正在生成命令预览…' : '尚未生成命令预览。'}
        </div>
      )}
    </div>
  )
}
