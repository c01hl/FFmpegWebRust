/**
 * 批量待处理文件列表（对应 ui-spec §3.10 `BatchFileList.razor` + §6.3 / §7.1）。
 *
 * 添加方式（浏览器拿不到绝对路径，因此**没有拖放**）：
 * 1. 多行粘贴：按 `\n` / `\r` 拆分并 Trim，逐行判定是文件、目录还是不存在；
 * 2. `选择文件` / `选择文件夹`：`POST /api/files/dialog` 拉起系统原生对话框；
 * 3. 目录 → `POST /api/media/scan`（可递归，默认勾选），单次上限 `scanLimit`（默认 5000）。
 *
 * 文件大小来自扫描结果或 `POST /api/files/check`，并**按路径缓存**：
 * 参考实现里每行同步 IO 会在网络盘上卡死界面，这里不允许重复探测。
 */

import { useEffect, useRef, useState } from 'react'
import { getErrorMessage } from '../../api/client'
import { filesApi, mediaApi } from '../../api/endpoints'
import type { TaskStatus } from '../../api/types'
import { taskStatusText } from '../../api/types'
import { formatFileSize } from '../../lib/format'
import { toast } from '../../store/toast'
import { Badge } from '../ui/Badge'
import { Button } from '../ui/Button'
import { ConfirmDialog } from '../ui/ConfirmDialog'
import { IconButton } from '../ui/IconButton'
import { resolveDialogBackend } from '../files/dialogBackend'

/** 单次扫描的文件数量上限（与参考实现一致） */
export const DEFAULT_SCAN_LIMIT = 5000

export interface BatchFileListProps {
  files: string[]
  onChange: (files: string[]) => void
  /** 键为完整路径，值为任务状态（由批次任务列表填充） */
  fileStatuses?: Record<string, TaskStatus>
  isProcessing?: boolean
  scanLimit?: number
}

const VIDEO_EXTENSIONS = new Set(['mp4', 'm4v', 'mov', 'mkv', 'avi', 'webm', 'wmv', 'flv'])
const AUDIO_EXTENSIONS = new Set(['mp3', 'wav', 'flac', 'aac', 'ogg', 'm4a', 'opus', 'wma'])
const IMAGE_EXTENSIONS = new Set(['gif', 'png', 'jpg', 'jpeg', 'bmp', 'webp'])

const STATUS_ICON: Record<TaskStatus, string> = {
  Running: '⏳',
  Completed: '✅',
  Failed: '❌',
  Cancelled: '⏹️',
  Skipped: '⏭️',
  Pending: '⏸️',
}

/** 左侧 3px 状态竖条颜色（ui-spec §3.10） */
const STATUS_STRIPE: Record<TaskStatus, string> = {
  Running: 'var(--info-color)',
  Completed: 'var(--success-color)',
  Failed: 'var(--danger-color)',
  Cancelled: 'var(--warning-color)',
  Skipped: 'var(--warning-color)',
  Pending: 'var(--border-strong)',
}

const STATUS_TONE: Record<TaskStatus, 'default' | 'success' | 'danger' | 'warning' | 'info'> = {
  Completed: 'success',
  Failed: 'danger',
  Running: 'info',
  Cancelled: 'warning',
  Skipped: 'warning',
  Pending: 'default',
}

/** 文件名（去掉目录部分，兼容 Windows 分隔符） */
function fileNameOf(path: string): string {
  const normalized = path.replace(/\\/g, '/')
  return normalized.split('/').pop() || path
}

/** 目录部分 */
function directoryOf(path: string): string {
  const normalized = path.replace(/\\/g, '/')
  const index = normalized.lastIndexOf('/')
  return index <= 0 ? '' : normalized.slice(0, index)
}

/** 无状态时按扩展名给图标 */
function extensionIcon(path: string): string {
  const name = fileNameOf(path)
  const index = name.lastIndexOf('.')
  const extension = index > 0 ? name.slice(index + 1).toLowerCase() : ''
  if (VIDEO_EXTENSIONS.has(extension)) return '🎬'
  if (AUDIO_EXTENSIONS.has(extension)) return '🎵'
  if (IMAGE_EXTENSIONS.has(extension)) return '🖼️'
  return '📄'
}

