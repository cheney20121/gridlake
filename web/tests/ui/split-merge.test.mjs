// 拆分 / 合并列（界面）：「更多 → 拆分 / 合并列」→ 按「-」拆收货地址：预览各段数、样例，列数自动取最常见的 3 段 →
// 起名省 / 市 / 区 → 添加计算列；再把省、区合并成一列
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
const CSV = '单号,收货地址,金额\n1,广东-深圳-南山,10\n2,广东-广州-天河,20\n3,北京-北京-朝阳,30\n4,上海,40\n5,,50\n'

/** 点开 Element 下拉（真实鼠标点在中间，位置稳定后再点，没弹出来就重试）并选中文字完全一致的选项 */
async function pick(page, selector, text) {
  const visible = `[...document.querySelectorAll('.ec-select-dropdown__item')].find(e => e.getBoundingClientRect().width > 0 && e.innerText.trim() === ${JSON.stringify(text)})`
  if (await page.eval(`!!${visible}`))
    return page.eval(`${visible}.click()`)
  const rect = `(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return [Math.round(r.left), Math.round(r.top), r.width, r.height] })()`
  let last = ''
  for (let i = 0; i < 20; i++) {
    const now = JSON.stringify(await page.eval(rect))
    if (now === last)
      break
    last = now
    await new Promise(r => setTimeout(r, 100))
  }
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

describe('拆分 / 合并列（界面）', { skip }, () => {
  let services
  let browser
  let datasetId

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    datasetId = await api.importDataset(TOKEN.alice, '收货', CSV)
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      await services?.stop()
    }
  })

  it('拆分收货地址成省 / 市 / 区，再把省、区合并', async () => {
    const page = await browser.newPage()
    const previewText = `(document.querySelector('[data-testid="split-preview"]')?.innerText ?? '')`
    const enabled = `document.querySelector('[data-testid="split-apply"]')?.disabled === false`
    const values = async col => (await api.call('POST', `/ledger/datasets/${datasetId}/distinct`, TOKEN.alice, { body: { filters: [], column: col } })).data.values.map(v => v.value).sort()
    const columns = async () => (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.id === datasetId).columns
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?ds=${datasetId}&lang=zh-CN`)
      await page.waitFor(`document.body.innerText.includes('5 行')`, { timeout: 90_000, what: '打开数据表' })
      const openDialog = async () => {
        await page.click('button', '更多 ▾')
        await page.waitFor(`!!document.querySelector('[data-testid="open-split"]')?.offsetParent`, { what: '更多菜单里的拆分 / 合并列' })
        await page.click('[data-testid="open-split"]')
        await page.waitFor(`!!document.querySelector('[data-testid="split-dialog"]')?.offsetParent`, { what: '对话框' })
      }
      await openDialog()
      await page.waitFor(`${previewText}.includes('5 行：1 段 1 行，3 段 3 行，空 1 行')`, { what: '拆分预览：各段数的行数', timeout: 30_000 })
      assert.equal(await page.eval(`document.querySelector('input[data-testid="split-name-2"]')?.value`), '收货地址3', '列数自动取最常见的 3 段，默认列名')
      for (const [i, n] of ['省', '市', '区'].entries())
        await page.fill(`input[data-testid="split-name-${i}"]`, n)
      await page.waitFor(`${previewText}.includes('深圳') && ${previewText}.includes('南山')`, { what: '样例里拆出的各段' })
      await page.screenshot(path.join(ARTIFACTS, 'split.png')).catch(() => {})
      await page.waitFor(enabled, { what: '可以添加' })
      await page.click('[data-testid="split-apply"]')
      await page.waitFor(`document.body.innerText.includes('已添加计算列：省、市、区')`, { what: '添加成功', timeout: 30_000 })
      const cols = await columns()
      assert.deepEqual(cols.filter(c => c.formula).map(c => [c.title, c.formula]), [
        ['省', '=TRIM(SPLITPART([收货地址],"-",1))'],
        ['市', '=TRIM(SPLITPART([收货地址],"-",2))'],
        ['区', '=TRIM(TEXTAFTER([收货地址],"-",2))'],
      ])
      assert.deepEqual(await values('市'), ['', '北京', '广州', '深圳'])

      await openDialog()
      await page.click('[data-testid="split-mode"] label', '合并几列')
      await pick(page, '[data-testid="merge-columns"]', '省')
      await pick(page, '[data-testid="merge-columns"]', '区')
      await page.waitFor(`document.querySelector('input[data-testid="merge-name"]')?.value === '省区'`, { what: '默认合并列名' })
      await page.waitFor(`${previewText}.includes('广东-南山')`, { what: '合并预览', timeout: 30_000 })
      await page.waitFor(enabled, { what: '可以添加合并列' })
      await page.click('[data-testid="split-apply"]')
      await page.waitFor(`document.body.innerText.includes('已添加计算列：省区')`, { what: '合并成功', timeout: 30_000 })
      assert.deepEqual(await values('省区'), ['', '上海', '北京-朝阳', '广东-南山', '广东-天河'])
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'split-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
