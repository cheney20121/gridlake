// 管理员要求两步验证（LEDGER_REQUIRE_2FA=1）：没开启的账号登录后自动弹出设置对话框（不能关闭），设置完成并保存恢复码后才能继续用
import assert from 'node:assert/strict'
import { Buffer } from 'node:buffer'
import { createHmac } from 'node:crypto'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { LOCAL_ADMIN, startServices, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
// 设置之前其他接口被拦（403）是预期的
const ALLOWED = [/管理员要求所有内置账号开启两步验证/, /403|Forbidden/i, /401|Unauthorized/i, /command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]

function totp(secretB32, offset = 0) {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'
  let bits = ''
  for (const c of secretB32.replace(/=+$/, '').toUpperCase()) bits += alphabet.indexOf(c).toString(2).padStart(5, '0')
  const key = Buffer.from(bits.match(/.{8}/g).map(b => Number.parseInt(b, 2)))
  const msg = Buffer.alloc(8)
  msg.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30_000) + offset))
  const h = createHmac('sha1', key).update(msg).digest()
  const o = h[19] & 15
  return String((h.readUInt32BE(o) & 0x7FFFFFFF) % 1_000_000).padStart(6, '0')
}

describe('要求两步验证（界面）', { skip }, () => {
  let services
  let browser
  let page

  before(async () => {
    services = await startServices({ auth: 'local', env: { LEDGER_REQUIRE_2FA: '1' } })
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

  it('登录后自动弹出设置对话框，不能关闭；设置完成、保存恢复码后继续', async () => {
    try {
      await page.goto(`${WEB}/excel/home`)
      await page.waitFor(`!!document.querySelector('input[data-testid="login-username"]')`, { what: '登录表单' })
      await page.fill('input[data-testid="login-username"]', LOCAL_ADMIN.username)
      await page.fill('input[data-testid="login-password"]', LOCAL_ADMIN.password)
      await page.click('[data-testid="login-submit"]')
      await page.waitFor(`location.pathname === '/excel/home'`, { what: '进入首页', timeout: 60_000 })
      await page.waitFor(`!!document.querySelector('[data-testid="twofactor-required"]')`, { what: '自动弹出，带「管理员要求」的提示', timeout: 60_000 })
      assert.equal(await page.eval(`!!document.querySelector('[data-testid="twofactor-dialog"] .ec-dialog__headerbtn')`), false, '没有关闭按钮')
      await page.press('Escape')
      assert.equal(await page.eval(`!!document.querySelector('[data-testid="twofactor-required"]')`), true, 'Esc 关不掉')

      await page.click('[data-testid="twofactor-start"]')
      await page.waitFor(`!!document.querySelector('[data-testid="twofactor-secret"]')`, { what: '显示密钥' })
      const secret = (await page.eval(`document.querySelector('[data-testid="twofactor-secret"]').innerText`)).trim()
      await page.fill('input[data-testid="twofactor-code"]', totp(secret))
      await page.click('[data-testid="twofactor-enable"]')
      await page.waitFor(`document.querySelectorAll('[data-testid="twofactor-recovery-code"]').length === 10`, { what: '显示恢复码' })
      assert.equal(await page.eval(`!!document.querySelector('[data-testid="twofactor-continue"]')`), true, '保存恢复码后才能继续')
      await page.click('[data-testid="twofactor-continue"]')
      // 刷新后不再弹出，首页正常加载
      await page.waitFor(`document.querySelector('.home-header')?.innerText.includes('管理员')`, { what: '刷新后首页正常显示', timeout: 60_000 })
      assert.equal(await page.eval(`!!document.querySelector('[data-testid="twofactor-required"]')`), false)
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [])
    }
    catch (e) {
      await shot('login-require2fa')
      throw e
    }
  })
})
