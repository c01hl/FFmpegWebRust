// 生成 README 用的界面截图（真实浏览器 + DevTools Protocol）。
//
// 为什么要脚本化：界面会不断改，手动截图很快就会过期；而且空页面（没有任务、没有文件）
// 截图出来信息量很低，所以这里会先把界面驱动到有内容的状态再截图。
//
// 用法（本机需要 Chrome，不需要安装任何 npm 包）：
//   1) 启动应用：cargo run -- --port 5271 --no-browser
//   2) SAMPLE=/path/to/video.mp4 node tools/screenshots/capture.mjs
//
// 产物写入 readmefiles/。

import { spawn } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import { setTimeout as sleep } from 'node:timers/promises'

const APP = process.env.APP_URL || 'http://127.0.0.1:5271'
const SAMPLE = process.env.SAMPLE || ''
const OUT = process.env.OUT_DIR || 'readmefiles'
const PORT = Number(process.env.CDP_PORT || 9444)
const CHROME = process.env.CHROME_PATH || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'

mkdirSync(OUT, { recursive: true })

const chrome = spawn(
  CHROME,
  [
    '--headless=new',
    '--disable-gpu',
    '--no-first-run',
    '--hide-scrollbars',
    `--remote-debugging-port=${PORT}`,
    `--user-data-dir=/tmp/ffwr-shots-${PORT}`,
    '--window-size=1440,1000',
    'about:blank',
  ],
  { stdio: 'ignore' },
)

async function target() {
  for (let i = 0; i < 60; i += 1) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()
      const page = list.find((t) => t.type === 'page')
      if (page) return page
    } catch {
      /* Chrome 还没起来 */
    }
    await sleep(300)
  }
  throw new Error('无法连接到 Chrome 调试端口')
}

const page = await target()
const ws = new WebSocket(page.webSocketDebuggerUrl)
await new Promise((resolve) => ws.addEventListener('open', resolve, { once: true }))

let nextId = 1
const pending = new Map()
ws.addEventListener('message', (event) => {
  const message = JSON.parse(event.data)
  if (message.id && pending.has(message.id)) {
    pending.get(message.id)(message)
    pending.delete(message.id)
  }
})

const send = (method, params = {}) =>
  new Promise((resolve) => {
    const id = nextId++
    pending.set(id, resolve)
    ws.send(JSON.stringify({ id, method, params }))
  })

const evaluate = async (expression) => {
  const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })
  return result.result?.result?.value
}

await send('Runtime.enable')
await send('Page.enable')
await send('Emulation.setDeviceMetricsOverride', {
  width: 1440,
  height: 1000,
  deviceScaleFactor: 2,
  mobile: false,
})

async function shot(path, file, { settle = 9000 } = {}) {
  await send('Page.navigate', { url: `${APP}${path}` })
  await sleep(settle)
  // 不用 captureBeyondViewport：转换页有 51 张模板卡片，整页截图非常大且很慢
  const result = await send('Page.captureScreenshot', { format: 'png' })
  writeFileSync(`${OUT}/${file}`, Buffer.from(result.result.data, 'base64'))
  console.log(`  ${file} ← ${path}`)
}

/** 把输入文件路径与模板填好，让转换页呈现「有内容」的状态 */
async function prepareConvert() {
  await send('Page.navigate', { url: `${APP}/convert` })
  await sleep(3500)
  if (!SAMPLE) return
  await evaluate(`(() => {
    const input = [...document.querySelectorAll('input')].find((i) => /输入视频或音频/.test(i.placeholder || ''))
    if (!input) return
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set
    setter.call(input, ${JSON.stringify(SAMPLE)})
    input.dispatchEvent(new Event('input', { bubbles: true }))
  })()`)
  await sleep(1500)
  await evaluate(`(() => {
    const card = [...document.querySelectorAll('button')].find((b) =>
      /MP4 · H\\.264（自动选择最佳编码器）/.test(b.textContent || ''))
    card && card.click()
  })()`)
  await sleep(3000)
}

await shot('/', 'index.png')
await prepareConvert()
const shotConvert = await send('Page.captureScreenshot', { format: 'png' })
writeFileSync(`${OUT}/cv.png`, Buffer.from(shotConvert.result.data, 'base64'))
console.log('  cv.png ← /convert（已填好输入与模板）')

// 命令预览区域：滚动到底部再截一张，突出「将要执行的命令」
await evaluate(`window.scrollTo(0, document.body.scrollHeight)`)
await sleep(1200)
const shotPreview = await send('Page.captureScreenshot', { format: 'png' })
writeFileSync(`${OUT}/cv-preview.png`, Buffer.from(shotPreview.result.data, 'base64'))
console.log('  cv-preview.png ← /convert 底部')

await shot('/templates', 'tpl.png')
await shot('/settings', 'setting.png')

// 深色主题：写入 localStorage 后重新加载
await evaluate(`localStorage.setItem('ffmpeg-theme', 'dark')`)
await shot('/', 'dark-index.png', { settle: 2500 })
await shot('/convert', 'dark-convert.png', { settle: 2500 })
await evaluate(`localStorage.removeItem('ffmpeg-theme')`)

ws.close()
chrome.kill()
console.log('截图完成')
