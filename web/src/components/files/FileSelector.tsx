/**
 * 单文件/文件夹路径选择器（对应 ui-spec §3.3 `FileSelector.razor`）。
 *
 * 重要设计变更（与参考实现一致）：浏览器拿不到拖入文件的绝对路径，
 * 所以**没有拖放能力**，界面上也不出现「拖拽文件到此处」的提示。
 *
 * 与 Blazor 版的差异（因为前端没有文件系统权限）：
 * - 存在性/可读性/文件大小改为调用 `POST /api/files/check`（服务端探测），
 *   不再在渲染线程做本地 IO；`ValidateExists=false` 时仍然探测大小（两者相互独立）。
 * - `浏览` 按钮调用 `POST /api/files/dialog`（服务端拉起原生对话框），
 *   后端描述来自 `GET /api/files/dialog/backend`。
 *
 * 输入行为与参考实现逐字一致：输入即时本地校验 + **350 ms 防抖**回调父组件；
 * 失焦/回车立即提交；提交前 Trim 并去掉首尾引号（支持「复制为路径」）。
 */

import { useCallback, useEffect, useId, useRef, useState } from 'react'
import { getErrorMessage } from '../../api/client'
import { filesApi } from '../../api/endpoints'
import { cx } from '../../lib/cx'
import { formatFileSize } from '../../lib/format'
import { resolveDialogBackend } from './dialogBackend'
import {
  FILE_DIALOG_MESSAGES,
  FILE_SELECTOR_ERRORS,
  INPUT_DEBOUNCE_MS,
} from './fileSelectorCopy'
import { toast } from '../../store/toast'

export interface FileSelectorProps {
  /** 中文标签，同时用于对话框标题 `选择{label}` */
  label?: string
  value: string
  onChange: (value: string) => void
  placeholder?: string
  showBrowse?: boolean
  showFileInfo?: boolean
  validateExists?: boolean
  /** 允许的扩展名（可带点）；仅在非目录且列表非空时校验 */
  allowedExtensions?: string[]
  isDirectory?: boolean
  disabled?: boolean
}

/** 去掉首尾引号 + Trim（支持资源管理器「复制为路径」带引号的情况） */
function normalize(text: string): string {
  let value = (text ?? '').trim()
  if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) {
    value = value.slice(1, -1).trim()
  }
  return value
}

/** 取父目录（同时兼容 Windows 的 `\`）；没有分隔符时返回空串 */
function parentDirectory(path: string): string {
  const normalized = path.replace(/\\/g, '/')
  const index = normalized.lastIndexOf('/')
  return index <= 0 ? '' : normalized.slice(0, index)
}

function extensionOf(path: string): string {
  const name = path.replace(/\\/g, '/').split('/').pop() ?? ''
  const index = name.lastIndexOf('.')
  if (index <= 0) return ''
  return name.slice(index + 1).toLowerCase()
}

