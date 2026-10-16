/**
 * 模板详情弹窗（对应 ui-spec §3.8 `TemplateDetailViewer.razor`）。
 *
 * 区块顺序：顶部提示 → 基本信息 → 命令与预览 → 参数 → 格式 → 硬件加速 → 其它 → 底部按钮栏。
 *
 * ⚠️ 命令预览**只来自后端**：`POST /api/tasks/preview`（输入/输出用占位路径
 * `<输入文件>` / `<输出文件>`），前端不自己展开占位符。
 * 因此预览里显示的输出路径是服务端解析后的结果，而不是字面的 `<输出文件>`。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { getErrorMessage } from '../../api/client'
import { systemApi, tasksApi, templatesApi } from '../../api/endpoints'
import type { CommandTemplate, HardwareReportDto, TemplateParameter } from '../../api/types'
import { COPY_FAILURE_MESSAGES, copyAndToast } from '../../lib/clipboard'
import { formatDateTime, toFixedTrim } from '../../lib/format'
import { toast } from '../../store/toast'
import { Alert } from '../ui/Alert'
import { Badge } from '../ui/Badge'
import { Button } from '../ui/Button'
import { Modal } from '../ui/Modal'
import { Spinner } from '../ui/Spinner'
import { TemplateParameterEditor } from './TemplateParameterEditor'
import { familyDisplayName } from '../convert/hardwareLabels'

/** 预览用的占位路径（尖括号是字面文本） */
export const PREVIEW_PLACEHOLDER_INPUT = '<输入文件>'
export const PREVIEW_PLACEHOLDER_OUTPUT = '<输出文件>'

/** 控件类型中文（只读展示用） */
const KIND_TEXT: Record<TemplateParameter['kind'], string> = {
  Select: '下拉选择',
  Number: '数字',
  Boolean: '开关',
  Text: '文本',
}

export interface TemplateDetailViewerProps {
  template: CommandTemplate | null
  open: boolean
  onOpenChange: (open: boolean) => void
  /** `📑 复制为自定义模板`（系统模板） */
  onCopy?: (template: CommandTemplate) => void
  /** `✏️ 编辑模板`（用户模板） */
  onEdit?: (template: CommandTemplate) => void
  /** `🚀 使用此模板` */
  onUse?: (template: CommandTemplate) => void
  /** 外部参数值（受控）；不传则用组件内部状态 */
  parameterValues?: Record<string, string>
  onParameterValuesChange?: (values: Record<string, string>) => void
}

/** 参数默认值字典（模板参数 → DefaultValue） */
function defaultValues(template: CommandTemplate | null): Record<string, string> {
  const result: Record<string, string> = {}
  for (const parameter of template?.parameters ?? []) {
    if ((parameter.name ?? '').trim() === '') continue
    result[parameter.name] = parameter.defaultValue ?? ''
  }
  return result
}

/** 从「具体编码器名或族名」里取出族名（`h264_nvenc` → `nvenc`） */
function familyOf(required: string | null | undefined): string {
  const value = (required ?? '').trim()
  if (value === '') return ''
  const underscore = value.lastIndexOf('_')
  return underscore > 0 ? value.slice(underscore + 1) : value
}

/** 动作包装：缺失回调 → Warning；抛异常 → Error（§3.8 文案） */
function runAction(action: string, callback: (() => void | Promise<void>) | undefined) {
  if (!callback) {
    toast.warning(`${action}功能不可用：父页面未提供对应回调`)
    return
  }
  try {
    const result = callback()
    if (result instanceof Promise) {
      result.catch((error: unknown) => toast.error(`${action}失败：${getErrorMessage(error)}`))
    }
  } catch (error) {
    toast.error(`${action}失败：${getErrorMessage(error)}`)
  }
}

