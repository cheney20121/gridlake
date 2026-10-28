// 界面测试的运行环境：模拟登录网关 + 表格服务（全新临时数据库）+ 前端开发服务器，以及造数据用的接口封装。
// 端口刻意避开日常开发用的（前端 5188/5189、服务 8787/8788、网关 19001），可以和手动开发同时运行。
import { Buffer } from 'node:buffer'
import { spawn, spawnSync } from 'node:child_process'
import fs from 'node:fs'
import http from 'node:http'
import net from 'node:net'
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import zlib from 'node:zlib'

const here = path.dirname(fileURLToPath(import.meta.url))
export const WEB_DIR = path.resolve(here, '../..')
const SERVER_DIR = path.resolve(WEB_DIR, '../server')

// UITEST_PORT_OFFSET：整体平移端口，方便同时跑多份界面测试（如两个检出目录、或测试时本机正在手动开发）
const PORT_OFFSET = Number(process.env.UITEST_PORT_OFFSET || 0)
export const PORTS = { gateway: 19101 + PORT_OFFSET, ledger: 18901 + PORT_OFFSET, web: 5291 + PORT_OFFSET, llm: 19102 + PORT_OFFSET, oidc: 19103 + PORT_OFFSET, s3: 19104 + PORT_OFFSET }
export const WEB = `http://127.0.0.1:${PORTS.web}`
export const API = `http://127.0.0.1:${PORTS.ledger}/api`
/** mock_gateway.py 里的四个用户：Alice / Bob / Carol 是普通用户，Root 是管理员 */
/** 内置账号模式（startServices({ auth: 'local' })）首次启动时创建的管理员 */
export const LOCAL_ADMIN = { username: 'admin', password: 'Admin-pass-1' }

export const TOKEN = { alice: 'tok-alice', bob: 'tok-bob', carol: 'tok-carol', root: 'tok-root' }

const sleep = ms => new Promise(r => setTimeout(r, ms))

/**
 * 脚本化的模拟模型（OpenAI 兼容接口）：测试用 llm.say(...) 排好后面几轮要返回的内容，AI 助手每调用一次取一条。
 * 不带 tools 的调用（服务端整理对话摘要）不占脚本。
 */
export const llm = {
  script: [],
  /** 收到的带工具的请求体（检查发给模型的提示与界面上下文） */
  requests: [],
  /** 下一轮：调用工具 [{ name, args }]，或直接回答文字 */
  say({ text = '', calls = [] }) {
    this.script.push({ text, calls })
  },
  reset() {
    this.script.length = 0
    this.requests.length = 0
  },
}

function startMockLlm() {
  const server = http.createServer((req, res) => {
    let raw = ''
    req.on('data', d => (raw += d))
    req.on('end', () => {
      const body = JSON.parse(raw || '{}')
      if (body.tools)
        llm.requests.push(body)
      const step = body.tools ? (llm.script.shift() ?? { text: '（脚本用完）', calls: [] }) : { text: '摘要', calls: [] }
      const message = { role: 'assistant', content: step.text }
      if (step.calls.length)
        message.tool_calls = step.calls.map((c, i) => ({ id: `call_${Date.now()}_${i}`, type: 'function', function: { name: c.name, arguments: JSON.stringify(c.args) } }))
      const out = JSON.stringify({ choices: [{ index: 0, message }] })
      res.writeHead(200, { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(out) })
      res.end(out)
    })
  })
  server.listen(PORTS.llm, '127.0.0.1')
  return server
}

/** 端口是否空着（能在 127.0.0.1 上监听） */
function portFree(port) {
  return new Promise((resolve) => {
    const srv = net.createServer()
    srv.once('error', () => resolve(false))
    srv.listen(port, '127.0.0.1', () => srv.close(() => resolve(true)))
  })
}

/**
 * 启动前确认测试端口都空着：上一个测试文件的服务还没退完就等一会儿；一直被占用就直接报错。
 * 否则会连上残留的旧服务（旧数据库、旧前端），后面的用例成片莫名失败或挂住
 */
