// 内置账号（LEDGER_AUTH_MODE=local）的界面流程：未登录跳转登录页 → 登录 → 回到原页面 → 管理员建账号 → 新账号登录 → 修改密码 → 退出
import assert from 'node:assert/strict'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { LOCAL_ADMIN, startServices, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
// 未登录时 /ledger/me 之外的请求会 401（页面跳转登录前可能已发出）；故意输错密码时登录接口返回 400
const ALLOWED = [/401|Unauthorized/i, /status of 400 .*\/ledger\/auth\/login/, /command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]

describe('内置账号登录（界面）', { skip }, () => {
  let services
  let browser
  let page

  before(async () => {
    services = await startServices({ auth: 'local' })
    browser = await launchChrome()
    page = await browser.newPage()
    await page.clearCookies()
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  const shot = name => page.screenshot(path.join(ARTIFACTS, `${name}.png`)).catch(() => {})
  const noErrors = () => assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [])

  async function login(username, password) {
    await page.waitFor(`!!document.querySelector('input[data-testid="login-username"]')`, { what: '登录表单' })
    await page.fill('input[data-testid="login-username"]', username)
    await page.fill('input[data-testid="login-password"]', password)
    await page.click('[data-testid="login-submit"]')
  }

  async function logout() {
    await page.click('[data-testid="user-menu"] .cursor-pointer')
    await page.waitFor(`[...document.querySelectorAll('.ec-dropdown-menu__item')].some(e => e.offsetParent && e.innerText.includes('退出登录'))`, { what: '用户菜单' })
    await page.click('.ec-dropdown-menu__item', '退出登录')
    await page.waitFor(`location.pathname === '/login'`, { what: '退出后回到登录页' })
  }

  it('未登录打开首页 → 跳转登录页；密码错误有提示；登录后回到原页面', async () => {
    try {
      await page.goto(`${WEB}/excel/home`)
      await page.waitFor(`location.pathname === '/login' && location.search.includes('redirect=')`, { what: '跳转到登录页' })
      await login(LOCAL_ADMIN.username, 'wrong-password')
      await page.waitFor(`document.querySelector('[data-testid="login-error"]')?.innerText.includes('用户名或密码不正确')`, { what: '密码错误的提示' })
      await login(LOCAL_ADMIN.username, LOCAL_ADMIN.password)
      await page.waitFor(`location.pathname === '/excel/home'`, { what: '登录后回到首页', timeout: 60_000 })
      await page.waitFor(`document.querySelector('.home-header')?.innerText.includes('管理员')`, { what: '首页显示登录的管理员', timeout: 60_000 })
      await page.click('button.nav-item', '自动化')
      await page.waitFor(`!!document.querySelector('[data-testid="home-accounts"]')`, { what: '自动化里有「账号管理」入口' })
      noErrors()
    }
    catch (e) {
      await shot('login-local-1')
      throw e
    }
  })

  it('管理员建账号 → 退出 → 新账号登录（没有账号管理入口）→ 修改密码 → 退出', async () => {
    try {
      await page.click('[data-testid="home-accounts"]')
      await page.waitFor(`!!document.querySelector('[data-testid="account-new"]')`, { what: '账号管理对话框' })
      await page.fill('[data-testid="account-new"] input[placeholder^="用户名"]', 'alice')
      await page.fill('[data-testid="account-new"] input[placeholder^="显示名"]', 'Alice')
      await page.fill('[data-testid="account-new"] input[placeholder^="初始密码"]', 'alice-pass-1')
      await page.click('[data-testid="account-new"] button', '新建账号')
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog .ec-table__row')].some(r => r.innerText.includes('alice') && r.innerText.includes('Alice'))`, { what: '列表里出现新账号' })
      await page.press('Escape')
      await logout()

      await login('alice', 'alice-pass-1')
      await page.waitFor(`location.pathname === '/excel/home' && document.body.innerText.includes('Alice')`, { what: 'Alice 登录进首页', timeout: 60_000 })
      await page.click('button.nav-item', '自动化')
      await page.waitFor(`document.body.innerText.includes('外部 AI 接入') || document.body.innerText.includes('定时推送')`, { what: '自动化分区' })
      assert.equal(await page.eval(`!!document.querySelector('[data-testid="home-accounts"]')`), false, '普通账号没有账号管理入口')

      await page.click('[data-testid="user-menu"] .cursor-pointer')
      await page.waitFor(`[...document.querySelectorAll('.ec-dropdown-menu__item')].some(e => e.offsetParent && e.innerText.includes('修改密码'))`, { what: '用户菜单里有修改密码' })
      await page.click('.ec-dropdown-menu__item', '修改密码')
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog')].some(d => d.offsetParent && d.innerText.includes('修改密码'))`, { what: '修改密码对话框' })
      const inputs = `[...document.querySelectorAll('.ec-dialog')].find(d => d.offsetParent && d.innerText.includes('修改密码')).querySelectorAll('input')`
      await page.eval(`(() => { const set = (el, v) => { el.value = v; el.dispatchEvent(new Event('input', { bubbles: true })) }; const [a, b, c] = ${inputs}; set(a, 'alice-pass-1'); set(b, 'alice-pass-2'); set(c, 'alice-pass-2') })()`)
      await page.click('.ec-dialog__footer button', '保存')
      await page.waitFor(`document.body.innerText.includes('密码已修改')`, { what: '修改成功提示' })
      await logout()

      await login('alice', 'alice-pass-1')
      await page.waitFor(`document.querySelector('[data-testid="login-error"]')?.innerText.includes('用户名或密码不正确')`, { what: '旧密码不能再登录' })
      await login('alice', 'alice-pass-2')
      await page.waitFor(`location.pathname === '/excel/home'`, { what: '新密码登录', timeout: 60_000 })
      noErrors()
    }
    catch (e) {
      await shot('login-local-2')
      throw e
    }
  })
})