export function TemplateDetailViewer({
  template,
  open,
  onOpenChange,
  onCopy,
  onEdit,
  onUse,
  parameterValues,
  onParameterValuesChange,
}: TemplateDetailViewerProps) {
  const [innerValues, setInnerValues] = useState<Record<string, string>>(() => defaultValues(template))
  const [report, setReport] = useState<HardwareReportDto | null>(null)
  const [reportError, setReportError] = useState<string | null>(null)
  const [previewCommand, setPreviewCommand] = useState<string>('')
  const [previewWarnings, setPreviewWarnings] = useState<string[]>([])
  const [previewError, setPreviewError] = useState<string | null>(null)
  const [isPreviewing, setIsPreviewing] = useState(false)
  const [isRefreshing, setIsRefreshing] = useState(false)
  const [syncedTemplate, setSyncedTemplate] = useState(template)
  const [current, setCurrent] = useState<CommandTemplate | null>(template)
  const [copied, setCopied] = useState(false)

  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const previewGeneration = useRef(0)
  const mounted = useRef(true)
  /** 上一次预览用的参数指纹：避免父组件每次渲染传新对象时反复请求 */
  const lastPreviewKey = useRef('')

  // 父组件换了模板实例（同 id 的新对象也算）时同步本地副本：
  // 用「渲染期调整状态」的官方写法，避免在 effect 里同步 setState。
  if (template !== syncedTemplate) {
    setSyncedTemplate(template)
    setCurrent(template)
  }

  const values = parameterValues ?? innerValues
  const editable = Boolean(onParameterValuesChange)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      if (copiedTimer.current !== null) clearTimeout(copiedTimer.current)
    }
  }, [])

  // 打开弹窗：重置参数默认值 + 读硬件报告（所有 setState 都在 await 之后）
  useEffect(() => {
    if (!open) return
    let cancelled = false

    void (async () => {
      await Promise.resolve()
      if (cancelled || !mounted.current) return
      if (parameterValues === undefined) setInnerValues(defaultValues(template))
      setReportError(null)
      lastPreviewKey.current = ''

      try {
        const result = await systemApi.encoders()
        if (cancelled || !mounted.current) return
        setReport(result)
      } catch (error) {
        if (cancelled || !mounted.current) return
        setReport(null)
        setReportError(`读取硬件编码器信息失败：${getErrorMessage(error)}`)
      }
    })()

    return () => {
      cancelled = true
    }
  }, [open, parameterValues, template])

  // 预览：模板 / 参数变化时重建（参数改动立即反映）
  const rebuildPreview = useCallback(
    async (target: CommandTemplate | null, nextValues: Record<string, string>) => {
      if (!target) {
        setPreviewCommand('')
        setPreviewWarnings([])
        return
      }
      const generation = ++previewGeneration.current
      setIsPreviewing(true)
      try {
        const result = await tasksApi.preview({
          inputPath: PREVIEW_PLACEHOLDER_INPUT,
          outputDirectory: PREVIEW_PLACEHOLDER_OUTPUT,
          outputFileNameTemplate: null,
          outputExtension: null,
          templateId: target.id,
          parameterValues: nextValues,
          overwriteExisting: false,
          allowRename: true,
        })
        if (generation !== previewGeneration.current || !mounted.current) return
        setPreviewCommand(result.commandPreview ?? '')
        setPreviewWarnings(result.warnings ?? [])
        setPreviewError(null)
      } catch (error) {
        if (generation !== previewGeneration.current || !mounted.current) return
        setPreviewCommand('')
        setPreviewWarnings([])
        setPreviewError(`命令预览生成失败：${getErrorMessage(error)}`)
      } finally {
        if (generation === previewGeneration.current && mounted.current) setIsPreviewing(false)
      }
    },
    [],
  )

  useEffect(() => {
    if (!open) return
    let cancelled = false
    void (async () => {
      await Promise.resolve()
      if (cancelled || !mounted.current) return
      // 参数内容没变就不重复请求（父组件可能每次渲染都传新的对象实例）
      const key = JSON.stringify(values)
      if (key === lastPreviewKey.current) return
      lastPreviewKey.current = key
      await rebuildPreview(current, values)
    })()
    return () => {
      cancelled = true
    }
  }, [open, current, rebuildPreview, values])

  const handleValuesChange = (next: Record<string, string>) => {
    // 参数改动立即刷新预览（不等 effect）
    lastPreviewKey.current = JSON.stringify(next)
    void rebuildPreview(current, next)
    if (parameterValues === undefined) setInnerValues(next)
    try {
      onParameterValuesChange?.(next)
    } catch (error) {
      toast.error(`参数变更通知失败：${getErrorMessage(error)}`)
    }
  }

  const handleRefresh = async () => {
    if (!current || isRefreshing) return
    setIsRefreshing(true)
    try {
      const fresh = await templatesApi.get(current.id)
      if (!mounted.current) return
      setCurrent(fresh)
      toast.success('已重新读取模板')
    } catch (error) {
      const message = getErrorMessage(error)
      if (message.includes('TEMPLATE_NOT_FOUND') || message.includes('不存在')) {
        toast.warning('该模板已不存在，可能已被删除')
      } else {
        toast.error(`重新读取模板失败：${message}`)
      }
    } finally {
      if (mounted.current) setIsRefreshing(false)
    }
  }

  const handleCopyCommand = async () => {
    const ok = await copyAndToast(current?.commandArgs ?? '', {
      successText: '命令参数已复制到剪贴板',
      emptyText: '该模板没有可复制的命令参数',
      failureText: COPY_FAILURE_MESSAGES.commandArgs,
    })
    if (!ok) return
    setCopied(true)
    if (copiedTimer.current !== null) clearTimeout(copiedTimer.current)
    copiedTimer.current = setTimeout(() => setCopied(false), 2000)
  }

  const requiredFamily = useMemo(() => familyOf(current?.requiredEncoder), [current])
  const availableMember = useMemo(() => {
    if (!report || requiredFamily === '') return null
    const key = requiredFamily.toLowerCase()
    return (
      report.encoders.find(
        (encoder) => encoder.isAvailable && (encoder.family ?? '').toLowerCase() === key,
      ) ?? null
    )
  }, [report, requiredFamily])

  const mode = current?.hardwareMode ?? 'None'
  const missingReason =
    requiredFamily === ''
      ? '模板未指定编码器族'
      : (report?.encoders.find((encoder) => (encoder.family ?? '').toLowerCase() === requiredFamily.toLowerCase() && encoder.unavailableReason)
          ?.unavailableReason ??
        `未检测到可用的 ${familyDisplayName(requiredFamily)} 编码器`)

  const title = current ? `📋 模板详情 · ${current.name}` : '📋 模板详情'

  return (
    <Modal title={title} open={open} onOpenChange={onOpenChange} size="lg">
      {!current ? (
        <p>没有可显示的模板。</p>
      ) : (
        <div className="flex flex-col gap-4">
          {reportError ? (
            <Alert tone="warning" role="status">
              {reportError}
            </Alert>
          ) : null}

          {mode === 'Required' && !availableMember ? (
            <Alert tone="warning" role="alert">
              该模板必须使用 {familyDisplayName(requiredFamily)} 硬件编码，本机不可用：{missingReason}
            </Alert>
          ) : null}

          <section>
            <h3 className="m-0 mb-2" style={{ fontSize: '1rem' }}>
              基本信息
            </h3>
            <dl className="m-0 grid gap-1" style={{ gridTemplateColumns: 'auto 1fr' }}>
              <dt className="text-muted-text">模板名称</dt>
              <dd className="m-0 font-semibold" style={{ fontSize: '1.05rem' }}>
                {current.name}
              </dd>
              <dt className="text-muted-text">类型</dt>
              <dd className="m-0">
                <Badge tone={current.type === 'System' ? 'default' : 'success'}>
                  {current.type === 'System' ? '系统预设' : '用户自定义'}
                </Badge>
              </dd>
              <dt className="text-muted-text">分类</dt>
              <dd className="m-0">📂 {current.category || '未分类'}</dd>
              <dt className="text-muted-text">描述</dt>
              <dd className="m-0">{current.description?.trim() ? current.description : '无描述'}</dd>
            </dl>
          </section>

          <section>
            <div className="flex flex-wrap items-center justify-between gap-2 mb-1">
              <span>FFmpeg 命令参数</span>
              <Button size="sm" aria-label="复制命令参数" onClick={() => void handleCopyCommand()}>
                {copied ? '✓ 已复制' : '📋 复制'}
              </Button>
            </div>
            <pre
              className="mono m-0 rounded-sm border border-line bg-code px-3 py-2"
              style={{ fontSize: '0.8rem', whiteSpace: 'pre-wrap', wordBreak: 'break-all' }}
            >
              {current.commandArgs}
            </pre>

            <div className="mt-3">
              <div className="flex flex-wrap items-center gap-2">
                <span>命令预览</span>
                <span className="text-muted-text" style={{ fontSize: '0.8rem' }}>
                  用占位符拼出的实际命令行，参数改动会立即反映在这里
                </span>
              </div>

              {previewError ? (
                <Alert tone="danger" role="alert" className="mt-2">
                  {previewError}
                </Alert>
              ) : isPreviewing && previewCommand === '' ? (
                <div className="ffui-loading-row" role="status">
                  <Spinner />
                  正在生成命令预览…
                </div>
              ) : (
                <pre
                  className="mono m-0 mt-2 rounded-sm px-3 py-2 border border-primary"
                  style={{ fontSize: '0.8rem', whiteSpace: 'pre-wrap', wordBreak: 'break-all', background: 'var(--code-bg)' }}
                >
                  {`ffmpeg ${previewCommand}`}
                </pre>
              )}

              {previewWarnings.length > 0 ? (
                <ul className="mt-2 pl-4 text-secondary-text" style={{ fontSize: '0.82rem' }}>
                  {previewWarnings.map((warning) => (
                    <li key={warning}>⚠ {warning}</li>
                  ))}
                </ul>
              ) : null}
            </div>
          </section>

          <section>
            <h3 className="m-0 mb-2" style={{ fontSize: '1rem' }}>
              参数
            </h3>
            {(current.parameters ?? []).filter((parameter) => (parameter.name ?? '').trim() !== '').length === 0 ? (
              <p className="text-muted-text m-0">该模板没有可调参数</p>
            ) : editable ? (
              <TemplateParameterEditor template={current} values={values} onChange={handleValuesChange} />
            ) : (
              <>
                <ul className="m-0 pl-4">
                  {(current.parameters ?? []).map((parameter) => (
                    <li key={parameter.name} className="mb-1">
                      <strong>{parameter.label || parameter.name}</strong>
                      <span className="text-muted-text">（{KIND_TEXT[parameter.kind]}）</span>
                      {parameter.defaultValue ? (
                        <span className="text-muted-text"> 默认：{parameter.defaultValue}</span>
                      ) : null}
                      {parameter.description ? (
                        <div className="text-muted-text" style={{ fontSize: '0.8rem' }}>
                          {parameter.description}
                        </div>
                      ) : null}
                    </li>
                  ))}
                </ul>
                <p className="text-muted-text mt-2 m-0" style={{ fontSize: '0.8rem' }}>
                  只读展示：父页面未提供 ParameterValuesChanged，暂时无法在此调整参数。
                </p>
              </>
            )}
          </section>

          <section>
            <h3 className="m-0 mb-2" style={{ fontSize: '1rem' }}>
              格式
            </h3>
            <div>
              输出格式 → <code className="mono">.{current.outputExtension}</code>
            </div>
            <div className="mt-1">
              支持的输入格式 <span className="text-muted-text">（参考信息）</span>：
              {current.supportedInputFormats?.length > 0
                ? current.supportedInputFormats.map((item) => `.${item.replace(/^\./, '')}`).join(', ')
                : '不限制'}
            </div>
            <p className="text-muted-text mt-1 m-0" style={{ fontSize: '0.8rem' }}>
              该列表只是参考资料，并不强制校验；能否处理取决于 FFmpeg 与源文件编码。
            </p>
          </section>

          <section>
            <h3 className="m-0 mb-2" style={{ fontSize: '1rem' }}>
              硬件加速
            </h3>
            {!report && !reportError ? (
              <div className="ffui-loading-row" role="status">
                <Spinner />
                正在读取本机硬件编码能力…
              </div>
            ) : (
              <>
                <div>
                  策略 →{' '}
                  {mode === 'Required'
                    ? '必须使用硬件编码器（本机不可用时该模板不可用）'
                    : mode === 'Auto'
                      ? '自动：优先使用硬件编码器，没有可用硬件时回退到软件编码'
                      : '不使用硬件加速（使用模板中指定的编码器）'}
                </div>

                {requiredFamily !== '' || mode === 'Required' ? (
                  <div className="mt-1 flex flex-wrap items-center gap-2">
                    <span>编码器族</span>
                    {requiredFamily === '' ? (
                      <>
                        <Badge tone="danger" icon="✗">
                          不可用
                        </Badge>
                        <span className="text-muted-text">模板未指定编码器族</span>
                      </>
                    ) : (
                      <>
                        <Badge tone={availableMember ? 'success' : 'danger'} icon={availableMember ? '✓' : '✗'}>
                          {availableMember ? '可用' : '不可用'}
                        </Badge>
                        <span>{familyDisplayName(requiredFamily)}</span>
                        <span className="text-muted-text">
                          {availableMember
                            ? `将使用 ${availableMember.displayName || availableMember.name}`
                            : missingReason}
                        </span>
                      </>
                    )}
                  </div>
                ) : null}

                {mode !== 'None' || (current.commandArgs ?? '').includes('{encoder}') ? (
                  <div className="mt-1 flex flex-wrap items-center gap-2">
                    <span>将使用的编码器</span>
                    {availableMember ? (
                      <Badge tone="success">{availableMember.name}</Badge>
                    ) : (
                      <span className="text-muted-text">（模板未使用 {`{encoder}`}，由模板内固定编码器决定）</span>
                    )}
                  </div>
                ) : null}
              </>
            )}
          </section>

          <section>
            <h3 className="m-0 mb-2" style={{ fontSize: '1rem' }}>
              其它
            </h3>
            <div>
              创建时间：{' '}
              {current.type === 'System' ? (
                <em className="text-muted-text">系统预设</em>
              ) : (
                formatDateTime(current.createdAt)
              )}
            </div>
            <div>更新时间：{formatDateTime(current.updatedAt)}</div>
            <div>排序权重：{toFixedTrim(current.sortOrder ?? 0, 0)}</div>
          </section>
        </div>
      )}

      <div className="ffui-modal-footer">
        <Button
          variant="ghost"
          className="mr-auto"
          disabled={isRefreshing || !current}
          onClick={() => void handleRefresh()}
        >
          {isRefreshing ? <Spinner /> : '↻'} 重新读取
        </Button>
        <Button onClick={() => onOpenChange(false)}>关闭</Button>
        <Button aria-label="复制命令参数" onClick={() => void handleCopyCommand()}>
          📋 复制命令
        </Button>
        {current?.type === 'System' ? (
          <Button
            onClick={() => {
              onOpenChange(false)
              runAction('复制为自定义模板', () => {
                if (current) onCopy?.(current)
              })
            }}
          >
            📑 复制为自定义模板
          </Button>
        ) : (
          <Button
            onClick={() => {
              onOpenChange(false)
              runAction('编辑模板', () => {
              if (current) onEdit?.(current)
            })
            }}
          >
            ✏️ 编辑模板
          </Button>
        )}
        <Button
          variant="primary"
          disabled={!current || (mode === 'Required' && !availableMember)}
          title={mode === 'Required' && !availableMember ? missingReason : undefined}
          onClick={() => {
            onOpenChange(false)
            runAction('使用此模板', () => {
              if (current) onUse?.(current)
            })
          }}
        >
          🚀 使用此模板
        </Button>
      </div>
    </Modal>
  )
}
