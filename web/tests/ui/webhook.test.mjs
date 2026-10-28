// 事件 Webhook（界面）：首页「事件 Webhook」→ 新建（选事件）→ 显示签名密钥 → 测试 → 导入数据集后发送记录里有 dataset.created
import assert from 'node:assert/strict'
import http from 'node:http'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { api, PORTS, startServices, TOKEN, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
const COOKIE = 'gridlake-token'
const HOOK_PORT = PORTS.ledger + 400
const ALLOWED = [/command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]

describe('事件 Webhook（界面）', { skip }, () => {
  let services
  let browser
  let receiver
  const got = []

  before(async () => {
    receiver = http.createServer((req, res) => {
      let body = ''
      req.on('data', c => (body += c))
      req.on('end', () => {
        got.push({ headers: req.headers, body: JSON.parse(body) })
        res.end('ok')
      })
    })
    await new Promise(r => receiver.listen(HOOK_PORT, '127.0.0.1', r))
    // 接收器在本机：放开本机地址（生产环境不要这样设）
    services = await startServices({ env: { LEDGER_URL_IMPORT_ALLOW_LOCAL: '1' } })
    browser = await launchChrome()
    await api.registerUsers()
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      await services?.stop()
      receiver?.close()
    }
  })

  it('新建 → 密钥 → 测试 → 发送记录', async () => {
    const page = await browser.newPage()
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.root, WEB)
      await page.goto(`${WEB}/excel/home?lang=zh-CN`)
      await page.waitFor(`[...document.querySelectorAll('button.nav-item')].some(e => e.innerText.includes('自动化'))`, { what: '首页导航', timeout: 60_000 })
      await page.click('button.nav-item', '自动化')
      await page.waitFor(`!!document.querySelector('[data-testid="home-webhooks"]')`, { what: '首页「事件 Webhook」卡片（管理员）', timeout: 60_000 })
      await page.click('[data-testid="home-webhooks"]')
      await page.waitFor(`document.body.innerText.includes('还没有 Webhook')`, { what: 'Webhook 对话框（空）' })
      await page.click('[data-testid="webhook-new"]')
      await page.waitFor(`!!document.querySelector('input[data-testid="webhook-name"]')`, { what: '新建表单' })
      await page.fill('input[data-testid="webhook-name"]', '数据管道')
      await page.fill('input[data-testid="webhook-url"]', `http://127.0.0.1:${HOOK_PORT}/hook?token=abc`)
      await page.click('[data-testid="webhook-event-dataset.created"]')
      await page.click('[data-testid="webhook-save"]')
      await page.waitFor(`!!document.querySelector('[data-testid="webhook-secret"]') && document.querySelector('[data-testid="webhook-secret"]').innerText.includes('whsec_')`, { what: '显示签名密钥' })
      await page.waitFor(`!!document.querySelector('[data-testid="webhook-数据管道"]')`, { what: '列表里有新 Webhook' })
      const card = await page.eval(`document.querySelector('[data-testid="webhook-数据管道"]').innerText`)
      assert.match(card, /hook\?…/, '地址里的查询参数打码')
      assert.match(card, /新建数据集/, '显示订阅的事件')

      await page.click('[data-testid="webhook-数据管道"] [data-testid="webhook-test"]')
      await page.waitFor(`document.body.innerText.includes('测试成功：对方返回 HTTP 200')`, { what: '测试成功提示', timeout: 30_000 })
      assert.equal(got.at(-1).body.event, 'ping')

      await api.importDataset(TOKEN.root, '订单', '店铺,金额\nA,1\n')
      const t0 = Date.now()
      while (!got.some(g => g.body.event === 'dataset.created') && Date.now() - t0 < 10_000)
        await new Promise(r => setTimeout(r, 100))
      assert.ok(got.some(g => g.body.event === 'dataset.created' && g.body.data.name === '订单'), '接收方收到 dataset.created')

      await page.click('[data-testid="webhook-数据管道"] [data-testid="webhook-deliveries"]')
      await page.waitFor(`(() => { const t = document.querySelector('[data-testid="webhook-delivery-table"]'); return !!t && t.innerText.includes('新建数据集') && t.innerText.includes('测试') && t.innerText.includes('成功') })()`, { what: '发送记录：测试 + 新建数据集，都成功' })
      await page.screenshot(path.join(ARTIFACTS, 'webhook-deliveries.png')).catch(() => {})
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'webhook-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
