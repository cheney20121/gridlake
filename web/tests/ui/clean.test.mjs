// 数据清洗（界面）：「更多 → 数据清洗」→ 删除重复行（预览 → 确认 → 撤销）→ 查找替换统一写法
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
const CSV = '单号,店铺,金额\n1,北京店,10\n1,北京店,10\n2,北京,20\n3,上海店,30\n3,上海店,30\n4,上海店,40\n'

describe('数据清洗（界面）', { skip }, () => {
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

  it('删除重复行 → 撤销 → 查找替换', async () => {
    const page = await browser.newPage()
    const previewText = `document.querySelector('[data-testid="clean-preview"]')?.innerText ?? ''`
    const enabled = `document.querySelector('[data-testid="clean-apply"]')?.disabled === false`
    const rows = async () => (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.id === datasetId).rowCount
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?ds=${datasetId}&lang=zh-CN`)
      await page.waitFor(`document.body.innerText.includes('6 行')`, { timeout: 90_000, what: '打开数据表' })
      await page.click('button', '更多 ▾')
      await page.waitFor(`!!document.querySelector('[data-testid="open-clean"]')?.offsetParent`, { what: '更多菜单里的数据清洗' })
      await page.click('[data-testid="open-clean"]')
      await page.waitFor(`(${previewText}).includes('将删除 2 行（共 6 行）')`, { what: '默认删除重复行：预览 2 行', timeout: 30_000 })
      await page.screenshot(path.join(ARTIFACTS, 'clean-dedupe.png')).catch(() => {})
      await page.waitFor(enabled, { what: '执行按钮可点（预览算完）' })
      await page.click('[data-testid="clean-apply"]')
      await page.waitFor(`[...document.querySelectorAll('.ec-message-box')].some(e => e.offsetParent && e.innerText.includes('将删除 2 行'))`, { what: '删除前确认' })
      await page.click('.ec-message-box__btns button', '删除')
      await page.waitFor(`document.body.innerText.includes('已删除 2 行')`, { what: '删除成功提示', timeout: 30_000 })
      assert.equal(await rows(), 4)
      await page.waitFor(`document.body.innerText.includes('4 行')`, { what: '数据表刷新成 4 行', timeout: 30_000 })
      await page.waitFor(`!!document.querySelector('[data-testid="clean-undo"]')`, { what: '可以撤销' })
      await page.click('[data-testid="clean-undo"]')
      await page.waitFor(`document.body.innerText.includes('已撤销')`, { what: '撤销成功', timeout: 30_000 })
      assert.equal(await rows(), 6, '撤销后 6 行')

      await page.click('[data-testid="clean-op"] label', '查找替换')
      await page.fill('input[data-testid="clean-find"]', '北京')
      await page.fill('input[data-testid="clean-replace"]', '北京店')
      await page.waitFor(`(${previewText}).includes('将修改 1 行') && (${previewText}).includes('北京店')`, { what: '替换预览：1 行，改后是北京店', timeout: 30_000 })
      await page.waitFor(enabled, { what: '执行按钮可点（预览算完）' })
      await page.click('[data-testid="clean-apply"]')
      await page.waitFor(`document.body.innerText.includes('已修改 1 行')`, { what: '替换成功', timeout: 30_000 })
      const values = (await api.call('POST', `/ledger/datasets/${datasetId}/distinct`, TOKEN.alice, { body: { filters: [], column: '店铺' } })).data.values.map(v => v.value).sort()
      assert.deepEqual(values, ['上海店', '北京店'])
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'clean-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