async function ensurePortsFree(ports, timeoutMs = 15_000) {
  const end = Date.now() + timeoutMs
  let busy = ports
  while (true) {
    busy = (await Promise.all(busy.map(async p => (await portFree(p)) ? null : p))).filter(Boolean)
    if (!busy.length)
      return
    if (Date.now() > end)
      throw new Error(`界面测试用的端口被占用：${busy.join('、')}。可能是上一次测试残留的进程（用 lsof -i :${busy[0]} 查看并结束），或设置 UITEST_PORT_OFFSET 换一组端口`)
    await sleep(300)
  }
}

/** 测试进程退出时还没停掉的服务（进程组）与临时目录：无论测试怎么结束（被取消、after 没跑到、Ctrl+C）都不留下占着端口的进程 */
const live = new Set()
const tmpDirs = new Set()
function killGroup(p, signal) {
  try {
    process.kill(-p.pid, signal)
  }
  catch {
    try {
      p.kill(signal)
    }
    catch {}
  }
}
process.once('exit', () => {
  for (const p of live) killGroup(p, 'SIGKILL')
  for (const d of tmpDirs) {
    try {
      fs.rmSync(d, { recursive: true, force: true })
    }
    catch {}
  }
})
for (const sig of ['SIGINT', 'SIGTERM', 'SIGHUP'])
  process.once(sig, () => process.exit(128 + os.constants.signals[sig]))

async function waitHttp(url, timeoutMs, what) {
  const end = Date.now() + timeoutMs
  while (Date.now() < end) {
    try {
      const r = await fetch(url)
      if (r.status < 500)
        return
    }
    catch {}
    await sleep(300)
  }
  throw new Error(`等待超时：${what}（${url}）`)
}

