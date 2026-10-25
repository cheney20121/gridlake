// 分类汇总（界面）：列头菜单「按此列分类汇总」→ 数据按店铺排好、每组后面一行小计（画布上出现小计行的底色）→
// 全部折叠 / 点小计行的行号展开一组 / 全部展开 → 全选时只数数据行 → 标签上显示几组 → 取消
import assert from 'node:assert/strict'
import { Buffer } from 'node:buffer'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { API, api, startServices, TOKEN, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
const COOKIE = 'gridlake-token'
const ALLOWED = [/command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]
const CSV = '单号,店铺,金额\n1,北京店,10\n2,上海店,20\n3,北京店,30\n4,,40\n5,上海店,50\n6,广州店,60\n7,北京店,70\n'

describe('分类汇总（界面）', { skip }, () => {
  let services
  let browser
  let datasetId

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    datasetId = await api.importDataset(TOKEN.alice, '分组订单', CSV)
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  it('按店铺分类汇总：每组一行小计，全选只数数据行，可以取消', async () => {
    const page = await browser.newPage()
    // 画布上与小计行底色 #EEF6F1 完全相同的像素数
    const tinted = `(() => { let n = 0; for (const c of document.querySelectorAll('.scroller')) { const cv = c.parentElement.querySelector('canvas'); const d = cv.getContext('2d').getImageData(0, 0, cv.width, cv.height).data; for (let i = 0; i < d.length; i += 4) if (d[i] === 238 && d[i + 1] === 246 && d[i + 2] === 241) n++ } return n })()`
    const tag = `(document.querySelector('[data-testid="group-tag"]')?.innerText ?? '')`
    try {
      const cols = (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.id === datasetId).columns
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?ds=${datasetId}&lang=zh-CN`)
      await page.waitFor(`document.body.innerText.includes('7 行') && !!document.querySelector('.scroller')`, { timeout: 90_000, what: '打开数据表' })
      assert.equal(await page.eval(tinted), 0, '还没有小计行')
      // 点「店铺」列表头的按钮（画布里没有 DOM，按几何位置算；行号列 48px）
      const canvas = await page.eval(`(() => { const r = document.querySelector('.scroller').parentElement.querySelector('canvas').getBoundingClientRect(); return [r.left, r.top] })()`)
      await page.mouseClick(canvas[0] + 48 + cols[0].width + cols[1].width - 9, canvas[1] + 34)
      await page.waitFor(`!!document.querySelector('[data-testid="group-by-column"]')`, { what: '列头菜单里的「按此列分类汇总」' })
      await page.click('[data-testid="group-by-column"]')
      await page.waitFor(`${tag}.includes('按「店铺」分类汇总（4 组）')`, { what: '分类汇总标签：4 组', timeout: 30_000 })
      await page.waitFor(`${tinted} > 1000`, { what: '画布上出现小计行', timeout: 30_000 })
      await page.screenshot(path.join(ARTIFACTS, 'subtotals.png')).catch(() => {})

      // 第 i 个显示行是不是小计行（看行首一个像素的底色）
      const isSub = i => `(() => { const cv = document.querySelector('.scroller').parentElement.querySelector('canvas'); const k = window.devicePixelRatio || 1; const d = cv.getContext('2d').getImageData(Math.round((48 + 8) * k), Math.round((48 + ${i} * 24 + 4) * k), 1, 1).data; return d[0] === 238 && d[1] === 246 && d[2] === 241 })()`
      const pattern = async n => (await Promise.all(Array.from({ length: n }, (_, i) => page.eval(isSub(i))))).map(x => x ? 'S' : 'd').join('')
      assert.equal(await pattern(5), 'ddSdd', '上海店 2 行 + 小计，北京店…')
      await page.click('[data-testid="collapse-all"]')
      await page.waitFor(`${isSub(0)} && ${isSub(3)} && !${isSub(4)}`, { what: '全部折叠：只剩 4 行小计', timeout: 30_000 })
      await page.screenshot(path.join(ARTIFACTS, 'subtotals-collapsed.png')).catch(() => {})
      // 点「北京店」小计（第 2 行）的行号：展开这一组
      await page.mouseClick(canvas[0] + 20, canvas[1] + 48 + 24 * 1 + 12)
      await page.waitFor(`${isSub(0)} && !${isSub(1)} && !${isSub(3)} && ${isSub(4)}`, { what: '展开北京店', timeout: 30_000 })
      assert.equal(await pattern(7), 'SdddSSS')
      await page.click('[data-testid="expand-all"]')
      await page.waitFor(`!${isSub(0)} && ${isSub(2)}`, { what: '全部展开', timeout: 30_000 })

      // 导出 Excel：链接里带上分类汇总和折叠的组（北京店折叠），下载得到的是 xlsx
      await page.click('[data-testid="collapse-all"]')
      await page.mouseClick(canvas[0] + 20, canvas[1] + 48 + 24 * 0 + 12)
      await page.waitFor(`!${isSub(0)} && ${isSub(2)} && ${isSub(3)}`, { what: '只展开上海店', timeout: 30_000 })
      await page.eval(`(() => { const orig = HTMLAnchorElement.prototype.click; HTMLAnchorElement.prototype.click = function () { if (this.href.includes('/export?')) { window.__lastExport = this.href; return } return orig.call(this) } })(); 0`)
      await page.click('button', '导出 ▾')
      await page.waitFor(`[...document.querySelectorAll('.ec-dropdown-menu__item')].some(e => e.offsetParent && e.innerText.includes('含分类汇总'))`, { what: '导出菜单写明含分类汇总' })
      await page.click('.ec-dropdown-menu__item', '含分类汇总')
      await page.waitFor(`!!window.__lastExport`, { what: '导出链接' })
      const group = JSON.parse(new URL(await page.eval(`window.__lastExport`)).searchParams.get('group'))
      assert.deepEqual([group.column, group.collapsed, group.allCollapsed], ['店铺', [1, 2, 3], false], JSON.stringify(group))
      // 用同一个链接下载（测试里页面的登录不走 cookie，这里带上令牌直接请求接口）
      const href = new URL(await page.eval(`window.__lastExport`))
      const res = await fetch(API + href.pathname.slice(href.pathname.indexOf('/ledger/datasets')) + href.search, { headers: { 'x-user-token': TOKEN.alice } })
      const bytes = Buffer.from(await res.arrayBuffer())
      assert.ok(res.ok && res.headers.get('content-type').includes('spreadsheetml') && bytes.subarray(0, 2).toString() === 'PK', `下载到 xlsx（${res.status}，${bytes.length} 字节）`)
      await page.click('[data-testid="expand-all"]')
      await page.waitFor(`!${isSub(0)} && ${isSub(2)} && !${isSub(3)}`, { what: '再全部展开', timeout: 30_000 })

      // 全选：7 个数据行（不含 4 行小计）
      await page.mouseClick(canvas[0] + 48 + 20, canvas[1] + 48 + 24 * 1 + 12)
      await page.press('a', { ctrl: true })
      await page.waitFor(`document.body.innerText.includes('已选 7 行 × 3 列')`, { what: '全选只数数据行', timeout: 30_000 })

      // 取消
      await page.eval(`document.querySelector('[data-testid="group-tag"] .ec-tag__close').click(); 0`)
      await page.waitFor(`!document.querySelector('[data-testid="group-tag"]') && ${tinted} === 0`, { what: '取消分类汇总', timeout: 30_000 })
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'subtotals-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
