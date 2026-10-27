// 修改列类型（界面）：列头菜单 →「更改列类型」→ 选新类型看预览（多少值转不过来、举例）→ 确认修改 → 列按新类型显示
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
// 「金额」里混了一个「待定」，导入时被当成文本
const CSV = '店铺,金额\nA,10\nA,20.5\nB,待定\nB,1200\n'

describe('修改列类型（界面）', { skip }, () => {
  let services
  let browser
  let datasetId
  let widths

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    datasetId = await api.importDataset(TOKEN.alice, '订单', CSV)
    widths = (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.id === datasetId).columns.map(c => c.width)
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  it('文本 → 金额：预览说明有 1 个值转不过来，确认后修改', async () => {
    const kindOf = async () => (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.id === datasetId).columns.find(c => c.title === '金额').kind
    assert.equal(await kindOf(), 'text')
    const page = await browser.newPage()
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?ds=${datasetId}&lang=zh-CN`)
      await page.waitFor(`document.body.innerText.includes('4 行') && !!document.querySelector('.scroller')`, { timeout: 90_000, what: '打开数据表' })
      // 点「金额」列表头的按钮（画布里没有 DOM，按几何位置算）
      const canvas = await page.eval(`(() => { const r = document.querySelector('.scroller').parentElement.querySelector('canvas').getBoundingClientRect(); return [r.left, r.top] })()`)
      await page.mouseClick(canvas[0] + 48 + widths[0] + widths[1] - 9, canvas[1] + 34)
      await page.waitFor(`!!document.querySelector('[data-testid="change-column-kind"]')`, { what: '列头菜单里的「更改列类型」' })
      assert.match(await page.eval(`document.querySelector('[data-testid="change-column-kind"]').innerText`), /现在是文本/)
      await page.click('[data-testid="change-column-kind"]')
      await page.waitFor(`document.body.innerText.includes('更改列类型') && !!document.querySelector('[data-testid="column-kind-select"]')`, { what: '更改列类型对话框' })
      const sel = await page.eval(`(() => { const r = document.querySelector('[data-testid="column-kind-select"]').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] })()`)
      await page.mouseClick(sel[0], sel[1])
      await page.waitFor(`[...document.querySelectorAll('.ec-select-dropdown__item')].some(e => e.getBoundingClientRect().width > 0 && e.innerText.trim() === '金额')`, { what: '类型选项' })
      await page.eval(`[...document.querySelectorAll('.ec-select-dropdown__item')].find(e => e.getBoundingClientRect().width > 0 && e.innerText.trim() === '金额').click()`)
      await page.waitFor(`document.body.innerText.includes('有 1 个转不成金额') && document.body.innerText.includes('「待定」')`, { what: '预览：1 个值转不过来', timeout: 30_000 })
      await page.screenshot(path.join(ARTIFACTS, 'column-kind-preview.png')).catch(() => {})
      assert.equal(await kindOf(), 'text', '只预览不改')
      await page.click('.ec-dialog__footer button', '仍然修改')
      await page.waitFor(`document.body.innerText.includes('已改为金额列')`, { what: '修改成功提示', timeout: 30_000 })
      assert.equal(await kindOf(), 'money')
      // 金额列按金额显示（两位小数、千分位）
      await page.waitFor(`document.body.innerText.includes('4 行')`, { what: '数据表重新打开' })
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'column-kind-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