/** auth：gateway（默认，模拟登录网关 + 固定令牌）/ local（内置账号，管理员见 LOCAL_ADMIN）；oidc：再配上模拟的单点登录身份提供方（只在 local 下有意义） */
/** packs：服务端的 LEDGER_PACKS。界面测试默认启用电商场景包（多数用例要用台账），测「没启用」时传 'none' */
export async function startServices({ auth = 'gateway', packs = 'ecommerce', oidc = false, env: extraEnv = {} } = {}) {
  // UI_DB：指定一个现成的数据库（如 1 亿行压测库）供手动查看，退出时不删除；默认用临时目录
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ui-test-data-'))
  tmpDirs.add(tmp)
  const dbPath = process.env.UI_DB || path.join(tmp, 'ledger.duckdb')
  const procs = []
  const logs = {}
  const start = (name, cmd, args, opts) => {
    // detached：各自一个进程组，停的时候连同子进程（vite 的 esbuild 等）一起结束
    const p = spawn(cmd, args, { ...opts, stdio: ['ignore', 'pipe', 'pipe'], detached: process.platform !== 'win32' })
    logs[name] = ''
    p.stdout.on('data', d => (logs[name] += d))
    p.stderr.on('data', d => (logs[name] += d))
    p.exited = new Promise(r => p.once('exit', r))
    p.exited.then(() => live.delete(p))
    live.add(p)
    procs.push(p)
    return p
  }

  await ensurePortsFree([PORTS.gateway, PORTS.ledger, PORTS.web, PORTS.llm, PORTS.s3, ...(oidc ? [PORTS.oidc] : [])])

  // 表格服务：优先用环境变量 LEDGER_BIN，否则增量编译源码目录里的 debug 版
  let bin = process.env.LEDGER_BIN
  if (!bin) {
    const r = spawnSync('cargo', ['build'], { cwd: SERVER_DIR, encoding: 'utf8' })
    if (r.status !== 0)
      throw new Error(`编译表格服务失败：\n${r.stderr.slice(-2000)}`)
    bin = path.join(SERVER_DIR, 'target/debug/gridlake')
  }

  // 模拟 S3 服务（对象存储连接器的界面测试用；桶 data、Access Key AKIATEST、Secret Key s3cr3t-key）
  start('s3', 'python3', [path.join(SERVER_DIR, 'tests/e2e/mock_s3.py'), String(PORTS.s3)])
  if (oidc)
    start('oidc', 'python3', [path.join(SERVER_DIR, 'tests/e2e/mock_oidc.py'), String(PORTS.oidc)])
  start('gateway', 'python3', [path.join(SERVER_DIR, 'tests/e2e/mock_gateway.py'), String(PORTS.gateway)])
  start('ledger', bin, [], {
    env: {
      ...process.env,
      ...(auth === 'local'
        ? { LEDGER_AUTH_MODE: 'local', LEDGER_ADMIN_USER: LOCAL_ADMIN.username, LEDGER_ADMIN_PASSWORD: LOCAL_ADMIN.password }
        : { LEDGER_AUTH_MODE: 'gateway', LEDGER_AUTH_URL: `http://127.0.0.1:${PORTS.gateway}/current_user` }),
      // 单点登录：回调地址是浏览器能访问到的页面同源地址（经 vite 代理到表格服务）
      ...(oidc
        ? {
            LEDGER_OIDC_ISSUER: `http://127.0.0.1:${PORTS.oidc}`,
            LEDGER_OIDC_CLIENT_ID: 'gridlake',
            LEDGER_OIDC_CLIENT_SECRET: 's3cret',
            LEDGER_OIDC_REDIRECT_URL: `http://127.0.0.1:${PORTS.web}/ledger-api/api/ledger/auth/oidc/callback`,
            LEDGER_OIDC_ADMIN_GROUP: 'gl-admins',
            LEDGER_OIDC_NAME: 'Mock-SSO',
          }
        : {}),
      ...extraEnv,
      LEDGER_ADDR: `127.0.0.1:${PORTS.ledger}`,
      LEDGER_DB: dbPath,
      LEDGER_PACKS: packs,
      LEDGER_BACKUP_AT: 'off',
      // AI 助手指向本机的模拟模型（见上面的 llm）
      LEDGER_AI_BASE_URL: `http://127.0.0.1:${PORTS.llm}/v1`,
      LEDGER_AI_MODEL: 'mock-model',
      LEDGER_AI_STREAM: '0',
    },
  })
  const llmServer = startMockLlm()
  start('web', process.execPath, ['node_modules/vite/bin/vite.js', '--mode', 'uitest', '--host', '127.0.0.1', '--port', String(PORTS.web), '--strictPort'], {
    cwd: WEB_DIR,
    env: { ...process.env, VITE_LEDGER_SERVER: `http://127.0.0.1:${PORTS.ledger}` },
  })

  // 停掉全部服务并等它们真正退出（端口释放后下一个测试文件才能启动）；可重复调用
  let stopping = null
  const stop = () => stopping ??= (async () => {
    const running = procs.filter(p => p.exitCode === null && p.signalCode === null)
    for (const p of running) killGroup(p, 'SIGTERM')
    llmServer.closeAllConnections?.()
    llmServer.close()
    const all = Promise.all(running.map(p => p.exited))
    if (await Promise.race([all.then(() => true), sleep(5000).then(() => false)]) === false) {
      for (const p of running) killGroup(p, 'SIGKILL')
      await Promise.race([all, sleep(5000)])
    }
    fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 })
    tmpDirs.delete(tmp)
  })()
  try {
    await waitHttp(`http://127.0.0.1:${PORTS.gateway}/webconfig`, 15_000, '模拟网关')
    if (oidc)
      await waitHttp(`http://127.0.0.1:${PORTS.oidc}/__last`, 15_000, '模拟身份提供方')
    await waitHttp(`${API}/health`, 30_000, '表格服务')
    await waitHttp(`${WEB}/`, 180_000, '前端开发服务器')
  }
  catch (e) {
    await stop()
    throw new Error(`${e.message}\n--- 日志 ---\n${Object.entries(logs).map(([k, v]) => `[${k}] ${v.slice(-1200)}`).join('\n')}`)
  }
  return { stop, logs }
}

// ── 造数据 ────────────────────────────────────────────

async function call(method, pathname, token, { body, raw, form } = {}) {
  const headers = { 'x-user-token': token }
  let payload
  if (form) {
    payload = form
  }
  else if (raw) {
    payload = raw
    headers['Content-Type'] = 'application/octet-stream'
  }
  else if (body !== undefined) {
    payload = JSON.stringify(body)
    headers['Content-Type'] = 'application/json'
  }
  const r = await fetch(API + pathname, { method, headers, body: payload })
  const text = await r.text()
  let json
  try {
    json = JSON.parse(text)
  }
  catch {
    json = text
  }
  return { status: r.status, body: json, data: json?.data }
}

