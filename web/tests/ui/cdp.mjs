// 极简的 Chrome DevTools 协议（CDP）驱动：只用 Node 内置的 WebSocket / fetch，不依赖 Playwright / Puppeteer。
// 够界面冒烟测试用：打开页面、执行脚本取值、等待条件、按键、收集控制台错误、失败时截图。
import { Buffer } from 'node:buffer'
import { spawn } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'

const sleep = ms => new Promise(r => setTimeout(r, ms))

/** 本机的 Chrome / Chromium：环境变量 CHROME_BIN 优先 */
export function findChrome() {
  const candidates = [
    process.env.CHROME_BIN,
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
  ].filter(Boolean)
  return candidates.find(p => fs.existsSync(p)) ?? null
}

/** 浏览器级连接：发命令、收事件 */
/** 单条命令最长等多久：页面卡死、Chrome 崩溃时命令不会有回应，不设上限的话 waitFor 的超时永远轮不到、整个测试挂住 */
const COMMAND_TIMEOUT = 60_000

class Connection {
  constructor(ws) {
    this.ws = ws
    this.nextId = 1
    this.pending = new Map()
    this.listeners = new Set()
    this.closed = null
    ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(ev.data)
      if (msg.id) {
        const p = this.pending.get(msg.id)
        if (!p)
          return
        this.pending.delete(msg.id)
        clearTimeout(p.timer)
        msg.error ? p.reject(new Error(`${p.method}: ${msg.error.message}`)) : p.resolve(msg.result)
      }
      else {
        for (const l of this.listeners) l(msg)
      }
    })
    // 连接断了（Chrome 退出或崩溃）：还在等的命令全部失败，不再干等
    ws.addEventListener('close', () => {
      this.closed = new Error('与 Chrome 的连接已断开')
      for (const p of this.pending.values()) {
        clearTimeout(p.timer)
        p.reject(new Error(`${p.method}: ${this.closed.message}`))
      }
      this.pending.clear()
    })
  }

  send(method, params = {}, sessionId, { timeout = COMMAND_TIMEOUT } = {}) {
    if (this.closed)
      return Promise.reject(new Error(`${method}: ${this.closed.message}`))
    const id = this.nextId++
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id)
        reject(new Error(`${method}: Chrome 在 ${timeout / 1000} 秒内没有回应`))
      }, timeout)
      this.pending.set(id, { resolve, reject, method, timer })
      this.ws.send(JSON.stringify({ id, method, params, sessionId }))
    })
  }

  on(fn) {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }
}

export async function launchChrome({ headless = true } = {}) {
  const bin = findChrome()
  if (!bin)
    throw new Error('找不到 Chrome / Chromium：请安装，或用环境变量 CHROME_BIN 指定路径')
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ui-test-chrome-'))
  const args = [
    `--user-data-dir=${dir}`,
    '--remote-debugging-port=0',
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-extensions',
    '--disable-background-timer-throttling',
    '--disable-renderer-backgrounding',
    '--window-size=1440,900',
    '--lang=zh-CN',
    ...(headless ? ['--headless=new', '--hide-scrollbars'] : []),
    ...(process.platform === 'linux' ? ['--no-sandbox'] : []),
    'about:blank',
  ]
  // Linux 上的 Chrome 不看 --lang，界面语言（navigator.language）取环境变量 LANGUAGE / LANG：固定成中文，测试不随本机语言变
  const proc = spawn(bin, args, { stdio: 'ignore', env: { ...process.env, LANGUAGE: 'zh_CN', LANG: 'zh_CN.UTF-8' } })
  // 测试进程无论怎么结束（用例超时被取消、after 没来得及跑）都带走 Chrome，不留给下一个测试文件
  const killOnExit = () => proc.kill('SIGKILL')
  process.once('exit', killOnExit)
  proc.once('exit', () => process.off('exit', killOnExit))
  const portFile = path.join(dir, 'DevToolsActivePort')
  let info = null
  for (let i = 0; i < 100 && !info; i++) {
    await sleep(100)
    if (fs.existsSync(portFile)) {
      const [port, wsPath] = fs.readFileSync(portFile, 'utf8').trim().split('\n')
      if (port && wsPath)
        info = { port, wsPath }
    }
  }
  if (!info) {
    proc.kill()
    throw new Error('Chrome 没有在 10 秒内启动')
  }
  const ws = new WebSocket(`ws://127.0.0.1:${info.port}${info.wsPath}`)
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, { once: true })
    ws.addEventListener('error', () => reject(new Error('连接 Chrome 失败')), { once: true })
  })
  const conn = new Connection(ws)
  return {
    conn,
    /** 新开一个标签页（独立的 cookie 之外，与其他页共享存储；各测试用不同令牌的 cookie 即可） */
    async newPage({ isolated = false } = {}) {
      // isolated：放进独立的浏览器上下文（cookie、本地存储都不与其他页共享），用来同时模拟两个不同的登录用户
      const ctx = isolated ? (await conn.send('Target.createBrowserContext')).browserContextId : undefined
      const { targetId } = await conn.send('Target.createTarget', { url: 'about:blank', ...(ctx ? { browserContextId: ctx } : {}) })
      const { sessionId } = await conn.send('Target.attachToTarget', { targetId, flatten: true })
      const page = new Page(conn, targetId, sessionId)
      await page.init()
      return page
    },
    async close() {
      const exited = proc.exitCode !== null || proc.signalCode !== null ? Promise.resolve() : new Promise(r => proc.once('exit', r))
      try {
        await conn.send('Browser.close', {}, undefined, { timeout: 5000 })
      }
      catch {}
      ws.close()
      proc.kill()
      // 等 Chrome 真正退出再删临时目录（否则偶尔 ENOTEMPTY）；没退出就强制结束。清理失败不应影响测试结果
      if (await Promise.race([exited.then(() => true), sleep(5000).then(() => false)]) === false) {
        proc.kill('SIGKILL')
        await Promise.race([exited, sleep(2000)])
      }
      try {
        fs.rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 })
      }
      catch {}
    },
  }
}

