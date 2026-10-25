// 汇总行（界面）：状态栏「Σ 汇总行」打开 → 金额列默认合计 → 点格子改成平均、店铺列改成不同值 → 搜索后按筛选重算 → 刷新页面设置还在
import assert from 'node:assert/strict'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { api, startServices, TOKEN, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
const COOKIE = 'gridlake-token'
const ALLOWED = [/command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]
const CSV = '单号,店铺,金额\n1,北京店,10\n2,北京店,10\n3,北京,20\n4,上海店,30\n5,上海店,30\n6,上海店,40\n'

describe('汇总行（界面）', { skip }, () => {
  let services
  let browser
  let datasetId

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    datasetId = await api.importDataset(TOKEN.alice, '订单', CSV)
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  it('打开汇总行 → 改汇总方式 → 按搜索重算 → 刷新后保留', async () => {
    const page = await browser.newPage()
    const cell = col => `(document.querySelector('[data-testid="grid-footer"] [data-col="${col}"]')?.innerText ?? '').replace(/\\s+/g, ' ')`
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?ds=${datasetId}&lang=zh-CN`)
      await page.waitFor(`document.body.innerText.includes('6 行')`, { timeout: 90_000, what: '打开数据表' })
      assert.equal(await page.eval(`!!document.querySelector('[data-testid="grid-footer"]')`), false, '默认不显示汇总行')
      await page.click('[data-testid="toggle-totals"]')
      await page.waitFor(`${cell('金额')} === '合计 140.00'`, { what: '金额列默认合计 140.00', timeout: 30_000 })
      assert.equal(await page.eval(cell('店铺')), '', '文本列默认不汇总')
      assert.equal(await page.eval(cell('单号')), '', '编号列（整数）默认不求和')

      await page.click('[data-testid="grid-footer"] [data-col="店铺"]')
      await page.waitFor(`!!document.querySelector('[data-testid="total-menu"]')`, { what: '汇总方式菜单' })
      await page.click('[data-testid="total-menu"] button', '不同值')
      await page.waitFor(`${cell('店铺')} === '不同值 3'`, { what: '店铺列：不同值 3', timeout: 30_000 })
      await page.click('[data-testid="grid-footer"] [data-col="金额"]')
      await page.click('[data-testid="total-menu"] button', '平均')
      await page.waitFor(`${cell('金额')} === '平均 23.33'`, { what: '金额列：平均 23.33', timeout: 30_000 })
      await page.screenshot(path.join(ARTIFACTS, 'totals.png')).catch(() => {})

      await page.fill('input[placeholder*="搜索"]', '北京')
      await page.waitFor(`document.body.innerText.includes('3 行') && ${cell('金额')} === '平均 13.33' && ${cell('店铺')} === '不同值 2'`, { what: '搜索后按 3 行重算', timeout: 30_000 })

      await page.goto(`${WEB}/excel?ds=${datasetId}&lang=zh-CN`)
      await page.waitFor(`${cell('金额')} === '平均 23.33' && ${cell('店铺')} === '不同值 3'`, { what: '刷新页面：汇总行和每列的选择都还在', timeout: 90_000 })
      await page.click('[data-testid="grid-footer"] [data-col="店铺"]')
      await page.click('[data-testid="total-menu"] button', '不汇总')
      await page.waitFor(`${cell('店铺')} === ''`, { what: '改成不汇总' })
      await page.click('[data-testid="toggle-totals"]')
      await page.waitFor(`!document.querySelector('[data-testid="grid-footer"]')`, { what: '关闭汇总行' })
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'totals-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
