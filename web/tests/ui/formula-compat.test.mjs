// 函数兼容：导入的 xlsx 用到不支持 / 拼错的函数时，导入结果点名是哪几个（不误报支持的函数）
import assert from 'node:assert/strict'
import { Buffer } from 'node:buffer'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { api, startServices, TOKEN, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
const COOKIE = 'gridlake-token'
const ALLOWED = [/command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]

describe('函数兼容（界面）', { skip }, () => {
  let services
  let browser
  let workbookId

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    workbookId = (await api.createWorkbook(TOKEN.alice, '兼容', [['x']])).id
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      await services?.stop()
    }
  })

  it('导入 xlsx：点名不支持的函数和拼错的函数', async () => {
    const ExcelJS = (await import('exceljs')).default
    const wbx = new ExcelJS.Workbook()
    const ws = wbx.addWorksheet('公式')
    ws.getCell('A1').value = { formula: 'CUBEVALUE("Sales","[Measures].[Amount]")' }
    ws.getCell('A2').value = { formula: 'CUBEVALUE("Sales","[Measures].[Count]")' }
    ws.getCell('A3').value = { formula: 'SUMM(1,2)' }
    ws.getCell('A4').value = { formula: 'SUM(1,2)' }
    ws.getCell('A5').value = { formula: '_xlfn.XLOOKUP(1,{1,2},{10,20})' }
    const base64 = Buffer.from(await wbx.xlsx.writeBuffer()).toString('base64')
    const page = await browser.newPage()
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?wb=${workbookId}&lang=zh-CN`)
      await page.waitFor(`!!document.querySelector('.excel-topbar') && !!window.__univerAPI?.getActiveWorkbook()`, { timeout: 90_000, what: '编辑器加载完成' })
      await page.eval(`(() => { const bin = atob(${JSON.stringify(base64)}); const bytes = Uint8Array.from(bin, c => c.charCodeAt(0)); const dt = new DataTransfer(); dt.items.add(new File([bytes], '兼容.xlsx')); const input = document.querySelector('input[type=file][accept*=xlsx]'); input.files = dt.files; input.dispatchEvent(new Event('change', { bubbles: true })) })()`)
      await page.waitFor(`[...document.querySelectorAll('.ec-message-box')].some(d => d.offsetParent && (d.innerText.includes('不支持的函数') || d.innerText.includes('覆盖') || d.innerText.includes('替换')))`, { what: '导入结果 / 确认框', timeout: 60_000 })
      const first = await page.eval(`[...document.querySelectorAll('.ec-message-box')].find(d => d.offsetParent).innerText`)
      if (!first.includes('不支持的函数')) {
        await page.click('.ec-message-box button', '确定').catch(async () => page.click('.ec-message-box button', '继续'))
        await page.waitFor(`[...document.querySelectorAll('.ec-message-box')].some(d => d.offsetParent && d.innerText.includes('不支持的函数'))`, { what: '导入结果', timeout: 60_000 })
      }
      const box = await page.eval(`[...document.querySelectorAll('.ec-message-box')].find(d => d.offsetParent && d.innerText.includes('不支持的函数')).innerText`)
      assert.match(box, /3 个公式使用了当前不支持的函数（显示 #NAME\?）：CUBEVALUE、SUMM/, box)
      assert.ok(!/XLOOKUP|SUM[^M]/.test(box.split('不支持的函数')[1] ?? ''), `不误报支持的函数：${box}`)
      await page.screenshot(path.join(ARTIFACTS, 'formula-compat.png')).catch(() => {})
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'formula-compat-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