/** 页面里允许出现的控制台错误（与被测功能无关的环境噪声） */
const NOISE = [
  /Failed to load resource.*(favicon|\.map)/i,
  /DevTools|vue-devtools|__devtools__/i,
]

export class Page {
  constructor(conn, targetId, sessionId) {
    this.conn = conn
    this.targetId = targetId
    this.sessionId = sessionId
    /** 页面里的控制台 error 与未捕获异常 */
    this.errors = []
    /** 页面发出的全部请求的 URL（Network.requestWillBeSent），用来断言「没有请求某个接口」 */
    this.requests = []
    /** 网络拦截规则（intercept 添加）：{ substr, delayMs, status, body, times } */
    this.rules = []
    this.off = conn.on((msg) => {
      if (msg.sessionId !== sessionId)
        return
      if (msg.method === 'Fetch.requestPaused') {
        this.onPaused(msg.params)
        return
      }
      if (msg.method === 'Page.javascriptDialogOpening') {
        // 原生对话框会挡住页面（同一站点的其他标签页也一起卡住），Page.navigate、截图等命令都不再回应。
        // 离开有未保存修改的编辑器时弹出的「确定离开」（beforeunload）是测试主动跳转，直接确认；
        // 应用里不用 alert / confirm / prompt，出现了就记为错误，同样关掉免得卡住
        if (msg.params.type !== 'beforeunload')
          this.record(`原生对话框（${msg.params.type}）：${msg.params.message}`)
        this.send('Page.handleJavaScriptDialog', { accept: true }).catch(() => {})
        return
      }
      if (msg.method === 'Network.requestWillBeSent') {
        this.requests.push(msg.params.request.url)
        return
      }
      if (msg.method === 'Runtime.exceptionThrown') {
        const d = msg.params.exceptionDetails
        this.record(`未捕获异常：${d.exception?.description ?? d.text}`)
      }
      else if (msg.method === 'Runtime.consoleAPICalled' && msg.params.type === 'error') {
        this.record(`console.error：${msg.params.args.map(a => a.value ?? a.description ?? '').join(' ')}`)
      }
      else if (msg.method === 'Log.entryAdded' && msg.params.entry.level === 'error') {
        this.record(`${msg.params.entry.source}：${msg.params.entry.text} ${msg.params.entry.url ?? ''}`)
      }
    })
  }

  record(text) {
    if (!NOISE.some(re => re.test(text)))
      this.errors.push(text)
  }

  send(method, params) {
    return this.conn.send(method, params, this.sessionId)
  }