export function BatchFileList({
  files,
  onChange,
  fileStatuses = {},
  isProcessing = false,
  scanLimit = DEFAULT_SCAN_LIMIT,
}: BatchFileListProps) {
  const [newPath, setNewPath] = useState('')
  const [filter, setFilter] = useState('')
  const [recursive, setRecursive] = useState(true)
  const [isScanning, setIsScanning] = useState(false)
  const [showConfirmClear, setShowConfirmClear] = useState(false)
  const [backendDescription, setBackendDescription] = useState('系统文件对话框')
  /** 路径 → 字节数（来自扫描结果或 /api/files/check，只探测一次） */
  const [sizes, setSizes] = useState<Record<string, number>>({})
  const mounted = useRef(true)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  // 对话框后端描述只查一次（失败也要能显示一个可读的兜底文案）
  useEffect(() => {
    void (async () => {
      const backend = await resolveDialogBackend()
      if (mounted.current) setBackendDescription(backend.backend)
    })()
  }, [])

  const sizeOf = (path: string): number => sizes[path] ?? 0

  /** 提交新列表：永远构造新数组再回传（父组件可能持有原引用），并清掉已移除文件的大小缓存 */
  const commit = (next: string[]) => {
    setSizes((current) => {
      const kept: Record<string, number> = {}
      for (const path of next) {
        if (current[path] !== undefined) kept[path] = current[path]
      }
      return kept
    })
    onChange(next)
  }

  const addPaths = (paths: { path: string; size: number }[]) => {
    const existing = new Set(files)
    const added: string[] = []
    const newSizes: Record<string, number> = {}
    let duplicates = 0

    for (const item of paths) {
      if (existing.has(item.path)) {
        duplicates++
        continue
      }
      existing.add(item.path)
      added.push(item.path)
      if (item.size > 0) newSizes[item.path] = item.size
    }

    if (added.length === 0) {
      toast.show(duplicates > 0 ? '这些文件已经在列表中了。' : '没有可添加的文件。')
      return
    }

    commit([...files, ...added])
    if (Object.keys(newSizes).length > 0) setSizes((current) => ({ ...current, ...newSizes }))
    const message = `已添加 ${added.length} 个文件`
    toast.success(duplicates > 0 ? `${message}，跳过 ${duplicates} 个重复项` : message)
  }

  /** 扫描目录：交给后端（`POST /api/media/scan`），可递归 */
  const scanDirectories = async (directories: string[]) => {
    setIsScanning(true)
    try {
      const limit = Math.max(1, scanLimit)
      const collected: { path: string; size: number; name: string }[] = []

      for (const directory of directories) {
        const result = await mediaApi.scan({ directory, extensions: [], recursive })
        collected.push(...(result.files ?? []))
      }

      if (collected.length === 0) {
        toast.warning('所选目录中没有找到可识别的媒体文件。')
        return
      }

      const truncated = collected.length > limit
      const chosen = truncated ? collected.slice(0, limit) : collected
      addPaths(chosen.map((item) => ({ path: item.path, size: item.size })))

      if (truncated) {
        toast.warning(`该目录文件过多，只添加了前 ${limit} 个。建议缩小范围后分批处理。`)
      }
    } catch (error) {
      toast.error(`扫描目录失败：${getErrorMessage(error)}`)
    } finally {
      if (mounted.current) setIsScanning(false)
    }
  }

  /** 粘贴的多行路径：逐行判定「文件 / 目录 / 不存在」 */
  const addTypedPath = async () => {
    if (newPath.trim() === '') return
    try {
      const candidates = newPath
        .split(/[\n\r]+/)
        .map((line) => line.trim())
        .filter((line) => line !== '')

      setNewPath('')

      const filesToAdd: { path: string; size: number }[] = []
      const directories: string[] = []
      const missing: string[] = []

      for (const candidate of candidates) {
        let handled = false
        try {
          const asFolder = await filesApi.check({ path: candidate, kind: 'folder' })
          if (asFolder.exists) {
            directories.push(candidate)
            handled = true
          }
        } catch {
          // 目录探测失败时继续按文件探测，下面的错误提示已经足够
        }

        if (handled) continue

        try {
          const asFile = await filesApi.check({ path: candidate, kind: 'file' })
          if (asFile.exists && asFile.accessible) {
            filesToAdd.push({ path: candidate, size: asFile.size ?? 0 })
            continue
          }
        } catch {
          // 落到 missing 分支
        }

        missing.push(candidate)
      }

      if (missing.length > 0) {
        const shown = missing.slice(0, 3).join('、')
        toast.warning(
          `以下路径不存在，已忽略：${shown}` + (missing.length > 3 ? ` 等共 ${missing.length} 个` : ''),
        )
      }

      if (directories.length > 0) await scanDirectories(directories)
      if (filesToAdd.length > 0) addPaths(filesToAdd)
    } catch (error) {
      toast.error(`添加失败：${getErrorMessage(error)}`)
    }
  }

  const browseFiles = async () => {
    const backend = await resolveDialogBackend()
    try {
      const result = await filesApi.dialog({
        kind: 'file',
        title: '选择媒体文件',
        initialDirectory: files.length > 0 ? directoryOf(files[0]) : undefined,
      })
      const picked = result.path?.trim() ?? ''
      if (picked === '') {
        toast.show(
          `没有选择文件。如果对话框没有弹出，可以直接把路径粘贴到上方输入框。（当前后端：${backend.backend}）`,
        )
        return
      }
      try {
        const info = await filesApi.check({ path: picked, kind: 'file' })
        addPaths([{ path: picked, size: info.size ?? 0 }])
      } catch {
        addPaths([{ path: picked, size: 0 }])
      }
    } catch (error) {
      toast.error(`打开文件选择器失败：${getErrorMessage(error)}`)
    }
  }

  const browseFolder = async () => {
    const backend = await resolveDialogBackend()
    try {
      const result = await filesApi.dialog({ kind: 'folder', title: '选择包含媒体文件的文件夹' })
      const picked = result.path?.trim() ?? ''
      if (picked === '') {
        toast.show(
          `没有选择文件夹。如果对话框没有弹出，可以直接把文件夹路径粘贴到上方输入框。（当前后端：${backend.backend}）`,
        )
        return
      }
      await scanDirectories([picked])
    } catch (error) {
      toast.error(`打开文件夹选择器失败：${getErrorMessage(error)}`)
    }
  }

  const removeFile = (path: string) => {
    commit(files.filter((item) => item !== path))
  }

  const handleClear = () => {
    setShowConfirmClear(false)
    setSizes({})
    onChange([])
    toast.success('已清空文件列表（磁盘上的文件未受影响）')
  }

  const filteredFiles =
    filter.trim() === ''
      ? files
      : files.filter((path) => fileNameOf(path).toLowerCase().includes(filter.trim().toLowerCase()))

  const totalSize = files.reduce((sum, path) => sum + sizeOf(path), 0)

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="m-0 flex flex-wrap items-center gap-1.5" style={{ fontSize: '1rem' }}>
          📄 待处理文件
          <Badge>{files.length}</Badge>
          {totalSize > 0 ? <Badge>共 {formatFileSize(totalSize)}</Badge> : null}
        </h3>
        {files.length > 0 ? (
          <Button size="sm" disabled={isProcessing} onClick={() => setShowConfirmClear(true)}>
            清空
          </Button>
        ) : null}
      </div>

      <div className="flex flex-wrap gap-1.5">
        <label className="visually-hidden" htmlFor="batch-path-input">
          要添加的文件或文件夹路径
        </label>
        <input
          id="batch-path-input"
          type="text"
          className="flex-1"
          style={{ minWidth: '220px' }}
          value={newPath}
          disabled={isProcessing}
          placeholder="粘贴文件或文件夹的完整路径，按 Enter 添加（可一次粘贴多行）"
          onChange={(event) => setNewPath(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== 'Enter' && event.key !== 'NumpadEnter') return
            if (newPath.trim() === '') return
            event.preventDefault()
            void addTypedPath()
          }}
        />
        <Button
          size="sm"
          variant="primary"
          disabled={isProcessing || newPath.trim() === ''}
          onClick={() => void addTypedPath()}
        >
          添加
        </Button>
        <Button size="sm" disabled={isProcessing || isScanning} onClick={() => void browseFiles()}>
          选择文件
        </Button>
        <Button size="sm" disabled={isProcessing || isScanning} onClick={() => void browseFolder()}>
          选择文件夹
        </Button>
      </div>

      <div className="flex flex-wrap items-center gap-4 text-muted-text" style={{ fontSize: '0.82rem' }}>
        <label className="inline-flex items-center gap-1.5">
          <input
            type="checkbox"
            checked={recursive}
            disabled={isProcessing || isScanning}
            onChange={(event) => setRecursive(event.target.checked)}
          />
          递归扫描子文件夹
        </label>
        <span>浏览按钮使用 {backendDescription}</span>
      </div>

      {isScanning ? (
        <div className="ffui-loading-row" role="status">
          <span className="spinner" aria-hidden="true" />
          <span>正在扫描目录…</span>
        </div>
      ) : null}

      {files.length > 6 ? (
        <div className="flex flex-wrap items-center gap-2.5">
          <label className="visually-hidden" htmlFor="batch-filter">
            按文件名筛选
          </label>
          <input
            id="batch-filter"
            type="search"
            style={{ maxWidth: '320px' }}
            placeholder="按文件名筛选…"
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
          />
          {filter.trim() !== '' ? (
            <span className="text-muted-text" style={{ fontSize: '0.82rem' }}>
              匹配 {filteredFiles.length} / {files.length}
            </span>
          ) : null}
        </div>
      ) : null}

      {files.length === 0 ? (
        <div className="ffui-empty-state">
          还没有添加文件。
          <br />
          用上面的输入框粘贴路径，或点击「选择文件 / 选择文件夹」批量添加。
        </div>
      ) : filteredFiles.length === 0 ? (
        <div className="ffui-empty-state">
          <p>没有匹配「{filter}」的文件。</p>
          <Button size="sm" onClick={() => setFilter('')}>
            清除筛选
          </Button>
        </div>
      ) : (
        <ul
          className="list-none m-0 p-0 flex flex-col gap-1.5"
          style={{ maxHeight: '380px', overflowY: 'auto' }}
        >
          {filteredFiles.map((path) => {
            const status = fileStatuses[path]
            return (
              <li
                key={path}
                className="flex items-center gap-2.5 rounded-sm border border-line bg-surface-3 px-2 py-1.5"
                style={{ borderLeftWidth: '3px', borderLeftColor: status ? STATUS_STRIPE[status] : 'var(--border-strong)' }}
              >
                <span aria-hidden="true">{status ? STATUS_ICON[status] : extensionIcon(path)}</span>

                <div className="flex-1 min-w-0">
                  <div className="truncate" style={{ fontSize: '0.88rem' }} title={path}>
                    {fileNameOf(path)}
                  </div>
                  <div className="truncate text-muted-text" style={{ fontSize: '0.75rem' }} title={path}>
                    {directoryOf(path)}
                  </div>
                </div>

                <span
                  className="mono shrink-0 text-muted-text"
                  style={{ fontSize: '0.78rem' }}
                  title={`${sizeOf(path)} 字节`}
                >
                  {formatFileSize(sizeOf(path))}
                </span>

                {status ? (
                  <Badge tone={STATUS_TONE[status]}>{taskStatusText(status, 'batch')}</Badge>
                ) : null}

                <IconButton
                  ariaLabel={`移除 ${fileNameOf(path)}`}
                  disabled={isProcessing}
                  onClick={() => removeFile(path)}
                >
                  ✕
                </IconButton>
              </li>
            )
          })}
        </ul>
      )}

      <ConfirmDialog
        open={showConfirmClear}
        onOpenChange={setShowConfirmClear}
        title="清空文件列表"
        danger
        message="确定要清空待处理文件列表吗？"
        detail={`将移除列表中的 ${files.length} 个文件。不会删除磁盘上的任何文件。`}
        confirmText="清空列表"
        onConfirm={handleClear}
      />
    </div>
  )
}
