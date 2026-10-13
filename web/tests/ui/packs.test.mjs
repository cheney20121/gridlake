// 场景包：没启用电商场景包时，界面里没有台账、月结、店铺档案等入口，打开工作簿也不会请求台账接口
import assert from 'node:assert/strict'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { LOCAL_ADMIN, startServices, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
const ALLOWED = [/401|Unauthorized/i, /command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]

describe('场景包关闭（界面）', { skip }, () => {
  let services
  let browser
  let page

  before(async () => {
    services = await startServices({ auth: 'local', packs: 'none' })
    browser = await launchChrome()
    page = await browser.newPage()
    await page.clearCookies()
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  it('没有台账入口；新建工作簿打开编辑器不请求台账接口、没有报错；通用功能入口都在', async () => {
    try {
      await page.goto(`${WEB}/login`)
      await page.waitFor(`!!document.querySelector('input[data-testid="login-username"]')`, { what: '登录表单' })
      await page.fill('input[data-testid="login-username"]', LOCAL_ADMIN.username)
      await page.fill('input[data-testid="login-password"]', LOCAL_ADMIN.password)
      await page.click('[data-testid="login-submit"]')
      await page.waitFor(`location.pathname === '/excel/home'`, { what: '首页', timeout: 60_000 })
      assert.equal(await page.eval(`fetch('/ledger-api/api/ledger/me').then(r => r.json()).then(j => j.data.packs.ecommerce)`), false, '/me 报告场景包未启用')
      const home = await page.eval(`document.body.innerText`)
      for (const word of ['店铺资金台账', '月结中心', '店铺档案'])
        assert.ok(!home.includes(word), `首页不应出现「${word}」`)
      assert.ok(home.includes('数据权限（行级）') || home.includes('自动化'), '通用的自动化区还在')

      await page.click('button', '新建工作簿')
      await page.waitFor(`!!document.querySelector('.ec-message-box')`, { what: '命名对话框' })
      await page.click('.ec-message-box__btns button', '确定')
      await page.waitFor(`location.pathname === '/excel' && location.search.includes('wb=')`, { what: '进入编辑器', timeout: 60_000 })
      await page.waitFor(`!!document.querySelector('.palette-entry')`, { what: '编辑器顶栏', timeout: 60_000 })
      await page.waitFor(`!!document.querySelector('[data-testid="data-panel"]') || document.body.innerText.includes('数据')`, { what: '数据面板' })
      const body = await page.eval(`document.body.innerText`)
      assert.ok(!body.includes('月结中心') && !body.includes('导入店铺明细'), '编辑器里没有台账入口')
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错（不会因为台账接口 404 报错）')
      const ledgerCalls = page.requests.filter(u => /\/api\/ledger\/(?:periods|shops|close|coverage|imports|details|balances|withdrawals|analysis)/.test(u))
      assert.deepEqual(ledgerCalls, [], '没有请求台账接口')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'packs-off-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 180_000 })
})