export function FileSelector({
  label = '文件路径',
  value,
  onChange,
  placeholder = '输入文件路径，或点击「浏览」选择',
  showBrowse = true,
  showFileInfo = true,
  validateExists = true,
  allowedExtensions,
  isDirectory = false,
  disabled = false,
}: FileSelectorProps) {
  const uid = useId()
  const inputId = `ffui-file-${uid}-input`
  const errorId = `ffui-file-${uid}-error`
  const infoId = `ffui-file-${uid}-info`

  // 「父组件持有的值真的变化时才覆盖本地文本」：用渲染期调整状态的官方写法，
  // 避免 effect 里同步 setState（会把用户正在输入的中间状态改写掉）
  const [syncedValue, setSyncedValue] = useState(value)
  const [text, setText] = useState(value)
  const [error, setError] = useState<string | null>(null)
  const [fileSize, setFileSize] = useState(0)
  const [fileExists, setFileExists] = useState(false)
  const [isBrowsing, setIsBrowsing] = useState(false)

  const debounceTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const probeGeneration = useRef(0)
  const mounted = useRef(true)

  if (value !== syncedValue) {
    setSyncedValue(value)
    setText(value)
  }

  const cancelDebounce = useCallback(() => {
    if (debounceTimer.current !== null) {
      clearTimeout(debounceTimer.current)
      debounceTimer.current = null
    }
  }, [])

  useEffect(() => {
    return () => {
      mounted.current = false
      cancelDebounce()
    }
  }, [cancelDebounce])

  /**
   * 服务端探测：存在性 + 可读性 + 大小。
   * 所有 setState 都在 await 之后，避免 effect 同步阶段级联渲染。
   */
  const probe = useCallback(
    async (raw: string) => {
      cancelDebounce()
      const generation = ++probeGeneration.current
      const path = normalize(raw)

      if (path === '') {
        setError(null)
        setFileSize(0)
        setFileExists(false)
        return
      }

      let nextError: string | null = null
      let size = 0
      let exists = false

      try {
        const result = await filesApi.check({ path, kind: isDirectory ? 'folder' : 'file' })
        if (generation !== probeGeneration.current || !mounted.current) return
        size = result.size ?? 0
        exists = Boolean(result.exists) || size > 0

        if (validateExists) {
          if (isDirectory) {
            if (!result.exists) nextError = FILE_SELECTOR_ERRORS.directoryMissing
          } else if (!result.exists || !result.accessible) {
            nextError = FILE_SELECTOR_ERRORS.fileMissing
          }
        }

        // 扩展名校验：只在没有其它错误且扩展名非空时提示
        if (!isDirectory && !nextError && allowedExtensions && allowedExtensions.length > 0) {
          try {
            const extension = extensionOf(path)
            if (extension !== '') {
              const allowed = new Set(
                allowedExtensions
                  .filter((item) => item.trim() !== '')
                  .map((item) => item.trim().replace(/^\./, '').toLowerCase()),
              )
              if (!allowed.has(extension)) {
                nextError = FILE_SELECTOR_ERRORS.unsupportedFormat(allowedExtensions)
              }
            }
          } catch (formatError) {
            nextError = FILE_SELECTOR_ERRORS.badFormat(getErrorMessage(formatError))
          }
        }
      } catch (probeError) {
        if (generation !== probeGeneration.current || !mounted.current) return
        nextError = FILE_SELECTOR_ERRORS.readFailed(getErrorMessage(probeError))
      }

      if (generation !== probeGeneration.current || !mounted.current) return
      setError(nextError)
      setFileSize(size)
      setFileExists(exists)
    },
    [allowedExtensions, cancelDebounce, isDirectory, validateExists],
  )

  /** 提交（回调父组件）并重新探测 */
  const commit = useCallback(
    async (raw: string) => {
      const normalized = normalize(raw)
      try {
        onChange(normalized)
      } catch (commitError) {
        toast.error(FILE_DIALOG_MESSAGES.notSaved(getErrorMessage(commitError)))
      }
      await probe(normalized)
    },
    [onChange, probe],
  )

  const handleInput = (next: string) => {
    if (disabled) return
    setText(next)
    cancelDebounce()
    debounceTimer.current = setTimeout(() => {
      debounceTimer.current = null
      void commit(next)
    }, INPUT_DEBOUNCE_MS)
  }

  const handleCommit = () => {
    if (disabled) return
    void commit(text)
  }

  const resolveInitialDirectory = useCallback(
    (current: string): string | undefined => {
      const normalized = normalize(current)
      if (normalized === '') return undefined
      try {
        if (isDirectory && fileExists) return normalized
        const parent = parentDirectory(normalized)
        return parent === '' ? undefined : parent
      } catch (initialError) {
        toast.warning(FILE_DIALOG_MESSAGES.initialDirectory(getErrorMessage(initialError)))
        return undefined
      }
    },
    [fileExists, isDirectory],
  )

  const handleBrowse = async () => {
    if (disabled || isBrowsing) return

    const backend = await resolveDialogBackend()
    const backendDescription = backend.backend
    if (!backend.isSupported) {
      toast.warning(FILE_DIALOG_MESSAGES.unsupported(backendDescription))
      return
    }

    setIsBrowsing(true)
    try {
      const result = await filesApi.dialog({
        kind: isDirectory ? 'folder' : 'file',
        title: `选择${label}`,
        initialDirectory: resolveInitialDirectory(text),
      })
      const selected = result.path?.trim() ?? ''
      if (selected === '') {
        toast.warning(FILE_DIALOG_MESSAGES.cancelled(isDirectory ? '文件夹' : '文件', backendDescription))
        return
      }
      setText(selected)
      setSyncedValue(selected)
      await commit(selected)
    } catch (browseError) {
      toast.error(FILE_DIALOG_MESSAGES.failed(getErrorMessage(browseError), backendDescription))
    } finally {
      if (mounted.current) setIsBrowsing(false)
    }
  }

  const showSizeLine = !isDirectory && showFileInfo && fileExists && text.trim() !== ''
  const describedBy = [error ? errorId : null, showSizeLine ? infoId : null].filter(Boolean).join(' ')

  return (
    <div className="mb-4">
      <label className="ffui-field-label mb-2 block" htmlFor={inputId}>
        {label}
      </label>

      <div className="flex gap-2 items-stretch">
        <input
          id={inputId}
          type="text"
          className={cx('mono flex-1 min-w-0', error && 'border-danger')}
          style={{ fontSize: '0.9rem' }}
          value={text}
          placeholder={placeholder}
          disabled={disabled}
          spellCheck={false}
          autoComplete="off"
          aria-invalid={error ? true : undefined}
          aria-describedby={describedBy === '' ? undefined : describedBy}
          onChange={(event) => handleInput(event.target.value)}
          onBlur={handleCommit}
          onKeyDown={(event) => {
            if (event.key === 'Enter') handleCommit()
          }}
        />

        {showBrowse ? (
          <button
            type="button"
            className="ffui-btn ffui-btn-primary shrink-0 whitespace-nowrap"
            disabled={disabled || isBrowsing}
            aria-label={`浏览并选择${label}（${isDirectory ? '文件夹' : '文件'}）`}
            aria-busy={isBrowsing || undefined}
            onClick={() => void handleBrowse()}
          >
            {isBrowsing ? (
              <span className="spinner" aria-hidden="true" />
            ) : (
              <span aria-hidden="true">📂</span>
            )}
            <span>浏览</span>
          </button>
        ) : null}
      </div>

      {error ? (
        <span className="block mt-1 text-danger" style={{ fontSize: '0.85rem' }} id={errorId}>
          <span aria-hidden="true">⚠️</span> {error}
        </span>
      ) : null}

      {showSizeLine ? (
        <span className="block mt-1 text-muted-text" style={{ fontSize: '0.85rem' }} id={infoId}>
          文件大小: {formatFileSize(fileSize)}
        </span>
      ) : null}
    </div>
  )
}
