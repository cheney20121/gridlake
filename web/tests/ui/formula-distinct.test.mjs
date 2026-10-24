// 数据集公式 DCOUNTDISTINCTIFS：在单元格里写公式 → 服务端去重计数 → 显示数字；条件引用单元格；带通配符时多个分组合并去重
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
// 客户 c1 同时在 A、B 两家店：A 店 2 个、B 店 2 个，两家合并是 3 个
const CSV = '订单号,店铺,客户,金额\n1,A,c1,10\n2,A,c1,20\n3,A,c2,5\n4,B,c1,100\n5,B,c3,200\n6,C,,1\n7,B,c3,7\n'

describe('DCOUNTDISTINCTIFS（界面）', { skip }, () => {
  let services
  let browser
  let workbookId

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    await api.importDataset(TOKEN.alice, '订单', CSV)
    workbookId = (await api.createWorkbook(TOKEN.alice, '去重公式', [['店铺'], ['A'], ['B']])).id
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  it('单元格里的公式由服务端去重计数；条件可以引用单元格，通配符合并分组后再去重', async () => {
    const page = await browser.newPage()
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?wb=${workbookId}`)
      await page.waitFor(`!!document.querySelector('.excel-topbar') && !!window.__univerAPI?.getActiveWorkbook()`, { timeout: 90_000, what: '编辑器加载完成' })
      await page.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { what: '已保存' })
      await page.eval(`(() => {
        const sheet = window.__univerAPI.getActiveWorkbook().getActiveSheet()
        sheet.getRange('B2').setFormula('=DCOUNTDISTINCTIFS("订单","客户","店铺",A2)')
        sheet.getRange('B3').setFormula('=DCOUNTDISTINCTIFS("订单","客户","店铺",A3)')
        sheet.getRange('B4').setFormula('=DCOUNTDISTINCTIFS("订单","客户","店铺","*")')
        sheet.getRange('B5').setFormula('=DCOUNTDISTINCTIFS("订单","客户")')
        sheet.getRange('B6').setFormula('=DCOUNTDISTINCTIFS("订单","订单号","店铺","B")')
        sheet.getRange('B7').setFormula('=DCOUNTDISTINCTIFS("订单","没有这列")')
        return true
      })()`)
      const read = cell => page.eval(`String(window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(${JSON.stringify(cell)}).getDisplayValue())`)
      await page.waitFor(`['B2','B3','B4','B5','B6'].every(c => /^\\d+$/.test(String(window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(c).getDisplayValue())))`, { timeout: 60_000, what: '公式算出数字' })
      assert.equal(await read('B2'), '2', 'A 店 2 个不同客户（条件引用单元格）')
      assert.equal(await read('B3'), '2', 'B 店 2 个不同客户')
      assert.equal(await read('B4'), '3', '通配符合并后去重是 3，不是 2+2')
      assert.equal(await read('B5'), '3', '没有条件：全表 3 个（空值不算）')
      assert.equal(await read('B6'), '3', '订单号去重：B 店 3 个')
      await page.waitFor(`String(window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('B7').getDisplayValue()).startsWith('#')`, { what: '列不存在显示错误' })
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [])
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'formula-distinct.png')).catch(() => {})
      throw e
    }
    finally {
      await page.close().catch(() => {})
    }
  })
})
