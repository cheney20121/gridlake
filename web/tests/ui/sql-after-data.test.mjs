// 「数据集更新后」重算（界面）：SQL 工作台保存查询 → 另存为数据集 → 重算选「数据集更新后」并选数据集 → 追加导入后自动重算
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

/** 下拉框：真实鼠标点开（元素自带的 click 打不开），再点选项 */
async function pick(page, selector, text) {
  const at = await page.eval(`(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] })()`)
  await page.mouseClick(at[0], at[1])
  const visible = `[...document.querySelectorAll('.ec-select-dropdown__item')].find(e => e.getBoundingClientRect().width > 0 && e.innerText.trim() === ${JSON.stringify(text)})`
  await page.waitFor(`!!${visible}`, { what: `选项「${text}」` })
  await page.eval(`${visible}.click()`)
}

describe('数据集更新后重算（界面）', { skip }, () => {
  let services
  let browser
  let orders

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    orders = await api.importDataset(TOKEN.alice, '订单', '店铺,金额\nA,10\nB,20\n')
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      await services?.stop()
    }
  })

  it('保存查询时选「数据集更新后」→ 追加导入后汇总自动更新', async () => {
    const page = await browser.newPage()
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel/home?lang=zh-CN`)
      await page.waitFor(`[...document.querySelectorAll('.nav-item')].some(e => e.innerText.includes('数据中心'))`, { what: '首页导航', timeout: 60_000 })
      await page.click('.nav-item', '数据中心')
      await page.click('.tool-card', 'SQL 查询')
      await page.waitFor(`!!document.querySelector('.sql-editor textarea')`, { what: 'SQL 编辑器' })
      await page.eval(`(() => { const t = document.querySelector('.sql-editor textarea'); t.focus(); t.select() })()`)
      await page.type('SELECT count(*) AS 单数 FROM 订单')
      await page.press('Escape')
      await page.click('.sql-dialog button', '运行')
      await page.waitFor(`document.querySelectorAll('.grid-wrap tbody tr').length === 1`, { what: '运行结果' })

      await page.click('.sql-dialog button', '保存查询')
      await page.waitFor(`!!document.querySelector('.ec-dialog input[maxlength="60"]')`, { what: '保存对话框' })
      await page.fill('.ec-dialog input[maxlength="60"]', '订单数')
      await page.click('.ec-dialog .ec-checkbox', '把查询结果保存为数据集')
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog .ec-form-item')].some(e => e.innerText.includes('重算'))`, { what: '另存为数据集的选项' })
      const nameInput = `[...document.querySelectorAll('.ec-dialog .ec-form-item')].find(e => e.innerText.includes('数据集名称'))?.querySelector('input')`
      if (await page.eval(`!!${nameInput}`))
        await page.eval(`(() => { const el = ${nameInput}; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, '订单数'); el.dispatchEvent(new Event('input', { bubbles: true })) })()`)
      await page.waitFor(`!!document.querySelector('[data-testid="sql-schedule-kind"]')`, { what: '重算方式' })
      await pick(page, '[data-testid="sql-schedule-kind"]', '数据集更新后')
      await page.waitFor(`!!document.querySelector('[data-testid="trigger-dataset"]')`, { what: '选择数据集' })
      await pick(page, '[data-testid="trigger-dataset"]', '订单')
      await page.screenshot(path.join(ARTIFACTS, 'sql-after-data.png')).catch(() => {})
      await page.click('.ec-dialog .ec-button--primary', '保存')
      await page.waitFor(`[...document.querySelectorAll('.sql-dialog .saved')].some(e => e.innerText.includes('订单数') && e.innerText.includes('「订单」更新后重算'))`, { what: '侧栏：「订单」更新后重算', timeout: 30_000 })

      const q = (await api.call('GET', '/ledger/sql/queries', TOKEN.alice)).data.find(x => x.name === '订单数')
      assert.deepEqual(q.schedule, { kind: 'after_data', datasetId: orders })
      const count = async () => (await api.call('POST', `/ledger/datasets/${q.datasetId}/aggregate`, TOKEN.alice, { body: { calls: [{ func: 'sum', column: '单数', criteria: [] }] } })).data.values[0]
      assert.equal(await count(), 2)

      // 追加导入两行：汇总自动变成 4
      const form = new FormData()
      form.append('file', new Blob(['店铺,金额\nC,1\nD,2\n'], { type: 'text/csv' }), '订单.csv')
      const p = await api.call('POST', '/ledger/datasets/import/preview', TOKEN.alice, { form })
      await api.call('POST', '/ledger/datasets/import/commit', TOKEN.alice, { body: { token: p.data.token, mode: 'append', datasetId: orders, name: '订单', columns: p.data.columns.map(x => ({ source: x.source, title: x.title, kind: x.kind })), primaryKey: [], fileName: p.data.fileName } })
      const t0 = Date.now()
      while (await count() !== 4 && Date.now() - t0 < 20_000)
        await new Promise(r => setTimeout(r, 300))
      assert.equal(await count(), 4, '追加导入后自动重算')
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'sql-after-data-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
