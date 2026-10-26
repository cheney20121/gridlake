// 数据校验（界面）：「更多 → 数据校验」→ 添加「编号不能重复」「手机号是手机号格式」→ 保存并检查 → 每条规则的不合格行数和样例 →
// 「只看这些行」在数据表上筛出来 → 数据表上显示「校验：2 项不通过」；「推荐规则」按数据推荐、勾选后加进列表
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
const CSV = '编号,手机号,等级\nA1,13800000000,金卡\nA2,1380000,银卡\nA2,13900000000,金卡\nA3,abc,金卡\n'
// 30 行：单号 D001 重复一次；手机号一个位数不对、一个空；等级「钻石」只出现一次；金额一个负数
const MEMBERS = `单号,手机号,等级,金额\n${Array.from({ length: 30 }, (_, k) => {
  const i = k + 1
  return [i === 30 ? 'D001' : `D${String(i).padStart(3, '0')}`, i === 5 ? '1380000' : i === 6 ? '' : `138${String(i).padStart(8, '0')}`, i === 9 ? '钻石' : ['金卡', '银卡', '普通'][i % 3], i === 8 ? -5 : i * 10].join(',')
}).join('\n')}\n`

/**
 * 点开 Element 下拉（真实鼠标点在中间，位置稳定后再点，没弹出来就重试）并在**它自己的**下拉列表里选文字完全一致的选项
 * （同一页上有好几个下拉时，只认这个下拉的输入框 aria-controls 指向的列表）
 */
async function pick(page, selector, text) {
  const list = `document.getElementById(document.querySelector(${JSON.stringify(selector)}).querySelector('input')?.getAttribute('aria-controls') ?? '')`
  const visible = `[...(${list}?.querySelectorAll('.ec-select-dropdown__item') ?? [])].find(e => e.getBoundingClientRect().width > 0 && e.innerText.trim() === ${JSON.stringify(text)})`
  const rect = `(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return [Math.round(r.left), Math.round(r.top), r.width, r.height] })()`
  let last = ''
  for (let i = 0; i < 20; i++) {
    const now = JSON.stringify(await page.eval(rect))
    if (now === last)
      break
    last = now
    await new Promise(r => setTimeout(r, 100))
  }
  for (let attempt = 0; ; attempt++) {
    const [x, y, w, h] = await page.eval(rect)
    await page.mouseClick(x + w / 2, y + h / 2)
    try {
      await page.waitFor(`!!${visible}`, { what: `选项「${text}」`, timeout: 4000 })
      break
    }
    catch (e) {
      if (attempt >= 2)
        throw e
    }
  }
  await page.eval(`${visible}.click()`)
  // 等下拉收起，免得下一次点到它上面
  await page.waitFor(`!(${list}?.getBoundingClientRect().width > 0) || ${list}?.closest('.ec-popper')?.style.display === 'none'`, { what: '下拉收起', timeout: 5000 }).catch(() => {})
}

