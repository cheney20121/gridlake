// 带访问密码的分享链接：创建时设密码 → 访客打开先看到密码框（不透露工作簿名称）→ 输错有提示 → 输对看到工作簿 → 刷新不用重输 → 吊销后失效
import assert from 'node:assert/strict'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { api, startServices, TOKEN, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
const COOKIE = 'webexcel-token'
const NAME = '密码分享测试看板'
const PASSWORD = 'open-sesame'
// 输错密码时服务端返回 400；吊销后的链接是 403；公式刷新的启动期提示与本功能无关
const ALLOWED = [/status of 400 .*\/share\/unlock/, /status of 403/, /command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]

describe('带访问密码的分享链接（界面）', { skip }, () => {
  let services
  let browser
  let workbookId
  let link

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    workbookId = (await api.createWorkbook(TOKEN.alice, NAME, [['项目', '金额'], ['房租', 5], ['工资', 6]])).id
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  async function withPage(name, fn, { user, allowErrors = [] } = {}) {
    const page = await browser.newPage()
    try {
      await page.clearCookies()
      if (user)
        await page.setCookie(COOKIE, TOKEN[user], WEB)
      await fn(page)
      const errors = page.errors.filter(e => ![...ALLOWED, ...allowErrors].some(re => re.test(e)))
      assert.deepEqual(errors, [], `页面出现控制台错误：\n${errors.join('\n')}`)
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, `${name}.png`)).catch(() => {})
      throw e
    }
    finally {
      await page.close().catch(() => {})
    }
  }

  it('创建链接对话框：填访问密码，创建后列表里标出「有密码」', async () => {
    await withPage('share-password-dialog', async (page) => {
      await page.goto(`${WEB}/excel?wb=${workbookId}`)
      await page.waitFor(`!!document.querySelector('.excel-topbar') && !!window.__univerAPI?.getActiveWorkbook()`, { timeout: 90_000, what: '编辑器加载完成' })
      await page.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { what: '已保存' })
      await page.press('q', { alt: true, code: 'KeyQ' })
      await page.waitFor(`!!document.querySelector('input[placeholder*="搜索功能"]')`, { what: '命令面板' })
      await page.fill('input[placeholder*="搜索功能"]', '分享链接')
      await page.waitFor(`document.querySelector('[data-active="true"]')?.innerText.includes('分享链接')`, { what: '搜索到分享链接' })
      // 工作表加载完成时会把焦点抢回画布：回车前确认焦点在搜索框里，否则回车会落到表格上（选中格下移）而不是执行命令
      await page.eval(`document.querySelector('input[placeholder*="搜索功能"]')?.focus()`)
      await page.press('Enter')
      await page.waitFor(`!!document.querySelector('[data-testid="share-new-password"]')`, { what: '分享对话框' })

      // 太短：前端先拦下，不创建
      await page.fill('input[data-testid="share-new-password"]', '123')
      await page.click('.ec-dialog button', '创建链接')
      await page.waitFor(`document.body.innerText.includes('访问密码需要 6 到 128 位')`, { what: '密码长度提示' })
      assert.equal((await api.call('GET', `/ledger/workbooks/${workbookId}/shares`, TOKEN.alice)).data.length, 0, '没有创建出链接')

      await page.fill('input[data-testid="share-new-password"]', PASSWORD)
      await page.click('.ec-dialog button', '创建链接')
      await page.waitFor(`document.querySelector('.ec-dialog')?.innerText.includes('链接已创建') && document.querySelector('.ec-dialog')?.innerText.includes('访问密码要另外告诉对方')`, { what: '链接已创建，并提醒密码另外告诉对方' })
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog .ec-tag')].some(e => e.innerText.includes('有密码'))`, { what: '列表里标出有密码' })
      const shares = (await api.call('GET', `/ledger/workbooks/${workbookId}/shares`, TOKEN.alice)).data
      assert.equal(shares.length, 1)
      assert.equal(shares[0].protected, true)
      // 完整链接只在创建时显示：从输入框读出来，给后面的访客测试用
      link = await page.eval(`document.querySelector('.created input')?.value ?? ''`)
      assert.match(link, /\/share\/gls_/)
    }, { user: 'alice' })
  })

  it('访客：先看到密码框且不透露工作簿；输错有提示；输对打开；刷新不用重输', async () => {
    assert.ok(link, '上一个用例应已创建链接')
    const url = `${WEB}/share/${link.split('/share/')[1]}`
    await withPage('share-password-guest', async (page) => {
      await page.goto(url)
      await page.waitFor(`!!document.querySelector('[data-testid="share-password"]')`, { what: '密码框', timeout: 60_000 })
      assert.equal(await page.eval(`document.body.innerText.includes(${JSON.stringify(NAME)})`), false, '输入密码前不显示工作簿名称')
      assert.equal(await page.eval(`document.title.includes(${JSON.stringify(NAME)})`), false)
      assert.equal(await page.eval(`document.querySelector('[data-testid="share-unlock"]').disabled`), true, '没输密码时按钮不可点')

      await page.fill('input[data-testid="share-password"]', 'wrong-password')
      await page.click('[data-testid="share-unlock"]')
      await page.waitFor(`document.querySelector('[data-testid="share-password-error"]')?.innerText.includes('访问密码不正确')`, { what: '输错的提示' })
      assert.equal(await page.eval(`document.body.innerText.includes(${JSON.stringify(NAME)})`), false)

      await page.fill('input[data-testid="share-password"]', PASSWORD)
      await page.click('[data-testid="share-unlock"]')
      await page.waitFor(`document.querySelector('.share-bar')?.innerText.includes(${JSON.stringify(NAME)})`, { what: '输对后显示工作簿', timeout: 60_000 })
      await page.waitFor(`!document.querySelector('[data-testid="share-password"]')`, { what: '密码框消失' })
      await page.waitFor(`document.querySelector('.share-bar')?.innerText.includes('只读')`, { what: '只读标记' })

      // 刷新：通行证在本标签页的 sessionStorage 里，不用重输
      await page.goto(url)
      await page.waitFor(`document.querySelector('.share-bar')?.innerText.includes(${JSON.stringify(NAME)})`, { what: '刷新后仍可看', timeout: 60_000 })
      assert.equal(await page.eval(`!!document.querySelector('[data-testid="share-password"]')`), false)
    })
  })

  it('换一个标签页（没有通行证）要重新输入密码；吊销后提示链接失效', async () => {
    const token = link.split('/share/')[1]
    await withPage('share-password-newtab', async (page) => {
      await page.goto(`${WEB}/share/${token}`)
      await page.waitFor(`!!document.querySelector('[data-testid="share-password"]')`, { what: '新标签页要重新输入密码', timeout: 60_000 })
      const [{ id }] = (await api.call('GET', `/ledger/workbooks/${workbookId}/shares`, TOKEN.alice)).data
      assert.equal((await api.call('DELETE', `/ledger/shares/${id}`, TOKEN.alice)).status, 200)
      await page.fill('input[data-testid="share-password"]', PASSWORD)
      await page.click('[data-testid="share-unlock"]')
      await page.waitFor(`document.body.innerText.includes('这个分享链接已失效或不存在')`, { what: '吊销后输对密码也提示失效' })
    })
  })
})
