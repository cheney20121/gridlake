// 单点登录（OIDC）的界面流程：登录页出现「用 XX 登录」→ 点击后整页跳转到身份提供方并回来 → 已登录；拒绝授权时回到登录页并给出提示
import assert from 'node:assert/strict'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { PORTS, startServices, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
const ALLOWED = [/401|Unauthorized/i, /command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]

describe('单点登录（界面）', { skip }, () => {
  let services
  let browser
  let page

  before(async () => {
    services = await startServices({ auth: 'local', oidc: true })
    browser = await launchChrome()
    page = await browser.newPage()
    await page.clearCookies()
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      await services?.stop()
    }
  })

  const shot = name => page.screenshot(path.join(ARTIFACTS, `${name}.png`)).catch(() => {})
  const idp = (user, deny = false) => fetch(`http://127.0.0.1:${PORTS.oidc}/__next?user=${user}&deny=${deny ? 1 : 0}`)

  it('登录页有 SSO 按钮；点击后登录成功并回到原来要去的页面', async () => {
    try {
      await idp('zhang')
      await page.goto(`${WEB}/excel/home`)
      await page.waitFor(`location.pathname === '/login'`, { what: '未登录跳转登录页' })
      await page.waitFor(`document.querySelector('[data-testid="login-sso"]')?.innerText.includes('用 Mock-SSO 登录')`, { what: 'SSO 按钮' })
      assert.equal(await page.eval(`!!document.querySelector('input[data-testid="login-username"]')`), true, '密码登录仍然保留（应急用）')
      await page.click('[data-testid="login-sso"]')
      await page.waitFor(`location.pathname === '/excel/home'`, { what: '登录后回到首页', timeout: 60_000 })
      await page.waitFor(`document.querySelector('.home-header')?.innerText.includes('张三')`, { what: '首页显示 SSO 登录的用户', timeout: 60_000 })
      // 没有账号管理入口：自动建的是普通账号
      await page.click('button.nav-item', '自动化')
      await page.waitFor(`document.body.innerText.includes('外部 AI 接入') || document.body.innerText.includes('定时推送')`, { what: '自动化分区' })
      assert.equal(await page.eval(`!!document.querySelector('[data-testid="home-accounts"]')`), false)
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [])
    }
    catch (e) {
      await shot('login-sso-1')
      throw e
    }
  })

  it('管理员组的人登录后是管理员；在身份提供方拒绝授权会回到登录页并提示', async () => {
    try {
      await page.clearCookies()
      await idp('boss')
      await page.goto(`${WEB}/login`)
      await page.waitFor(`!!document.querySelector('[data-testid="login-sso"]')`, { what: 'SSO 按钮' })
      await page.click('[data-testid="login-sso"]')
      await page.waitFor(`location.pathname === '/excel/home'`, { what: '管理员登录', timeout: 60_000 })
      await page.waitFor(`document.querySelector('.home-header')?.innerText.includes('老板')`, { what: '首页显示登录的用户', timeout: 60_000 })
      await page.click('button.nav-item', '自动化')
      await page.waitFor(`!!document.querySelector('[data-testid="home-accounts"]')`, { what: '管理员有账号管理入口' })

      await page.clearCookies()
      await idp('zhang', true)
      await page.goto(`${WEB}/login`)
      await page.waitFor(`!!document.querySelector('[data-testid="login-sso"]')`, { what: 'SSO 按钮' })
      await page.click('[data-testid="login-sso"]')
      await page.waitFor(`location.pathname === '/login' && location.search.includes('sso_error=denied')`, { what: '回到登录页' })
      await page.waitFor(`document.querySelector('[data-testid="login-error"]')?.innerText.includes('你取消了授权')`, { what: '拒绝授权的提示' })
    }
    catch (e) {
      await shot('login-sso-2')
      throw e
    }
  })
})
