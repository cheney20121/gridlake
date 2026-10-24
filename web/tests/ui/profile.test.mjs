// 数据概况（界面）：数据表「数据概况」→ 每列的空值、取值个数、要点 → 点开一列看分布 → 点一个取值，数据表按它筛选、概况随之重算
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
// 店铺 A 3 行、B 2 行、空 1 行；金额有一个空值；下单时间有一个认不出的写法
const CSV = '店铺,金额,下单时间\nA,10,2026-09-01 10:00:00\nA,20,2026-09-15 11:00:00\nA,,2026-10-02 09:00:00\nB,100,2026-10-05 08:00:00\nB,300,下周一\n,5,2026-10-20 12:00:00\n'

describe('数据概况（界面）', { skip }, () => {
  let services
  let browser
  let datasetId

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    datasetId = await api.importDataset(TOKEN.alice, '订单', CSV)
    // 「下周一」让导入把这列当成文本；改成日期时间（认不出的保留原文），概况应该点出来
    const r = await api.call('PUT', `/ledger/datasets/${datasetId}/columns/kind`, TOKEN.alice, { body: { column: '下单时间', kind: 'datetime' } })
    assert.equal(r.status, 200, JSON.stringify(r.body))
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  it('概况 → 展开一列 → 点取值筛选', async () => {
    const page = await browser.newPage()
    const card = title => `document.querySelector('[data-testid="profile-col-${title}"]')?.innerText ?? ''`
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?ds=${datasetId}&lang=zh-CN`)
      await page.waitFor(`document.body.innerText.includes('6 行') && !!document.querySelector('[data-testid="open-profile"]')`, { timeout: 90_000, what: '打开数据表' })
      await page.click('[data-testid="open-profile"]')
      await page.waitFor(`document.querySelector('[data-testid="profile-drawer"]')?.innerText.includes('按当前筛选 6 行')`, { what: '概况：6 行', timeout: 60_000 })
      assert.match(await page.eval(card('店铺')), /2 种/, '店铺：两种取值（空值不算）')
      assert.match(await page.eval(card('店铺')), /空值 17%/, '店铺：1/6 空值')
      assert.match(await page.eval(card('金额')), /5\.00 ~ 300\.00/, '金额的范围（金额两位小数）')
      assert.match(await page.eval(card('下单时间')), /1 个认不出的日期/, '下单时间：认不出的写法')
      assert.match(await page.eval(card('下单时间')), /2026-09-01 10:00:00 ~ 2026-10-20 12:00:00/, '日期范围只看认得出的写法')

      await page.click('[data-testid="profile-col-金额"] .head')
      await page.waitFor(`!!document.querySelector('[data-testid="profile-col-金额"] [data-testid="profile-histogram"]')`, { what: '金额的直方图' })
      await page.click('[data-testid="profile-col-店铺"] .head')
      await page.waitFor(`!!document.querySelector('[data-testid="profile-value-A"]')`, { what: '店铺的取值列表' })
      assert.match(await page.eval(`document.querySelector('[data-testid="profile-value-A"]').innerText`), /3（50%）/)
      await page.screenshot(path.join(ARTIFACTS, 'profile.png')).catch(() => {})

      await page.click('[data-testid="profile-value-A"]')
      await page.waitFor(`document.body.innerText.includes('3 行') && document.querySelector('[data-testid="profile-drawer"]')?.innerText.includes('按当前筛选 3 行')`, { what: '数据表按店铺 A 筛选，概况重算成 3 行', timeout: 60_000 })
      assert.match(await page.eval(card('金额')), /空值 33%/, '筛选后：A 店 3 行里 1 个空值')
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'profile-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