describe('数据校验（界面）', { skip }, () => {
  let services
  let browser
  let datasetId
  let membersId

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    datasetId = await api.importDataset(TOKEN.alice, '会员', CSV)
    membersId = await api.importDataset(TOKEN.alice, '推荐会员', MEMBERS)
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  it('设规则 → 保存并检查 → 只看不合格的行', async () => {
    const page = await browser.newPage()
    const report = `(document.querySelector('[data-testid="validation-report"]')?.innerText ?? '')`
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?ds=${datasetId}&lang=zh-CN`)
      await page.waitFor(`document.body.innerText.includes('4 行')`, { timeout: 90_000, what: '打开数据表' })
      await page.click('button', '更多 ▾')
      await page.waitFor(`!!document.querySelector('[data-testid="open-validation"]')?.offsetParent`, { what: '更多菜单里的数据校验' })
      await page.click('[data-testid="open-validation"]')
      await page.waitFor(`document.querySelector('[data-testid="validation-dialog"]')?.innerText.includes('还没有校验规则')`, { what: '还没有规则' })

      await page.click('[data-testid="add-rule"]')
      await pick(page, '[data-testid="rule-kind-0"]', '不能重复')
      await page.click('[data-testid="add-rule"]')
      await pick(page, '[data-testid="rule-1"] .ec-select', '手机号')
      await pick(page, '[data-testid="rule-kind-1"]', '格式')
      await page.waitFor(`document.querySelector('[data-testid="validation-save"]')?.disabled === false`, { what: '可以保存' })
      await page.click('[data-testid="validation-save"]')
      await page.waitFor(`${report}.includes('共 4 行，3 行至少有一项不合格（2 条规则不通过）')`, { what: '检查结果', timeout: 30_000 })
      assert.match(await page.eval(report), /「编号」不能重复\s*2 行不合格\s*如 「A2」/)
      assert.match(await page.eval(report), /「手机号」是手机号格式\s*2 行不合格/)
      await page.screenshot(path.join(ARTIFACTS, 'validation.png')).catch(() => {})
      await page.waitFor(`document.querySelector('[data-testid="validation-badge"]')?.innerText.includes('校验：2 项不通过（3 行）')`, { what: '数据表上的小结', timeout: 30_000 })

      await page.click('[data-testid="only-1"]')
      await page.waitFor(`document.querySelector('[data-testid="violation-tag"]')?.innerText.includes('只看不合格：「手机号」是手机号格式')`, { what: '筛选标签' })
      await page.waitFor(`document.body.innerText.includes('2 行') && document.body.innerText.includes('/ 共 4')`, { what: '数据表只剩 2 行', timeout: 30_000 })
      await page.eval(`document.querySelector('[data-testid="violation-tag"] .ec-tag__close').click(); 0`)
      await page.waitFor(`!document.querySelector('[data-testid="violation-tag"]') && !document.body.innerText.includes('/ 共 4')`, { what: '去掉筛选，回到 4 行', timeout: 30_000 })
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'validation-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })

  it('推荐规则：按数据推荐 → 去掉一条 → 加进列表 → 保存并检查', async () => {
    const page = await browser.newPage()
    const report = `(document.querySelector('[data-testid="validation-report"]')?.innerText ?? '')`
    const panel = `(document.querySelector('[data-testid="suggestions"]')?.innerText ?? '')`
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?ds=${membersId}&lang=zh-CN`)
      await page.waitFor(`document.body.innerText.includes('30 行')`, { timeout: 90_000, what: '打开数据表' })
      await page.click('button', '更多 ▾')
      await page.waitFor(`!!document.querySelector('[data-testid="open-validation"]')?.offsetParent`, { what: '更多菜单里的数据校验' })
      await page.click('[data-testid="open-validation"]')
      await page.waitFor(`document.querySelector('[data-testid="validation-dialog"]')?.innerText.includes('还没有校验规则')`, { what: '还没有规则' })
      await page.click('[data-testid="suggest-rules"]')
      await page.waitFor(`${panel}.includes('按现有数据推荐了 5 条规则')`, { what: '推荐了 5 条（导入时手机号被认成了数字列，不推荐格式）', timeout: 30_000 })
      const text = await page.eval(panel)
      assert.match(text, /「单号」不能重复\s*像是编号，但有 1 个值重复出现\s*现在 2 行不合格/)
      assert.match(text, /「等级」只能是 普通 \/ 银卡 \/ 金卡[\s\S]*「钻石」/)
      assert.match(text, /「金额」不小于 0\s*只有 1 个负数/)
      await page.screenshot(path.join(ARTIFACTS, 'validation-suggest.png')).catch(() => {})
      // 不要「金额不小于 0」
      await page.click('[data-testid="suggestion-4"] .ec-checkbox')
      await page.waitFor(`document.querySelector('[data-testid="add-suggestions"]')?.innerText.includes('添加选中的 4 条')`, { what: '只选 4 条' })
      await page.click('[data-testid="add-suggestions"]')
      await page.waitFor(`!document.querySelector('[data-testid="suggestions"]') && !!document.querySelector('[data-testid="rule-3"]') && !document.querySelector('[data-testid="rule-4"]')`, { what: '加进列表' })
      await page.click('[data-testid="validation-save"]')
      await page.waitFor(`${report}.includes('共 30 行，4 行至少有一项不合格（3 条规则不通过）')`, { what: '检查结果', timeout: 30_000 })
      assert.equal((await api.call('GET', `/ledger/datasets/${membersId}/validation`, TOKEN.alice)).data.rules.length, 4)
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'validation-suggest-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 240_000 })
})
