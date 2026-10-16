/**
 * 导入模板弹窗（对应 ui-spec §4.5 / §4.6 / §8.7 的「导入弹窗」）。
 *
 * 两个页面的差异只有两处文案（复选框标签与勾选后的警告），因此由调用方用 props 传入，
 * 其余结构（说明文案、textarea、按钮、禁用规则）完全共用。
 *
 * 行为：
 * - 说明文案逐字：`把「导出」得到的 JSON 内容粘贴到下面。浏览器出于安全限制不允许页面直接读取剪贴板，请手动粘贴（⌘/Ctrl + V）。`
 * - 支持「粘贴」与「选择文件」两种录入方式（任务要求；浏览器仍然不允许页面主动读剪贴板）；
 * - 内容为空 → Warning `请先粘贴模板 JSON 内容。`；
 * - 导入中弹窗与遮罩都禁止关闭；
 * - 服务端返回的 `errors` 逐条原样渲染（不加工文案）。
 */

import { useRef, useState } from 'react'
import type { TemplateImportResultDto } from '../../api/types'
import { getErrorMessage } from '../../api/client'
import { templatesApi } from '../../api/endpoints'
import { toast } from '../../store/toast'
import { Alert } from '../ui/Alert'
import { Button } from '../ui/Button'
import { Modal } from '../ui/Modal'

export interface TemplateImportDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** 复选框文案：模板管理页 `导入为系统模板（高级选项）`，预设管理页 `导入为系统模板` */
  asSystemLabel: string
  /** 勾选后的警告文案（两页措辞不同，逐字传入） */
  asSystemWarning: string
  /** 导入成功（`imported > 0`）后回调，由页面决定 Toast 文案与刷新 */
  onImported: (result: TemplateImportResultDto, asSystem: boolean) => void
}

export function TemplateImportDialog({
  open,
  onOpenChange,
  asSystemLabel,
  asSystemWarning,
  onImported,
}: TemplateImportDialogProps) {
  const [json, setJson] = useState('')
  const [asSystem, setAsSystem] = useState(false)
  const [importing, setImporting] = useState(false)
  const [serverErrors, setServerErrors] = useState<string[]>([])
  const fileInputRef = useRef<HTMLInputElement | null>(null)

  const close = () => {
    if (importing) return
    setJson('')
    setAsSystem(false)
    setServerErrors([])
    onOpenChange(false)
  }

  const handleOpenChange = (next: boolean) => {
    if (!next) {
      close()
      return
    }
    onOpenChange(true)
  }

  const pickFile = async (file: File | undefined) => {
    if (!file) return
    try {
      const text = await file.text()
      setJson(text)
      setServerErrors([])
    } catch (error) {
      toast.error(getErrorMessage(error))
    }
  }

  const submit = async () => {
    if (importing) return
    const payload = json.trim()
    if (payload.length === 0) {
      toast.warning('请先粘贴模板 JSON 内容。')
      return
    }

    setImporting(true)
    setServerErrors([])
    try {
      const result = await templatesApi.importJson({ json: payload, asSystemTemplate: asSystem })
      const errors = result.errors ?? []
      setServerErrors(errors)

      if (result.imported > 0) {
        onImported(result, asSystem)
        if (errors.length === 0) {
          setJson('')
          setAsSystem(false)
          onOpenChange(false)
        } else {
          // 有逐条错误时保持打开：服务端消息必须让用户看到（原样渲染）
          setJson('')
        }
      } else {
        toast.warning('没有导入任何模板：JSON 里没有含 name 与 commandArgs 的模板条目。')
      }
    } catch (error) {
      toast.error(`导入失败：${getErrorMessage(error)}`)
    } finally {
      setImporting(false)
    }
  }

  return (
    <Modal
      open={open}
      onOpenChange={handleOpenChange}
      title="导入模板"
      size="lg"
      disableClose={importing}
      closeOnBackdrop={!importing}
    >
      <p className="m-0 mb-3 text-[0.88rem] text-secondary-text">
        把「导出」得到的 JSON 内容粘贴到下面。浏览器出于安全限制不允许页面直接读取剪贴板，请手动粘贴（⌘/Ctrl
        + V）。
      </p>

      <div className="mb-2 flex flex-wrap items-center gap-2">
        <Button size="sm" disabled={importing} onClick={() => fileInputRef.current?.click()}>
          📄 选择 JSON 文件
        </Button>
        <span className="text-[0.78rem] text-muted-text">也可以直接粘贴文本。</span>
        <input
          ref={fileInputRef}
          type="file"
          accept=".json,application/json"
          className="visually-hidden"
          aria-label="选择要导入的模板 JSON 文件"
          onChange={(event) => {
            void pickFile(event.target.files?.[0])
            event.target.value = ''
          }}
        />
      </div>

      <textarea
        className="mono w-full text-[0.85rem]"
        rows={10}
        aria-label="模板 JSON 内容"
        placeholder="在此粘贴包含 templates 数组的 JSON"
        value={json}
        disabled={importing}
        onChange={(event) => setJson(event.target.value)}
      />

      <label className="mt-3 inline-flex cursor-pointer items-center gap-[0.4rem] text-[0.9rem]">
        <input
          type="checkbox"
          className="w-auto"
          checked={asSystem}
          disabled={importing}
          onChange={(event) => setAsSystem(event.target.checked)}
        />
        {asSystemLabel}
      </label>

      {asSystem ? (
        <Alert tone="warning" role="alert">
          {asSystemWarning}
        </Alert>
      ) : null}

      {serverErrors.length > 0 ? (
        <Alert tone="warning" role="alert" className="mt-2">
          <ul className="m-0 list-disc pl-[1.1rem]">
            {serverErrors.map((item, index) => (
              <li key={`${index}-${item}`}>{item}</li>
            ))}
          </ul>
        </Alert>
      ) : null}

      <div className="ffui-modal-footer">
        <Button disabled={importing} onClick={close}>
          取消
        </Button>
        <Button variant="primary" loading={importing} disabled={importing} onClick={() => void submit()}>
          导入
        </Button>
      </div>
    </Modal>
  )
}
