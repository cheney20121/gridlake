// 外部数据集（界面）：管理员在「导入数据」里选「直接查询服务器上的 Parquet 文件」，预览后创建，打开数据表浏览、刷新文件列表
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { LOCAL_ADMIN, startServices, WEB } from './infra.mjs'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const ARTIFACTS = path.join(HERE, '.artifacts')
const FIXTURES = path.resolve(HERE, '../../../server/tests/e2e/fixtures/external')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
const ALLOWED = [/401|Unauthorized/i, /command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]

describe('外部数据集（界面）', { skip }, () => {
  let services
  let browser
  let page
  const inbox = fs.mkdtempSync(path.join(os.tmpdir(), 'ui-inbox-'))
  const dir = path.join(inbox, '订单')

  before(async () => {
    fs.mkdirSync(dir)
    for (const f of ['orders-2026-09.parquet', 'orders-2026-10.parquet'])
      fs.copyFileSync(path.join(FIXTURES, f), path.join(dir, f))
    services = await startServices({ auth: 'local', packs: 'none', env: { LEDGER_INBOX_ROOTS: inbox } })
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
      fs.rmSync(inbox, { recursive: true, force: true })
    }
  })

  it('预览 → 创建 → 打开数据表 → 刷新文件列表', async () => {
    try {
      // 这个环境的 Chromium 不按 --lang 设置界面语言：用 ?lang= 固定中文（会被记住）
      await page.goto(`${WEB}/login?lang=zh-CN`)
      await page.waitFor(`!!document.querySelector('input[data-testid="login-username"]')`, { what: '登录表单' })
      await page.fill('input[data-testid="login-username"]', LOCAL_ADMIN.username)
      await page.fill('input[data-testid="login-password"]', LOCAL_ADMIN.password)
      await page.click('[data-testid="login-submit"]')
      await page.waitFor(`location.pathname === '/excel/home'`, { what: '首页', timeout: 60_000 })

      await page.waitFor(`[...document.querySelectorAll('.home-card')].some(e => e.innerText.includes('导入数据'))`, { what: '首页「导入数据」卡片', timeout: 60_000 })
      await page.click('.home-card', '导入数据')
      await page.waitFor(`document.body.innerText.includes('直接查询服务器上的 Parquet 文件')`, { what: '导入对话框里的外部文件入口' })
      await page.click('button', '直接查询服务器上的 Parquet 文件（不导入）…')
      await page.waitFor(`document.body.innerText.includes('可用的目录')`, { what: '外部文件对话框（含可用目录提示）' })
      await page.fill('.ec-dialog input[placeholder*="相对于第一个可用目录"]', '订单')
      await page.click('.ec-dialog__footer button', '预览')
      await page.waitFor(`document.body.innerText.includes('2 个文件') && document.body.innerText.includes('1,500 行')`, { what: '预览：2 个文件、1500 行', timeout: 60_000 })
      const body = await page.eval(`document.body.innerText`)
      for (const w of ['整数', '金额', '日期时间', '是/否'])
        assert.ok(body.includes(w), `预览表头显示列类型「${w}」`)
      assert.ok(body.includes('2026-09-01 08:00:00'), '日期时间列按约定显示')
      await page.click('.ec-dialog__footer button', '创建数据集')
      await page.waitFor(`location.pathname === '/excel' && location.search.includes('ds=')`, { what: '进入编辑器打开数据集', timeout: 60_000 })
      await page.waitFor(`document.body.innerText.includes('外部文件') && document.body.innerText.includes('1,500 行')`, { what: '数据表：外部文件标签、1500 行', timeout: 60_000 })

      await page.waitFor(`[...document.querySelectorAll('.data-panel')].some(e => e.innerText.includes('外部文件（直接查询）') && e.innerText.includes('订单'))`, { what: '左侧数据面板里有「外部文件」分组和这个数据集' })

      fs.copyFileSync(path.join(FIXTURES, 'orders-2026-11.parquet'), path.join(dir, 'orders-2026-11.parquet'))
      await page.click('button', '刷新文件列表')
      await page.waitFor(`document.body.innerText.includes('新增列：备注')`, { what: '刷新结果提示', timeout: 60_000 })
      await page.waitFor(`document.body.innerText.includes('1,510 行')`, { what: '刷新后 1510 行', timeout: 60_000 })

      // 再刷新一次：文件没变
      await page.click('button', '刷新文件列表')
      await page.waitFor(`document.body.innerText.includes('文件没有变化')`, { what: '没有变化的提示', timeout: 60_000 })
      // 设置：改成每 5 分钟自动检查
      await page.click('[data-testid="external-settings"]')
      await page.waitFor(`document.body.innerText.includes('外部数据集设置') && !!document.querySelector('[data-testid="external-refresh-minutes"]')`, { what: '外部数据集设置对话框' })
      const sel = await page.eval(`(() => { const r = document.querySelector('[data-testid="external-refresh-minutes"]').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] })()`)
      await page.mouseClick(sel[0], sel[1])
      await page.waitFor(`[...document.querySelectorAll('.ec-select-dropdown__item')].some(e => e.getBoundingClientRect().width > 0 && e.innerText.trim() === '每 5 分钟')`, { what: '间隔选项' })
      await page.eval(`[...document.querySelectorAll('.ec-select-dropdown__item')].find(e => e.getBoundingClientRect().width > 0 && e.innerText.trim() === '每 5 分钟').click()`)
      await page.click('[data-testid="external-save"]')
      await page.waitFor(`(document.querySelector('[data-testid="external-tag"]')?.innerText ?? '').includes('自动')`, { what: '标签显示「外部文件 · 自动」', timeout: 60_000 })
      await page.screenshot(path.join(ARTIFACTS, 'external-parquet.png')).catch(() => {})
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'external-parquet-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