  async init() {
    await this.send('Page.enable')
    await this.send('Runtime.enable')
    await this.send('Log.enable')
    await this.send('Network.enable')
    await this.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false })
  }

  /**
   * 拦截 URL 含 substr 的请求：delayMs 先等一会儿再放行；给了 status 则不放行、直接以该状态码与 body（对象，按 JSON）回应。
   * times 限制生效次数（默认不限）。用来在界面里稳定地复现「查询很慢」「服务繁忙 503」，不依赖机器快慢
   */
  async intercept(substr, rule) {
    this.rules.push({ substr, ...rule })
    await this.send('Fetch.enable', { patterns: [{ urlPattern: '*' }] })
  }

  async clearIntercepts() {
    this.rules = []
    await this.send('Fetch.disable').catch(() => {})
  }

  async onPaused(p) {
    const rule = this.rules.find(r => p.request.url.includes(r.substr) && (r.times === undefined || r.times > 0))
    try {
      if (!rule) {
        await this.send('Fetch.continueRequest', { requestId: p.requestId })
        return
      }
      if (rule.times !== undefined)
        rule.times--
      if (rule.delayMs)
        await sleep(rule.delayMs)
      if (rule.status) {
        await this.send('Fetch.fulfillRequest', {
          requestId: p.requestId,
          responseCode: rule.status,
          responseHeaders: [{ name: 'Content-Type', value: 'application/json' }, { name: 'Access-Control-Allow-Origin', value: '*' }],
          body: Buffer.from(JSON.stringify(rule.body ?? {})).toString('base64'),
        })
      }
      else {
        await this.send('Fetch.continueRequest', { requestId: p.requestId })
      }
    }
    catch {
      // 页面已经取消了这个请求
    }
  }

  /** 真实鼠标点击（画布里的列头按钮等没有 DOM 元素的地方） */
  /** modifiers：Alt=1、Ctrl=2、Meta=4、Shift=8（可相加） */
  async mouseClick(x, y, modifiers = 0) {
    for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased'])
      await this.send('Input.dispatchMouseEvent', { type, x, y, button: type === 'mouseMoved' ? 'none' : 'left', clickCount: 1, modifiers })
  }

  async setCookie(name, value, url) {
    await this.send('Network.setCookie', { name, value, url, path: '/' })
  }

  async clearCookies() {
    await this.send('Network.clearBrowserCookies')
  }

  /** 打开地址并等页面 load 完（单页应用的内容加载另用 waitFor） */
  async goto(url) {
    const loaded = new Promise((resolve) => {
      const off = this.conn.on((m) => {
        if (m.sessionId === this.sessionId && m.method === 'Page.loadEventFired') {
          off()
          resolve()
        }
      })
    })
    await this.send('Page.navigate', { url })
    await Promise.race([loaded, sleep(30_000)])
  }

  /** 在页面里执行表达式（可含 await），返回可序列化的结果 */
  async eval(expression) {
    const r = await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true })
    if (r.exceptionDetails)
      throw new Error(`页面脚本出错：${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}\n${expression.slice(0, 200)}`)
    return r.result.value
  }

  /** 反复执行表达式直到结果为真，返回该结果；超时抛错并带上最后一次的值 */
  async waitFor(expression, { timeout = 20_000, interval = 150, what = expression } = {}) {
    const end = Date.now() + timeout
    let last
    while (Date.now() < end) {
      try {
        last = await this.eval(expression)
        if (last)
          return last
      }
      catch (e) {
        last = e.message
      }
      await sleep(interval)
    }
    throw new Error(`等待超时（${timeout / 1000}s）：${what}；最后一次结果：${JSON.stringify(last)?.slice(0, 300)}`)
  }

  /** 点第一个文字（含）匹配的元素；用页面脚本触发 click，Element Plus 的菜单、按钮都响应 */
  async click(selector, text) {
    const ok = await this.eval(`(() => {
      const els = [...document.querySelectorAll(${JSON.stringify(selector)})].filter(e => e.offsetParent !== null || getComputedStyle(e).position === 'fixed')
      const el = ${text === undefined ? 'els[0]' : `els.find(e => e.innerText && e.innerText.includes(${JSON.stringify(text)}))`}
      if (!el) return false
      el.click()
      return true
    })()`)
    if (!ok)
      throw new Error(`找不到可点击的元素：${selector}${text !== undefined ? `（含文字「${text}」）` : ''}`)
  }

  /** 给输入框赋值并触发 input 事件（Vue 的 v-model 能收到） */
  async fill(selector, value) {
    const ok = await this.eval(`(() => {
      const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find(e => e.offsetParent !== null)
      if (!el) return false
      const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, ${JSON.stringify(value)})
      el.dispatchEvent(new Event('input', { bubbles: true }))
      return true
    })()`)
    if (!ok)
      throw new Error(`找不到输入框：${selector}`)
  }

  /** 在当前焦点处插入文字（等同输入法上屏，会触发 input 事件；不经过 keydown，所以不会触发快捷键） */
  async type(text) {
    await this.send('Input.insertText', { text })
  }

  /** 真实键盘事件：press('s', { ctrl: true })；code 用物理键名（如 KeyQ），默认由 key 推出 */
  async press(key, { ctrl = false, alt = false, meta = false, shift = false, code } = {}) {
    const modifiers = (alt ? 1 : 0) | (ctrl ? 2 : 0) | (meta ? 4 : 0) | (shift ? 8 : 0)
    const info = { key, code: code ?? (key.length === 1 ? `Key${key.toUpperCase()}` : key), modifiers, windowsVirtualKeyCode: key.length === 1 ? key.toUpperCase().charCodeAt(0) : ({ Escape: 27, Enter: 13 }[key] ?? 0) }
    await this.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...info })
    await this.send('Input.dispatchKeyEvent', { type: 'keyUp', ...info })
  }

  async screenshot(file) {
    const { data } = await this.send('Page.captureScreenshot', { format: 'png' })
    fs.mkdirSync(path.dirname(file), { recursive: true })
    fs.writeFileSync(file, Buffer.from(data, 'base64'))
    return file
  }

  async close() {
    this.off()
    await this.conn.send('Target.closeTarget', { targetId: this.targetId }).catch(() => {})
  }
}

export { sleep }
