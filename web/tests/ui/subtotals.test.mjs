// 分类汇总（界面）：列头菜单「按此列分类汇总」→ 数据按店铺排好、每组后面一行小计（画布上出现小计行的底色）→
// 全部折叠 / 点小计行的行号展开一组 / 全部展开 → 全选时只数数据行 → 标签上显示几组 → 取消；
// 再按渠道「添加为第 2 级」：内层、外层两种小计，折叠到某一级，取消一级
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
const CSV2 = '单号,店铺,渠道,金额\n1,北京店,直播,10\n2,上海店,直播,20\n3,北京店,短视频,30\n4,,直播,40\n5,上海店,短视频,50\n6,广州店,直播,60\n7,北京店,直播,70\n'

describe('分类汇总（界面）', { skip }, () => {
  let services
  let browser
  let datasetId
  let twoLevelId

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    datasetId = await api.importDataset(TOKEN.alice, '分组订单', CSV)
    twoLevelId = await api.importDataset(TOKEN.alice, '两级订单', CSV2)
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      await services?.stop()
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
      assert.deepEqual([group.levels, group.collapsed, group.collapseLevel], [[{ column: '店铺', desc: false }], [[1, 2, 3]], null], JSON.stringify(group))
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

  it('两级：先按店铺、再按渠道，内层小计和外层小计，折叠到某一级，取消一级', async () => {
    const page = await browser.newPage()
    const tag = `(document.querySelector('[data-testid="group-tag"]')?.innerText ?? '')`
    try {
      const cols = (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.id === twoLevelId).columns
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?ds=${twoLevelId}&lang=zh-CN`)
      await page.waitFor(`document.body.innerText.includes('7 行') && !!document.querySelector('.scroller')`, { timeout: 90_000, what: '打开数据表' })
      // 画布的位置（工具栏可能换行，每次点之前重新量）
      const canvasAt = () => page.eval(`(() => { const r = document.querySelector('.scroller').parentElement.querySelector('canvas').getBoundingClientRect(); return [r.left, r.top] })()`)
      // 第 n 列表头上的菜单按钮
      const header = async (n) => {
        const [x, y] = await canvasAt()
        await page.mouseClick(x + 48 + cols.slice(0, n + 1).reduce((w, c) => w + c.width, 0) - 9, y + 34)
      }
      await header(1)
      await page.waitFor(`!!document.querySelector('[data-testid="group-by-column"]')`, { what: '店铺列的菜单' })
      await page.click('[data-testid="group-by-column"]')
      await page.waitFor(`${tag}.includes('按「店铺」分类汇总（4 组）')`, { what: '一级', timeout: 30_000 })
      await header(2)
      await page.waitFor(`!!document.querySelector('[data-testid="group-add-level"]')`, { what: '渠道列的菜单里「添加为第 2 级分类汇总」' })
      await page.click('[data-testid="group-add-level"]')
      await page.waitFor(`${tag}.includes('按「店铺 → 渠道」分类汇总（4 / 6 组）')`, { what: '两级', timeout: 30_000 })

      // 第 i 个显示行：d 数据、s 内层小计（#EEF6F1）、S 外层小计（#DCEDE2）
      const kind = i => `(() => { const cv = document.querySelector('.scroller').parentElement.querySelector('canvas'); const k = window.devicePixelRatio || 1; const d = cv.getContext('2d').getImageData(Math.round((48 + 8) * k), Math.round((48 + ${i} * 24 + 4) * k), 1, 1).data; return d[0] === 238 && d[1] === 246 && d[2] === 241 ? 's' : d[0] === 220 && d[1] === 237 && d[2] === 226 ? 'S' : 'd' })()`
      const pattern = async n => (await Promise.all(Array.from({ length: n }, (_, i) => page.eval(kind(i))))).join('')
      // 上海店：直播 1 行 + 小计、短视频 1 行 + 小计、店铺小计；北京店：直播 2 行…
      await page.waitFor(`${kind(0)} === 'd' && ${kind(1)} === 's' && ${kind(4)} === 'S'`, { what: '内层、外层小计', timeout: 30_000 })
      assert.equal(await pattern(10), 'dsdsSddsds')
      await page.screenshot(path.join(ARTIFACTS, 'subtotals-two-levels.png')).catch(() => {})

      await page.click('[data-testid="collapse-level-0"]')
      await page.waitFor(`${kind(0)} === 'S' && ${kind(3)} === 'S'`, { what: '折叠到店铺：只剩 4 行店铺小计', timeout: 30_000 })
      assert.equal(await pattern(4), 'SSSS')
      // 点上海店小计的行号：展开它，里面的渠道小计又出来了
      const [x0, y0] = await canvasAt()
      await page.mouseClick(x0 + 20, y0 + 48 + 12)
      await page.waitFor(`${kind(0)} === 'd' && ${kind(4)} === 'S' && ${kind(5)} === 'S'`, { what: '展开上海店', timeout: 30_000 })
      assert.equal(await pattern(7), 'dsdsSSS')
      await page.click('[data-testid="collapse-level-1"]')
      await page.waitFor(`${kind(0)} === 's' && ${kind(2)} === 'S'`, { what: '折叠到渠道：只剩渠道小计和店铺小计', timeout: 30_000 })
      assert.equal(await pattern(10), 'ssSssSsSsS')

      // 导出：两级、内层全部折叠
      await page.eval(`(() => { const orig = HTMLAnchorElement.prototype.click; HTMLAnchorElement.prototype.click = function () { if (this.href.includes('/export?')) { window.__lastExport = this.href; return } return orig.call(this) } })(); 0`)
      await page.click('button', '导出 ▾')
      await page.waitFor(`[...document.querySelectorAll('.ec-dropdown-menu__item')].some(e => e.offsetParent && e.innerText.includes('含分类汇总'))`, { what: '导出菜单' })
      await page.click('.ec-dropdown-menu__item', '含分类汇总')
      await page.waitFor(`!!window.__lastExport`, { what: '导出链接' })
      const group = JSON.parse(new URL(await page.eval(`window.__lastExport`)).searchParams.get('group'))
      assert.deepEqual([group.levels.map(l => l.column), group.collapsed, group.collapseLevel], [['店铺', '渠道'], [[], [0, 1, 2, 3, 4, 5]], 1], JSON.stringify(group))

      // 取消店铺这一级：只剩按渠道
      await header(1)
      await page.waitFor(`!!document.querySelector('[data-testid="ungroup"]')`, { what: '店铺列的「取消这一级」' })
      await page.click('[data-testid="ungroup"]')
      await page.waitFor(`${tag}.includes('按「渠道」分类汇总（2 组）')`, { what: '只剩一级', timeout: 30_000 })
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'subtotals-two-levels-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
