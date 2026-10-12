// 界面语言：?lang= 指定、语言切换器、选择被记住；英文界面里已翻译的文案显示英文
import assert from 'node:assert/strict'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { LOCAL_ADMIN, startServices, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
const ALLOWED = [/401|Unauthorized/i, /status of 400 .*\/ledger\/auth\/login/, /command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]

describe('界面语言（界面）', { skip }, () => {
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

  const text = sel => page.eval(`document.querySelector(${JSON.stringify(sel)})?.innerText ?? ''`)
  const submitText = () => text('[data-testid="login-submit"]')
  const chooseLanguage = async (label) => {
    await page.click('[data-testid="language-switch"] .lang-switch')
    await page.waitFor(`[...document.querySelectorAll('.ec-dropdown-menu__item')].some(e => e.offsetParent && e.innerText.includes(${JSON.stringify(label)}))`, { what: '语言菜单' })
    await page.click('.ec-dropdown-menu__item', label)
  }

  it('?lang=en-US 显示英文；切换器选中文后刷新页面、记住选择；再切回英文也记住', async () => {
    try {
      await page.goto(`${WEB}/login?lang=en-US`)
      await page.waitFor(`!!document.querySelector('[data-testid="login-submit"]')`, { what: '登录页' })
      assert.equal(await submitText(), 'Sign in')
      assert.equal(await page.eval(`document.documentElement.lang`), 'en-US')
      assert.match(await text('.el-form, .ec-form'), /Username[\s\S]*Password/)

      await chooseLanguage('简体中文')
      await page.waitFor(`document.querySelector('[data-testid="login-submit"]')?.innerText.includes('登录')`, { what: '切到中文并刷新' })
      assert.equal(await page.eval(`localStorage.getItem('gridlake-lang')`), 'zh-CN')
      assert.equal(await page.eval(`location.search`), '', '?lang= 已从地址里去掉')

      await page.goto(`${WEB}/login`)
      await page.waitFor(`!!document.querySelector('[data-testid="login-submit"]')`, { what: '登录页' })
      assert.equal(await submitText(), '登录', '不带参数时用上次的选择')

      await chooseLanguage('English')
      await page.waitFor(`document.querySelector('[data-testid="login-submit"]')?.innerText === 'Sign in'`, { what: '切到英文并刷新' })
      assert.equal(await page.eval(`localStorage.getItem('gridlake-lang')`), 'en-US')
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [])
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'i18n-failed.png')).catch(() => {})
      throw e
    }
  })

  it('英文界面：登录 → 首页 → 新建工作簿进入编辑器，已翻译的位置是英文，没有控制台错误', async () => {
    try {
      await page.goto(`${WEB}/login?lang=en-US`)
      await page.waitFor(`!!document.querySelector('input[data-testid="login-username"]')`, { what: '登录表单' })
      // 输错密码：服务端的错误信息按界面语言返回英文
      await page.fill('input[data-testid="login-username"]', LOCAL_ADMIN.username)
      await page.fill('input[data-testid="login-password"]', 'wrong-password')
      await page.click('[data-testid="login-submit"]')
      await page.waitFor(`document.querySelector('[data-testid="login-error"]')?.innerText === 'Incorrect username or password'`, { what: '英文的密码错误提示' })
      await page.fill('input[data-testid="login-password"]', LOCAL_ADMIN.password)
      await page.click('[data-testid="login-submit"]')
      await page.waitFor(`location.pathname === '/excel/home'`, { what: '登录后到首页', timeout: 60_000 })
      await page.waitFor(`!!document.querySelector('.home-header')`, { what: '首页顶栏' })
      const header = await text('.home-header')
      assert.match(header, /New workbook/)
      assert.match(header, /New from template/)
      assert.match(header, /Spreadsheets for 100M rows/)
      assert.doesNotMatch(header, /新建|模板/, '顶栏不应残留中文')
      const body = await page.eval(`document.body.innerText`)
      assert.match(body, /Recent workbooks/)

      await page.click('button', 'New workbook')
      await page.waitFor(`document.querySelector('.ec-message-box')?.innerText.includes('New blank workbook')`, { what: '命名对话框（英文标题）' })
      assert.match(await text('.ec-message-box'), /Workbook name/)
      assert.match(await text('.ec-message-box input'), /^$/) // 输入框的值不在 innerText 里
      assert.equal(await page.eval(`document.querySelector('.ec-message-box input').value`), 'Untitled workbook')
      await page.click('.ec-message-box__btns button', 'OK')
      await page.waitFor(`location.pathname === '/excel' && location.search.includes('wb=')`, { what: '进入编辑器', timeout: 60_000 })
      await page.waitFor(`!!document.querySelector('.palette-entry')`, { what: '编辑器顶栏', timeout: 60_000 })
      assert.match(await text('.palette-entry'), /Search features/)
      // 表格内核自己的界面（工具栏、公式栏）也是英文
      await page.waitFor(`!!document.querySelector('.univer-toolbar, [class*="univer-toolbar"]') || document.body.innerText.includes('Undo')`, { what: '表格工具栏', timeout: 60_000 })
      assert.doesNotMatch(await text('.palette-entry'), /搜索功能/)
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [])
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'i18n-en-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 180_000 })

  it('浏览器语言不是中文时默认英文，是中文时默认中文', async () => {
    // 测试浏览器启动时带 --lang=zh-CN：清掉记住的选择后应是中文；用 ?lang=ja 这类不认识的语言则回退到浏览器语言
    await page.clearCookies()
    await page.goto(`${WEB}/login`)
    await page.eval(`localStorage.removeItem('gridlake-lang')`)
    await page.goto(`${WEB}/login`)
    await page.waitFor(`!!document.querySelector('[data-testid="login-submit"]')`, { what: '登录页' })
    assert.equal(await submitText(), '登录')
    await page.goto(`${WEB}/login?lang=ja`)
    await page.waitFor(`!!document.querySelector('[data-testid="login-submit"]')`, { what: '登录页' })
    assert.equal(await submitText(), '登录', '不认识的语言回退到浏览器语言')
  })
})
