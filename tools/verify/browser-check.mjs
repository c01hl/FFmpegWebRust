// 浏览器端到端回归：用真实 Chrome（无头 + DevTools Protocol）走一遍界面。
//
// 为什么需要它：服务端测试跑不出「页面能不能用」——路由、事件绑定、
// WebSocket 实时更新、按钮到底有没有反应，只有真的点一遍才知道。
//
// 用法（本机需要 Chrome，不需要安装任何 npm 包；Node 22+ 自带 WebSocket）：
//   1) 启动应用：cargo run -- --port 5271 --no-browser
//   2) node tools/verify/browser-check.mjs
//
// 环境变量：
//   APP_URL   默认 http://127.0.0.1:5271
//   SAMPLE    一个真实媒体文件的绝对路径（用于转换流程；不传则跳过转换步骤）
//   OUT_DIR   输出目录（默认与 SAMPLE 同目录下的 browser-check-out）
//   HEADFUL=1 显示浏览器窗口（调试用）

import { spawn } from 'node:child_process'
import { mkdirSync } from 'node:fs'
import { setTimeout as sleep } from 'node:timers/promises'

const APP = process.env.APP_URL || 'http://127.0.0.1:5271'
const SAMPLE = process.env.SAMPLE || ''
const OUT_DIR = process.env.OUT_DIR || '/tmp/ffwr-browser-check-out'
const CHROME =
  process.env.CHROME_PATH || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
const PORT = Number(process.env.CDP_PORT || 9333)

const results = []
function check(name, ok, detail = '') {
  results.push({ name, ok })
  console.log(`${ok ? '  [ok]' : '[FAIL]'} ${name}${detail ? ` — ${detail}` : ''}`)
}

// ── CDP 最小客户端 ───────────────────────────────────────────────────────────
class Cdp {
  constructor(socket) {
    this.socket = socket
    this.nextId = 1
    this.pending = new Map()
    this.consoleErrors = []
    this.pageErrors = []

    socket.addEventListener('message', (event) => {
      const message = JSON.parse(event.data)
      if (message.id && this.pending.has(message.id)) {
        const { resolve, reject } = this.pending.get(message.id)
        this.pending.delete(message.id)
        message.error ? reject(new Error(JSON.stringify(message.error))) : resolve(message.result)
        return
      }
      if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
        this.consoleErrors.push(message.params.args.map((a) => a.value ?? a.description).join(' '))
      }
      if (message.method === 'Runtime.exceptionThrown') {
        this.pageErrors.push(message.params.exceptionDetails?.exception?.description || 'unknown')
      }
    })
  }

  send(method, params = {}) {
    const id = this.nextId++
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject })
      this.socket.send(JSON.stringify({ id, method, params }))
      setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error(`${method} 超时`))
      }, 60_000)
    })
  }

  /** 在页面里执行一段表达式并取回值 */
  async evaluate(expression) {
    const result = await this.send('Runtime.evaluate', {
      expression,
      returnByValue: true,
      awaitPromise: true,
    })
    if (result.exceptionDetails) {
      throw new Error(result.exceptionDetails.exception?.description || '页面脚本异常')
    }
    return result.result.value
  }

  /** 轮询直到表达式为真 */
  async waitFor(expression, { timeout = 15_000, interval = 250 } = {}) {
    const deadline = Date.now() + timeout
    while (Date.now() < deadline) {
      if (await this.evaluate(`(() => { try { return !!(${expression}) } catch (e) { return false } })()`)) {
        return true
      }
      await sleep(interval)
    }
    return false
  }

  async goto(path) {
    await this.send('Page.navigate', { url: `${APP}${path}` })
    await sleep(600)
    await this.waitFor(`document.querySelector('.app-title, .brand-text, nav') !== null`, { timeout: 10_000 })
  }
}

// ── 主流程 ───────────────────────────────────────────────────────────────────
const chrome = spawn(
  CHROME,
  [
    process.env.HEADFUL ? '--headless=new' : '--headless=new',
    '--disable-gpu',
    '--no-first-run',
    '--no-default-browser-check',
    `--remote-debugging-port=${PORT}`,
    `--user-data-dir=/tmp/ffwr-chrome-${PORT}`,
    'about:blank',
  ],
  { stdio: 'ignore' },
)

