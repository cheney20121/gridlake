// 单元格里的 SQL：=SQL() 结果带表头铺到右下方；=SQLVALUE() 取一个值，参数引用单元格；查询改了 / 数据变了重新计算；出错显示说明
import assert from 'node:assert/strict'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { api, startServices, TOKEN, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
const COOKIE = 'gridlake-token'
// 故意写错的两个公式（表不存在、不是只读查询）：服务端返回 400，浏览器会记一条网络错误
const ALLOWED = [/command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/, /status of 400 \(Bad Request\) .*\/ledger\/sql\/run/]
const CSV = '订单号,店铺,金额\n1,A,10\n2,A,20\n3,B,5\n4,C,100\n'

describe('SQL / SQLVALUE 公式（界面）', { skip }, () => {
  let services
  let browser
  let workbookId

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    await api.importDataset(TOKEN.alice, '订单', CSV)
    workbookId = (await api.createWorkbook(TOKEN.alice, 'SQL 公式', [['店铺'], ['A']])).id
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  it('结果铺开、参数引用单元格、出错有说明', async () => {
    const page = await browser.newPage()
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?wb=${workbookId}&lang=zh-CN`)
      await page.waitFor(`!!document.querySelector('.excel-topbar') && !!window.__univerAPI?.getActiveWorkbook()`, { timeout: 90_000, what: '编辑器加载完成' })
      await page.eval(`(() => {
        const sheet = window.__univerAPI.getActiveWorkbook().getActiveSheet()
        sheet.getRange('D1').setFormula('=SQL("SELECT 店铺, sum(金额) AS 合计 FROM 订单 GROUP BY 1 ORDER BY 1")')
        sheet.getRange('B2').setFormula('=SQLVALUE("SELECT sum(金额) FROM 订单 WHERE 店铺 = {{店铺}}", "店铺", A2)')
        sheet.getRange('B3').setFormula('=SQLVALUE("SELECT count(*) FROM 订单 WHERE 金额 > {{下限}}", "下限", 15)')
        sheet.getRange('B4').setFormula('=SQL("SELECT * FROM 不存在的表")')
        sheet.getRange('B5').setFormula('=SQLVALUE("DELETE FROM 订单")')
      })()`)
      const grid = range => page.eval(`JSON.stringify(window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(${JSON.stringify(range)}).getValues())`).then(JSON.parse)
      const read = cell => page.eval(`String(window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(${JSON.stringify(cell)}).getDisplayValue())`)
      await page.waitFor(`String(window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('E4').getValue()) === '100'`, { timeout: 60_000, what: 'SQL 结果铺到 D1:E4' })
      assert.deepEqual(await grid('D1:E4'), [['店铺', '合计'], ['A', 30], ['B', 5], ['C', 100]], '表头 + 三行')
      await page.waitFor(`String(window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('B2').getValue()) === '30'`, { timeout: 30_000, what: 'SQLVALUE 引用 A2 的店铺' })
      assert.equal(await read('B3'), '2', '数字参数')
      assert.ok((await read('B4')).startsWith('#'), `表不存在显示错误：${await read('B4')}`)
      assert.ok((await read('B5')).startsWith('#'), `只读检查：${await read('B5')}`)

      // 参数单元格改了：重新查询
      await page.eval(`void window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('A2').setValue('C')`)
      await page.waitFor(`String(window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('B2').getValue()) === '100'`, { timeout: 30_000, what: '改了参数后重算' })
      await page.screenshot(path.join(ARTIFACTS, 'formula-sql.png')).catch(() => {})
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'formula-sql-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