export const api = {
  call,
  /** 让网关里的用户在表格服务登记（审批人列表里才选得到）；返回 { 名字: id } */
  async registerUsers() {
    for (const t of Object.values(TOKEN)) await call('GET', '/ledger/me', t)
    await sleep(800)
    const users = (await call('GET', '/ledger/users', TOKEN.root)).data
    return Object.fromEntries(users.map(u => [u.name, u.id]))
  },

  /** 新建工作簿：cells 为 [[A1, B1, ...], [A2, ...]] */
  async createWorkbook(token, name, cells = [['订单号', '金额'], ['A1', 100], ['A2', 250.5]], sheetName = 'Sheet1') {
    const cellData = {}
    cells.forEach((row, r) => {
      cellData[r] = {}
      row.forEach((v, c) => (cellData[r][c] = { v }))
    })
    const json = { id: 'wb', name, sheetOrder: ['s1'], sheets: { s1: { id: 's1', name: sheetName, cellData, rowCount: 100, columnCount: 20 } } }
    const gz = zlib.gzipSync(Buffer.from(JSON.stringify(json)))
    const r = await call('POST', `/ledger/workbooks?name=${encodeURIComponent(name)}`, token, { raw: gz })
    if (r.status !== 200)
      throw new Error(`新建工作簿失败：${JSON.stringify(r.body)}`)
    return r.data
  },

  async shareWorkbook(token, id, visibility) {
    return (await call('PUT', `/ledger/workbooks/${id}/share`, token, { body: { visibility } })).data
  },

  /** 导入 CSV 为数据集，返回数据集 id */
  async importDataset(token, name, csv) {
    const form = new FormData()
    form.append('file', new Blob([csv], { type: 'text/csv' }), `${name}.csv`)
    const p = await call('POST', '/ledger/datasets/import/preview', token, { form })
    if (p.status !== 200)
      throw new Error(`导入预览失败：${JSON.stringify(p.body)}`)
    const preview = p.data
    const c = await call('POST', '/ledger/datasets/import/commit', token, {
      body: { token: preview.token, mode: 'create', name, columns: preview.columns.map(x => ({ source: x.source, title: x.title, kind: x.kind })), primaryKey: [], fileName: preview.fileName },
    })
    if (c.status !== 200)
      throw new Error(`导入失败：${JSON.stringify(c.body)}`)
    return c.data.datasetId
  },

  /** 新建对账（逐笔对账），返回结果数据集 id */
  async createRecon(token, name, def) {
    const r = await call('POST', '/ledger/recons', token, { body: { name, def } })
    if (r.status !== 200)
      throw new Error(`新建对账失败：${JSON.stringify(r.body)}`)
    return r.data
  },

  async autoRecon(token, id) {
    const r = await call('POST', `/ledger/recons/${id}/auto`, token, { body: {} })
    if (r.status !== 200)
      throw new Error(`自动勾对失败：${JSON.stringify(r.body)}`)
    return r.data
  },

  /** 按摘要里的文字找一侧未勾对的行标识 */
  async reconRowKey(token, id, side, q) {
    const r = await call('GET', `/ledger/recons/${id}/items?side=${side}&status=open&q=${encodeURIComponent(q)}`, token)
    if (r.status !== 200 || r.data.total !== 1)
      throw new Error(`找不到唯一的行「${q}」：${JSON.stringify(r.body)}`)
    return r.data.rows[0].key
  },

  async matchRecon(token, id, body) {
    const r = await call('POST', `/ledger/recons/${id}/match`, token, { body })
    if (r.status !== 200)
      throw new Error(`手工勾对失败：${JSON.stringify(r.body)}`)
    return r.data
  },

  async bindDataset(datasetId, { column, open }) {
    return call('PUT', `/ledger/data-scope/datasets/${datasetId}`, TOKEN.root, { body: { scopeColumn: column ?? null, open: !!open } })
  },

  async setScope(userId, shops) {
    return call('PUT', '/ledger/data-scope/rules', TOKEN.root, { body: { subjectId: userId, mode: shops ? 'shops' : 'all', shops: shops ?? [] } })
  },

  async workbookMeta(token, id) {
    return (await call('GET', `/ledger/workbooks/${id}`, token)).data
  },
}