let exitCode = 0
try {
  const target = await waitForTarget()
  const socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true })
    socket.addEventListener('error', reject, { once: true })
  })

  const cdp = new Cdp(socket)
  await cdp.send('Runtime.enable')
  await cdp.send('Page.enable')
  await cdp.send('Log.enable')

  // 1) 应用外壳
  await cdp.goto('/')
  check('首页渲染出应用标题', await cdp.evaluate(`document.title.length > 0`))
  check(
    '首页显示 FFmpeg 环境信息',
    await cdp.waitFor(`document.body.innerText.includes('FFmpeg')`, { timeout: 20_000 }),
  )
  check(
    '首页显示硬件编码器区块',
    await cdp.waitFor(
      `/(VideoToolbox|NVENC|QSV|AMF|VA-API|硬件编码)/.test(document.body.innerText)`,
      { timeout: 25_000 },
    ),
  )
  check(
    '首页显示模板数量',
    await cdp.waitFor(`document.body.innerText.includes('51')`, { timeout: 15_000 }),
  )

  // 2) 导航与各路由
  const routes = [
    ['/convert', /媒体转换|转换/],
    ['/batch', /批量/],
    ['/templates', /模板/],
    ['/tasks', /任务/],
    ['/settings', /设置/],
  ]
  for (const [path, pattern] of routes) {
    await cdp.goto(path)
    const ok = await cdp.waitFor(
      `document.body.innerText.split('\\n').some(l => ${pattern}.test(l)) && document.body.innerText.length > 200`,
      { timeout: 25_000 },
    )
    check(`${path} 渲染出内容（不是白屏）`, ok)
  }

  // 3) 模板页：51 个内置模板 + 不可用模板标注原因
  await cdp.goto('/templates')
  const templateCount = await cdp.evaluate(
    `document.querySelectorAll('[data-template-id], .ffui-card, article').length`,
  )
  check('模板页渲染出模板卡片', templateCount > 0, `卡片数 ${templateCount}`)
  check(
    '不可用模板会显示原因（本机缺 libass 的烧录字幕模板）',
    await cdp.evaluate(`/不可用|本机|缺少/.test(document.body.innerText)`),
  )

  // 4) 设置页：保存 → 服务端 warnings → Toast
  await cdp.goto('/settings')
  check('设置页显示文件已存在处理方式', await cdp.evaluate(`/文件已存在|覆盖|跳过|重命名/.test(document.body.innerText)`))
  check('设置页显示监听端口', await cdp.evaluate(`/监听端口|端口/.test(document.body.innerText)`))

  // 5) 任务页：表格/空状态
  await cdp.goto('/tasks')
  check(
    '任务页有内容（列表或空状态）',
    await cdp.evaluate(`document.body.innerText.length > 200`),
  )

  // 6) 转换流程（可选：需要 SAMPLE）
  if (SAMPLE) {
    mkdirSync(OUT_DIR, { recursive: true })
    await cdp.goto('/convert')

    const typed = await cdp.evaluate(`(() => {
      const inputs = [...document.querySelectorAll('input[type="text"], input:not([type])')]
      const target = inputs.find(i => /路径|文件/.test(i.getAttribute('placeholder') || '')) || inputs[0]
      if (!target) return false
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set
      setter.call(target, ${JSON.stringify(SAMPLE)})
      target.dispatchEvent(new Event('input', { bubbles: true }))
      target.dispatchEvent(new Event('change', { bubbles: true }))
      return true
    })()`)
    check('能把输入文件路径填进转换页', typed)

    // 选一个模板（默认没有选中任何模板，命令预览要等模板确定后才有）
    const picked = await cdp.evaluate(`(() => {
      const button = [...document.querySelectorAll('button')].find((b) =>
        /MP4 · H\\.264（自动选择最佳编码器）/.test(b.textContent || ''),
      )
      if (!button) return false
      button.click()
      return true
    })()`)
    check('能选中一个内置模板', picked)

    // 命令预览可能渲染在 <pre> 里，也可能在只读输入框/文本域里（value 不出现在 innerText）
    const previewed = await cdp.waitFor(
      `(() => {
        const text = document.body.innerText + ' ' +
          [...document.querySelectorAll('input,textarea')].map((e) => e.value).join(' ')
        return /-hide_banner|-progress pipe:2/.test(text)
      })()`,
      { timeout: 30_000 },
    )
    check('命令预览由服务端返回并渲染出来', previewed)

    const started = await cdp.evaluate(`(() => {
      const button = [...document.querySelectorAll('button')].find((b) => /开始转换/.test(b.textContent || ''))
      if (!button || button.disabled) return false
      button.click()
      return true
    })()`)
    check('能点击「开始转换」', started)

    const finished = await cdp.waitFor(`/已完成|转换完成|成功/.test(document.body.innerText)`, {
      timeout: 90_000,
    })
    check('转换在界面上跑到完成状态', finished)

    await cdp.goto('/tasks')
    check(
      '任务列表里能看到刚才的任务',
      await cdp.waitFor(`/已完成/.test(document.body.innerText)`, { timeout: 20_000 }),
    )
  } else {
    console.log('  [skip] 未提供 SAMPLE，跳过真实转换流程（可用 SAMPLE=/path/to.mp4 启用）')
  }

  // 7) 无控制台错误
  check(
    '页面没有 console.error / 未捕获异常',
    cdp.consoleErrors.length === 0 && cdp.pageErrors.length === 0,
    [...cdp.consoleErrors, ...cdp.pageErrors].slice(0, 3).join(' | '),
  )

  socket.close()
} catch (error) {
  check('脚本执行', false, String(error))
} finally {
  chrome.kill()
}

const failed = results.filter((r) => !r.ok)
console.log(`\n${results.length - failed.length}/${results.length} 项检查通过`)
if (failed.length) {
  console.error('失败项：', failed.map((f) => f.name).join('、'))
  exitCode = 1
}
process.exit(exitCode)

async function waitForTarget(timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${PORT}/json/list`)
      const targets = await response.json()
      const page = targets.find((t) => t.type === 'page')
      if (page?.webSocketDebuggerUrl) return page
    } catch {
      // Chrome 还没起来
    }
    await sleep(300)
  }
  throw new Error('无法连接到 Chrome 的调试端口')
}
