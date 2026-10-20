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

  it('访问令牌：创建（只显示一次）→ 用它调接口 → 吊销后失效', async () => {
    try {
      // 上一个用例结束时已登录为 alice，停在首页
      const call = token => page.eval(`fetch('/ledger-api/api/ledger/workbooks', { headers: { Authorization: 'Bearer ${token}' } }).then(r => r.status)`)
      await page.click('[data-testid="user-menu"] .cursor-pointer')
      await page.waitFor(`!!document.querySelector('[data-testid="user-menu-tokens"]')`, { what: '用户菜单里有访问令牌' })
      await page.click('[data-testid="user-menu-tokens"]')
      await page.waitFor(`!!document.querySelector('[data-testid="token-name"]')`, { what: '访问令牌对话框' })
      await page.click('[data-testid="token-create"]')
      await page.waitFor(`document.querySelector('[data-testid="token-error"]')?.innerText.includes('请填写令牌名称')`, { what: '名称为空的提示' })
      await page.fill('input[data-testid="token-name"]', 'ui-test')
      await page.click('[data-testid="token-create"]')
      await page.waitFor(`!!document.querySelector('[data-testid="token-value"]')`, { what: '创建后显示完整令牌' })
      const token = await page.eval(`document.querySelector('[data-testid="token-value"]').innerText.trim()`)
      assert.match(token, /^glp_[\w-]{43}$/)
      assert.equal(await call(token), 200, '令牌可以调接口')
      assert.equal(await call('glp_wrong'), 401)
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog .ec-table__row')].some(r => r.innerText.includes('ui-test') && r.innerText.includes(${JSON.stringify(token.slice(0, 8))}))`, { what: '列表里出现这个令牌（只有前缀）' })
      assert.equal(await page.eval(`document.querySelector('.ec-dialog .ec-table').innerText.includes(${JSON.stringify(token)})`), false, '列表里不含完整令牌')

      await page.click('[data-testid="token-revoke-ui-test"]')
      await page.waitFor(`!!document.querySelector('.ec-message-box')`, { what: '吊销确认' })
      await page.click('.ec-message-box__btns button', '吊销')
      await page.waitFor(`![...document.querySelectorAll('.ec-dialog .ec-table__row')].some(r => r.innerText.includes('ui-test'))`, { what: '列表里没有了' })
      assert.equal(await call(token), 401, '吊销后立即失效')
      noErrors()
    }
    catch (e) {
      await shot('login-local-3')
      throw e
    }
  })

  it('只读访问令牌：勾选「只读令牌」创建 → 能读、写被拒绝', async () => {
    try {
      const call = (token, method = 'GET', path = 'workbooks', body) => page.eval(`fetch('/ledger-api/api/ledger/${path}', { method: '${method}', headers: { Authorization: 'Bearer ${token}'${body ? ', \'Content-Type\': \'application/json\'' : ''} }${body ? `, body: ${JSON.stringify(JSON.stringify(body))}` : ''} }).then(r => r.status)`)
      // 上一个用例结束时对话框还开着（吊销后留在列表里）
      await page.fill('input[data-testid="token-name"]', 'ro-test')
      await page.click('[data-testid="token-readonly"]')
      await page.click('[data-testid="token-create"]')
      await page.waitFor(`!!document.querySelector('[data-testid="token-value"]')`, { what: '创建后显示完整令牌' })
      const token = await page.eval(`document.querySelector('[data-testid="token-value"]').innerText.trim()`)
      assert.match(token, /^glr_[\w-]{43}$/)
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog .ec-table__row')].some(r => r.innerText.includes('ro-test') && r.innerText.includes('只读令牌'))`, { what: '列表里标出只读' })
      assert.equal(await call(token), 200, '只读令牌能读')
      assert.equal(await call(token, 'POST', 'folders', { name: 'x', parentId: null }), 403, '只读令牌不能写')
      assert.equal(await call(token, 'POST', 'sql/run', { sql: 'SELECT 1' }), 200, '计算类查询可以')
    }
    catch (e) {
      await shot('login-local-4')
      throw e
    }
  })
})
