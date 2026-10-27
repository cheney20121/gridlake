// 窗口计算（界面）：数据表「更多 → 窗口计算」→ 默认按日期累计金额，选「按店铺分组」→ 预览 → 生成数据集并打开
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
const CSV = '店铺,日期,金额\nA,2026-09-01 10:00:00,10\nA,2026-09-02 10:00:00,20\nA,2026-09-03 10:00:00,40\nB,2026-09-01 10:00:00,100\nB,2026-09-02 10:00:00,50\n'

/** 下拉框：等位置稳定（对话框的打开动画结束）后用真实鼠标点开；没弹出来就再点一次 */
async function pick(page, selector, text) {
  const rect = `(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return [Math.round(r.left), Math.round(r.top), r.width, r.height] })()`
  let last = ''
  for (let i = 0; i < 20; i++) {
    const now = JSON.stringify(await page.eval(rect))
    if (now === last)
      break
    last = now
    await new Promise(r => setTimeout(r, 100))
  }
  const visible = `[...document.querySelectorAll('.ec-select-dropdown__item')].find(e => e.getBoundingClientRect().width > 0 && e.innerText.trim() === ${JSON.stringify(text)})`
  for (let attempt = 0; ; attempt++) {
    const [x, y, w, h] = await page.eval(rect)
    await page.mouseClick(x + w / 2, y + h / 2)
    try {
      await page.waitFor(`!!${visible}`, { what: `选项「${text}」`, timeout: 4000 })
      break
    }
    catch (e) {
      if (attempt >= 2)
        throw e
    }
  }
  await page.eval(`${visible}.click()`)
}

describe('窗口计算（界面）', { skip }, () => {
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

  it('按店铺累计金额 → 预览 → 生成并打开', async () => {
    const page = await browser.newPage()
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?ds=${datasetId}&lang=zh-CN`)
      await page.waitFor(`document.body.innerText.includes('5 行')`, { timeout: 90_000, what: '打开数据表' })
      await page.click('button', '更多 ▾')
      await page.waitFor(`!!document.querySelector('[data-testid="open-window"]')?.offsetParent`, { what: '更多菜单里的窗口计算' })
      await page.click('[data-testid="open-window"]')
      await page.waitFor(`!!document.querySelector('[data-testid="window-dialog"]')`, { what: '窗口计算对话框' })
      // 默认：累计、值列是第一个数值列（金额）、按日期排序，新列名自动起好
      assert.equal(await page.eval(`document.querySelector('[data-testid="window-calc-0"] input[maxlength="40"]').value`), '金额累计')
      await pick(page, '[data-testid="window-partition"]', '店铺')
      await page.click('[data-testid="window-preview-btn"]')
      await page.waitFor(`(document.querySelector('[data-testid="window-preview"]')?.innerText ?? '').includes('金额累计')`, { what: '预览表格里有新列', timeout: 60_000 })
      const preview = await page.eval(`document.querySelector('[data-testid="window-preview"]').innerText`)
      assert.match(preview, /70/, 'A 店到第三天累计 70')
      assert.match(preview, /150/, 'B 店累计 150')
      await page.screenshot(path.join(ARTIFACTS, 'window-calc.png')).catch(() => {})
      await page.click('[data-testid="window-create"]')
      await page.waitFor(`document.body.innerText.includes('已生成「订单（窗口计算）」')`, { what: '生成成功提示', timeout: 60_000 })
      await page.waitFor(`[...document.querySelectorAll('.data-panel')].some(e => e.innerText.includes('订单（窗口计算）'))`, { what: '数据面板里有新数据集', timeout: 30_000 })
      const ds = (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.name === '订单（窗口计算）')
      assert.equal(ds?.rowCount, 5)
      assert.equal(ds.columns.find(c => c.title === '金额累计')?.kind, 'money')
      const max = (await api.call('POST', `/ledger/datasets/${ds.id}/aggregate`, TOKEN.alice, { body: { calls: [{ func: 'max', column: '金额累计', criteria: [{ column: '店铺', value: 'A' }] }] } })).data.values[0]
      assert.equal(max, 70)
      await page.waitFor(`location.search.includes('${ds.id}') || document.body.innerText.includes('订单（窗口计算）')`, { what: '打开新数据集' })
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'window-calc-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
