// AI 助手的数据清洗 / 窗口计算建议（界面）：在数据表页面让 AI 体检并给出清洗建议 → 卡片预览改前改后 → 应用（删除行要确认）→
// 打开着的数据表自动刷新（行数不变的查找替换也刷新）→ 撤销；窗口计算建议应用后生成数据集、撤销后移入回收站
import assert from 'node:assert/strict'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { api, llm, startServices, TOKEN, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const skip = findChrome() ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'
const COOKIE = 'gridlake-token'
const ALLOWED = [/command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]
const CSV = '单号,店铺,金额\n1,北京店,10\n1,北京店,10\n2,北京,20\n3,上海店,30\n3,上海店,30\n4,上海店,40\n'

describe('AI 数据清洗 / 窗口计算建议（界面）', { skip }, () => {
  let services
  let browser
  let datasetId

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    await api.registerUsers()
    datasetId = await api.importDataset(TOKEN.alice, 'AI订单', CSV)
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  const card = `([...document.querySelectorAll('.ai .card')].at(-1)?.innerText ?? '')`
  const footer = col => `(document.querySelector('[data-testid="grid-footer"] [data-col="${col}"]')?.innerText ?? '').replace(/\\s+/g, ' ')`
  const rows = async () => (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.id === datasetId).rowCount
  const names = async () => (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.map(d => d.name)

  /** 点最后一张建议卡上的按钮 */
  async function clickLast(page, text) {
    const ok = await page.eval(`(() => { const b = [...([...document.querySelectorAll('.ai .card')].at(-1)?.querySelectorAll('button') ?? [])].find(x => x.innerText.includes(${JSON.stringify(text)})); b?.click(); return !!b })()`)
    assert.ok(ok, `最后一张卡上没有「${text}」按钮`)
  }

  async function ask(page, text, done) {
    if (!await page.eval(`!!document.querySelector('.side-panel .panel-title')?.innerText.includes('AI 分析助手')`))
      await page.click('.tool.ai')
    await page.waitFor(`document.querySelector('.side-panel .panel-title')?.innerText.includes('AI 分析助手')`, { what: 'AI 面板' })
    await page.fill('.ai textarea', text)
    await page.click('.ai button', '发送')
    await page.waitFor(`[...document.querySelectorAll('.ai .answer')].some(a => a.innerText.includes(${JSON.stringify(done)}))`, { what: 'AI 回答结束', timeout: 30_000 })
  }

  it('清洗建议：预览 → 应用 → 数据表刷新 → 撤销；窗口计算建议：应用 → 撤销', async () => {
    const page = await browser.newPage()
    llm.reset()
    llm.say({ calls: [{ name: 'profile_dataset', args: { dataset: 'AI订单' } }] })
    llm.say({ calls: [{ name: 'propose_clean', args: { dataset: 'AI订单', op: 'dedupe' } }] })
    llm.say({ text: '去重建议已生成。' })
    llm.say({ calls: [{ name: 'propose_clean', args: { dataset: 'AI订单', op: 'replace', column: '店铺', find: '北京', replace: '北京店', whole: true } }] })
    llm.say({ text: '替换建议已生成。' })
    llm.say({ calls: [{ name: 'create_window_dataset', args: { dataset: 'AI订单', name: 'AI店铺排名', calcs: [{ func: 'rank', column: '金额', name: '店内排名' }], partitionBy: ['店铺'], orderBy: [{ column: '金额', desc: true }] } }] })
    llm.say({ text: '排名建议已生成。' })
    try {
      await page.clearCookies()
      await page.setCookie(COOKIE, TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?ds=${datasetId}&lang=zh-CN`)
      await page.waitFor(`document.body.innerText.includes('6 行')`, { timeout: 90_000, what: '打开数据表' })
      // 汇总行显示「店铺」的不同值个数：用来确认行数不变的修改后数据表也重新取了数
      await page.eval(`localStorage.setItem('big-grid-totals-on:${datasetId}', 'true'); localStorage.setItem('big-grid-totals:${datasetId}', JSON.stringify({ 店铺: 'distinct' })); 0`)
      await page.goto(`${WEB}/excel?ds=${datasetId}&lang=zh-CN`)
      await page.waitFor(`${footer('店铺')} === '不同值 3'`, { timeout: 90_000, what: '汇总行：店铺 3 种' })

      await ask(page, 'AI订单的数据有没有问题？帮我清理一下', '去重建议已生成')
      assert.match(JSON.stringify(llm.requests[1].messages.at(-1)), /共 6 行/, '模型拿到了数据概况')
      await page.waitFor(`${card}.includes('清洗「AI订单」') && ${card}.includes('6 行中将删除 2 行')`, { what: '清洗建议卡' })
      assert.equal(await page.eval(`!!document.querySelector('.ai .card .preview table')`), true, '卡片带要删的行')
      await page.screenshot(path.join(ARTIFACTS, 'ai-clean-card.png')).catch(() => {})
      // 数据集页面里 AI 面板不能被数据表挡住：面板中间那一点上最上层的元素属于面板
      assert.equal(await page.eval(`(() => { const r = document.querySelector('.side-panel').getBoundingClientRect(); return !!document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2)?.closest('.side-panel') })()`), true, 'AI 面板在数据表上面')
      assert.equal(await rows(), 6, '确认之前不改数据')
      await clickLast(page, '应用')
      await page.waitFor(`[...document.querySelectorAll('.ec-message-box')].some(e => e.offsetParent && e.innerText.includes('删除 2 行'))`, { what: '删除前确认' })
      await page.click('.ec-message-box__btns button', '删除')
      await page.waitFor(`${card}.includes('已清洗「AI订单」')`, { what: '已应用', timeout: 60_000 })
      assert.equal(await rows(), 4)
      await page.waitFor(`document.body.innerText.includes('4 行')`, { what: '数据表刷新成 4 行', timeout: 30_000 })

      await ask(page, '把「北京」统一成「北京店」', '替换建议已生成')
      await page.waitFor(`${card}.includes('将修改 1 行') && ${card}.includes('改后')`, { what: '替换建议卡' })
      await clickLast(page, '应用')
      await page.waitFor(`${card}.includes('已清洗「AI订单」') && ${card}.includes('修改 1 行')`, { what: '替换已应用', timeout: 60_000 })
      await page.waitFor(`${footer('店铺')} === '不同值 2'`, { what: '行数不变，数据表也重新取数（店铺剩 2 种）', timeout: 30_000 })
      await page.screenshot(path.join(ARTIFACTS, 'ai-clean.png')).catch(() => {})
      await clickLast(page, '撤销')
      await page.waitFor(`${card}.includes('已撤销对「AI订单」的清洗')`, { what: '替换已撤销', timeout: 60_000 })
      await page.waitFor(`${footer('店铺')} === '不同值 3'`, { what: '撤销后数据表刷新', timeout: 30_000 })
      // 更早的去重卡片：最近一次清洗已经不是它，不能撤销（不会误撤销别的清洗）
      await page.eval(`[...[...document.querySelectorAll('.ai .card')].find(c => c.innerText.includes('删除 2 行')).querySelectorAll('button')].find(b => b.innerText.includes('撤销')).click(); 0`)
      await page.waitFor(`document.body.innerText.includes('只能撤销最近一次')`, { what: '旧的清洗不能撤销', timeout: 30_000 })
      assert.equal(await rows(), 4, '没有误撤销')

      await ask(page, '每家店铺按金额排名，生成数据集', '排名建议已生成')
      await page.waitFor(`${card}.includes('窗口计算生成数据集「AI店铺排名」') && ${card}.includes('店内排名')`, { what: '窗口计算建议卡' })
      assert.ok(!(await names()).includes('AI店铺排名'), '确认之前不创建数据集')
      await clickLast(page, '应用')
      await page.waitFor(`${card}.includes('已用窗口计算生成数据集「AI店铺排名」')`, { what: '已生成', timeout: 60_000 })
      assert.ok((await names()).includes('AI店铺排名'), '数据集创建了')
      await clickLast(page, '撤销')
      await page.waitFor(`${card}.includes('已撤销') || ${card}.includes('已删除数据集')`, { what: '已撤销', timeout: 60_000 })
      assert.ok(!(await names()).includes('AI店铺排名'), '撤销后数据集移入回收站')
      assert.deepEqual(page.errors.filter(e => !ALLOWED.some(re => re.test(e))), [], '没有控制台报错')
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, 'ai-clean-failed.png')).catch(() => {})
      throw e
    }
  }, { timeout: 300_000 })
})
