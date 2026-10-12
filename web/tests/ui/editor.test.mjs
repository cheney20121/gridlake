// 在线表格编辑器的界面冒烟测试：真实 Chrome + 真实前端 + 真实表格服务（模拟登录网关）。
//   npm run test:ui           需要本机有 Chrome / Chromium（CHROME_BIN 可指定）、python3、cargo（或用 LEDGER_BIN 指定已编译的服务）
// 这些测试固定住编辑器「能打开、能保存、各入口能用、权限相关的界面表现正确」，重构 views/excel 时用它做安全网。
import assert from 'node:assert/strict'
import { Buffer } from 'node:buffer'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { findChrome, launchChrome } from './cdp.mjs'
import { api, API, llm, startServices, TOKEN, WEB } from './infra.mjs'

const ARTIFACTS = path.join(path.dirname(fileURLToPath(import.meta.url)), '.artifacts')
const COOKIE = 'webexcel-token'

/**
 * 已知的、与被测功能无关的启动期控制台错误（只放行这一条，其他任何错误都会让测试失败）：
 * Univer 的 registerFunction 会在 10 毫秒后触发一次防抖的强制重算，而此时还没有创建工作簿，公式引擎的重算命令尚未注册，
 * 于是抛出这个异常。每次打开编辑器都会出现（注册 DSUMIFS、IMPORTRANGE 各一次），不影响功能：随后打开工作簿时会正常计算。
 * 想消除它需要把函数注册挪到第一个工作簿创建之后，会改变公式首次计算的时序，另行评估。
 */
/**
 * 撤销方案会删除刚建的透视表页签。Univer 的「自动行高」任务有时在页签删除之后才执行，读到已清空的工作表数据而抛出这个异常
 * （Univer 内部的竞态，与我们的代码无关，不影响结果：页签已删除、后续操作正常）。只在会删页签的场景里放行
 */
const UNIVER_REMOVED_SHEET_RACE = /Cannot destructure property 'rowData' of 'this\._worksheetData' as it is null/

const KNOWN_STARTUP_ERRORS = [/command "formula\.mutation\.set-trigger-formula-calculation-start" is not registered/]

/** 日销售 CSV：见 before 里的说明；以数据里最新的一天（2026-09-30）为基准，所以不随运行日期变化 */
function dailySales() {
  const rows = ['下单时间,店铺,金额']
  const last = Date.UTC(2026, 8, 30)
  for (let back = 59; back >= 0; back--) {
    const d = new Date(last - back * 86_400_000)
    const day = d.toISOString().slice(0, 10)
    const weekend = [0, 6].includes(d.getUTCDay())
    for (const [shop, base] of [['A', 10], ['B', 10], ['C', 5]]) {
      if (shop === 'C' && day >= '2026-09-24')
        continue
      let n = weekend ? Math.round(base / 2) : base
      if (day === '2026-09-25')
        n = 3
      for (let i = 0; i < n; i++)
        rows.push(`${day} ${String(9 + (i % 10)).padStart(2, '0')}:${String(i).padStart(2, '0')}:00,${shop},${100 + (i % 5) * 2}`)
    }
  }
  return `${rows.join('\n')}\n`
}

const chrome = findChrome()
const skip = chrome ? false : '本机没有 Chrome / Chromium（可用 CHROME_BIN 指定）'

describe('在线表格编辑器（界面）', { skip }, () => {
  let services
  let browser
  let ids // 用户 id
  const wb = {} // 造好的工作簿
  let ordersId

  before(async () => {
    services = await startServices()
    browser = await launchChrome()
    ids = await api.registerUsers()
    wb.alice = (await api.createWorkbook(TOKEN.alice, 'Alice 利润表')).id
    wb.shared = (await api.createWorkbook(TOKEN.alice, '共享只读看板')).id
    await api.shareWorkbook(TOKEN.alice, wb.shared, 'view')
    // 订单数据集：6 行（A 店 2、B 店 2、C 店 1、D 店 1），按店铺列过滤；Bob 只能看 A、B 两家
    ordersId = await api.importDataset(TOKEN.alice, '订单', '店铺,金额\nA,10\nA,20\nB,100\nB,200\nC,1000\nD,5000\n')
    await api.bindDataset(ordersId, { column: '店铺' })
    await api.setScope(ids.Bob, ['A', 'B'])
    // 日销售：60 天（周末只有平日一半，属常态），2026-09-25 全线故障、C 店 09-24 起消失；用来验证「异常发现」
    await api.importDataset(TOKEN.alice, '日销售', dailySales())
    // 预热：开发服务器冷启动时第一个页面会遇到依赖预构建与「公式引擎尚未就绪就触发重算」的启动竞态，
    // 先打开一次吸收掉，之后的测试在稳态下严格断言「没有任何控制台错误」
    const warm = await browser.newPage()
    await warm.setCookie(COOKIE, TOKEN.alice, WEB)
    await warm.goto(`${WEB}/excel?wb=${wb.alice}`)
    await warm.waitFor(`!!window.__univerAPI?.getActiveWorkbook()`, { timeout: 120_000, what: '预热页面打开工作簿' })
    await warm.close()
  }, { timeout: 300_000 })

  after(async () => {
    try {
      await browser?.close()
    }
    finally {
      services?.stop()
    }
  })

  /** 以某个用户打开页面，跑完关闭；失败时截图，并断言页面没有未预期的控制台错误 */
  async function withPage(name, user, url, fn, { allowErrors = [] } = {}) {
    allowErrors = [...KNOWN_STARTUP_ERRORS, ...allowErrors]
    const page = await browser.newPage()
    try {
      await page.clearCookies()
      if (user)
        await page.setCookie(COOKIE, TOKEN[user], WEB)
      await page.goto(`${WEB}${url}`)
      await fn(page)
      const errors = page.errors.filter(e => !allowErrors.some(re => re.test(e)))
      assert.deepEqual(errors, [], `页面出现控制台错误：\n${errors.join('\n')}`)
    }
    catch (e) {
      await page.screenshot(path.join(ARTIFACTS, `${name}.png`)).catch(() => {})
      throw e
    }
    finally {
      await page.close()
    }
  }

  /** 编辑器就绪：顶栏出现、Univer 工作簿已打开 */
  async function editorReady(page) {
    await page.waitFor(`!!document.querySelector('.excel-topbar') && !!window.__univerAPI?.getActiveWorkbook()`, { timeout: 90_000, what: '编辑器与工作簿加载完成' })
    await page.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { what: '保存状态显示「已保存」' })
  }

  const topbar = page => page.eval(`document.querySelector('.topbar-right')?.innerText.replace(/\\s+/g, ' ') ?? ''`)
  /** 点开顶栏里某个下拉（工作簿名 / 审批）再点其中一项 */
  async function menu(page, triggerSelector, itemText) {
    await page.click(triggerSelector)
    await page.waitFor(`[...document.querySelectorAll('.ec-dropdown-menu__item')].some(e => e.offsetParent && e.innerText.includes(${JSON.stringify(itemText)}))`, { what: `菜单项「${itemText}」` })
    await page.click('.ec-dropdown-menu__item', itemText)
  }

  it('未登录访问在线表格：跳转登录页并带上回跳地址', async () => {
    await withPage('redirect-to-login', null, '/excel/home', async (page) => {
      await page.waitFor(`location.pathname === '/login' && location.search.includes('redirect=')`, { what: '跳转到登录页' })
    }, { allowErrors: [/401|Unauthorized/i] })
  })

  it('编辑器能打开工作簿：顶栏、登录用户、画布、单元格内容', async () => {
    await withPage('editor-opens', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      assert.match(await topbar(page), /Alice/)
      assert.equal(await page.eval(`document.querySelector('.workbook-name')?.innerText.trim()`), 'Alice 利润表 ▾')
      assert.equal(await page.eval(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getSheetName()`), 'Sheet1')
      assert.equal(await page.eval(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(0, 0).getValue()`), '订单号')
      assert.equal(await page.eval(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(1, 1).getValue()`), 100)
      assert.ok(await page.eval(`!!document.querySelector('#univer-container canvas')`), '工作表画布已渲染')
      assert.ok(await page.eval(`!!document.querySelector('.data-panel')`), '左侧数据面板存在')
    })
  })

  it('修改单元格 → 显示未保存 → Ctrl+S 保存 → 服务端版本 +1 且内容已更新', async () => {
    await withPage('edit-and-save', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      const before = (await api.workbookMeta(TOKEN.alice, wb.alice)).version
      await page.eval(`void window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(3, 0).setValue('界面测试写入')`)
      await page.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('未保存')`, { what: '出现「有未保存的修改」' })
      await page.press('s', { ctrl: true })
      await page.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { what: 'Ctrl+S 后显示已保存' })
      const after = await api.workbookMeta(TOKEN.alice, wb.alice)
      assert.equal(after.version, before + 1)
      assert.equal(after.updatedBy, 'Alice')
      const r = await fetch(`${API}/ledger/workbooks/${wb.alice}/content`, { headers: { 'x-user-token': TOKEN.alice } })
      assert.match(JSON.stringify(await r.json()), /界面测试写入/)
    })
  })

  it('文件菜单：版本历史面板', async () => {
    await withPage('file-menu-history', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await menu(page, '.workbook-name', '版本历史')
      await page.waitFor(`document.querySelector('.side-panel .panel-title')?.innerText.includes('版本历史')`, { what: '版本历史面板' })
    })
  })

  it('命令面板（Alt+Q）：搜索「透视」→ 回车 → 打开透视表字段面板', async () => {
    await withPage('command-palette', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.press('q', { alt: true, code: 'KeyQ' })
      await page.waitFor(`!!document.querySelector('input[placeholder*="搜索功能"]')`, { what: '命令面板打开' })
      await page.fill('input[placeholder*="搜索功能"]', '透视')
      await page.waitFor(`[...document.querySelectorAll('[data-active]')].some(e => e.innerText.includes('数据透视表'))`, { what: '搜索结果含「数据透视表」' })
      await page.press('Enter')
      await page.waitFor(`document.querySelector('.side-panel .panel-title')?.innerText.includes('数据透视表字段')`, { what: '透视表字段面板' })
    })
  })

  it('命令面板：拼音首字母搜索（tsb → 数据透视表、ycfx → 异常发现），不影响英文与中文搜索', async () => {
    await withPage('command-palette-pinyin', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.press('q', { alt: true, code: 'KeyQ' })
      await page.waitFor(`!!document.querySelector('input[placeholder*="搜索功能"]')`, { what: '命令面板打开' })
      const results = () => page.eval(`[...document.querySelectorAll('[data-active]')].map(e => e.innerText)`)
      await page.fill('input[placeholder*="搜索功能"]', 'tsb')
      await page.waitFor(`[...document.querySelectorAll('[data-active]')].some(e => e.innerText.includes('数据透视表'))`, { what: 'tsb 搜到数据透视表' })
      await page.fill('input[placeholder*="搜索功能"]', 'ycfx')
      await page.waitFor(`[...document.querySelectorAll('[data-active]')].some(e => e.innerText.includes('异常发现'))`, { what: 'ycfx 搜到异常发现' })
      await page.fill('input[placeholder*="搜索功能"]', 'pivot')
      await page.waitFor(`[...document.querySelectorAll('[data-active]')].some(e => e.innerText.includes('数据透视表'))`, { what: '英文同义词仍可搜' })
      await page.fill('input[placeholder*="搜索功能"]', 'zzzzqq')
      await page.waitFor(`document.querySelectorAll('[data-active]').length === 0`, { what: '无匹配时没有结果' })
      assert.deepEqual(await results(), [])
    })
  })

  it('公式诊断：循环引用列出环路并能定位到格子；断开后重新扫描消失', async () => {
    await withPage('formula-cycles', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      // D5 → E5 → D5；F5 引用自己所在的整列求和（F:F 包含 F5）
      await page.eval(`(() => { const s = window.__univerAPI.getActiveWorkbook().getActiveSheet(); s.getRange('D5').setFormula('=E5+1'); s.getRange('E5').setFormula('=D5+1'); s.getRange('F5').setFormula('=SUM(F:F)') })()`)
      await page.press('q', { alt: true, code: 'KeyQ' })
      await page.waitFor(`!!document.querySelector('input[placeholder*="搜索功能"]')`, { what: '命令面板' })
      await page.fill('input[placeholder*="搜索功能"]', '公式诊断')
      await page.waitFor(`[...document.querySelectorAll('[data-active]')].some(e => e.innerText.includes('公式诊断'))`, { what: '搜到公式诊断' })
      await page.press('Enter')
      await page.waitFor(`!!document.querySelector('.ec-dialog')`, { what: '诊断对话框' })
      // 有循环引用时直接停在「循环引用」页签
      await page.waitFor(`document.querySelector('.ec-radio-button.is-active')?.innerText.includes('循环引用（2）')`, { what: '循环引用页签有 2 处' })
      const text = await page.eval(`document.querySelector('.ec-dialog').innerText.replace(/\\s+/g, ' ')`)
      assert.match(text, /D5 → E5 → D5/, '两个格子互相引用的环路')
      assert.match(text, /F5 → F5/, '整列求和包含自己')
      // 点环路里的地址：选中那个格子
      await page.click('.ec-dialog button.addr', 'E5')
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getActiveRange()?.getA1Notation() === 'E5'`, { what: '定位到 E5' })
      // 改掉：E5 不再引用 D5，F5 不再引用整列 → 重新扫描后没有循环引用
      await page.eval(`(() => { const s = window.__univerAPI.getActiveWorkbook().getActiveSheet(); s.getRange('E5').setValue(1); s.getRange('F5').setFormula('=SUM(F1:F4)') })()`)
      await page.click('.ec-dialog button', '重新扫描')
      await page.waitFor(`document.querySelector('.ec-radio-button.is-active')?.innerText.includes('循环引用（0）') || [...document.querySelectorAll('.ec-radio-button')].some(e => e.innerText.includes('循环引用（0）'))`, { what: '重新扫描后循环引用为 0' })
    })
  })

  it('导入 xlsx：条件格式里的图标集（含反向、不显示数值）进入表格，不再提示「未能导入」', async () => {
    const ExcelJS = (await import('exceljs')).default
    const wbx = new ExcelJS.Workbook()
    const ws = wbx.addWorksheet('图标集')
    for (let i = 1; i <= 10; i++) {
      ws.getCell(`A${i}`).value = i * 10
      ws.getCell(`B${i}`).value = 100 - i * 10
    }
    ws.addConditionalFormatting({ ref: 'A1:A10', rules: [{ type: 'iconSet', iconSet: '3Arrows', priority: 1, showValue: false, cfvo: [{ type: 'percent', value: 0 }, { type: 'percent', value: 33 }, { type: 'percent', value: 67 }] }] })
    ws.addConditionalFormatting({ ref: 'B1:B10', rules: [{ type: 'iconSet', iconSet: '4TrafficLights', priority: 2, reverse: true, cfvo: [{ type: 'percent', value: 0 }, { type: 'num', value: 25 }, { type: 'num', value: 50 }, { type: 'num', value: 75 }] }] })
    const base64 = Buffer.from(await wbx.xlsx.writeBuffer()).toString('base64')
    await withPage('xlsx-icon-set', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.eval(`(() => { const bin = atob(${JSON.stringify(base64)}); const bytes = Uint8Array.from(bin, c => c.charCodeAt(0)); const dt = new DataTransfer(); dt.items.add(new File([bytes], '图标集.xlsx')); const input = document.querySelector('input[type=file][accept*=xlsx]'); input.files = dt.files; input.dispatchEvent(new Event('change', { bubbles: true })) })()`)
      // 导入后若弹出覆盖确认，选择继续
      await page.waitFor(`[...document.querySelectorAll('.ec-message-box')].some(d => d.offsetParent && (d.innerText.includes('条件格式') || d.innerText.includes('覆盖') || d.innerText.includes('替换')))`, { what: '导入结果 / 确认框', timeout: 30_000 })
      const first = await page.eval(`[...document.querySelectorAll('.ec-message-box')].find(d => d.offsetParent).innerText`)
      if (!first.includes('条件格式')) {
        await page.click('.ec-message-box button', '确定').catch(async () => page.click('.ec-message-box button', '继续'))
        await page.waitFor(`[...document.querySelectorAll('.ec-message-box')].some(d => d.offsetParent && d.innerText.includes('条件格式'))`, { what: '导入结果', timeout: 30_000 })
      }
      const box = await page.eval(`[...document.querySelectorAll('.ec-message-box')].find(d => d.offsetParent && d.innerText.includes('条件格式')).innerText`)
      assert.match(box, /条件格式 2 条/, '两条图标集都导入了')
      assert.ok(!box.includes('未能导入'), `不应有「未能导入」：${box}`)
      const rules = await page.eval(`(() => { const sheet = window.__univerAPI.getActiveWorkbook().getSheets().find(s => s.getSheetName() === '图标集'); return sheet.getConditionalFormattingRules().map(r => ({ type: r.rule.type, show: r.rule.isShowValue, icons: r.rule.config.map(c => c.iconType + ':' + c.iconId), range: r.ranges[0].startColumn })) })()`)
      assert.equal(rules.length, 2)
      const byCol = Object.fromEntries(rules.map(r => [r.range, r]))
      assert.equal(byCol[0].type, 'iconSet')
      assert.equal(byCol[0].show, false, 'showValue=0 → 不显示数值')
      assert.deepEqual(byCol[0].icons, ['3Arrows:0', '3Arrows:1', '3Arrows:2'])
      assert.deepEqual(byCol[1].icons, ['4TrafficLights:3', '4TrafficLights:2', '4TrafficLights:1', '4TrafficLights:0'], '反向：最高档用最后一个图标')
    })
  })

  it('表格（超级表）：创建后结构化引用可算，保存后重新打开仍在', async () => {
    const id = (await api.createWorkbook(TOKEN.alice, '表格测试', [['订单号', '金额'], ['A1', 100], ['A2', 250.5], ['A3', 49.5]])).id
    await withPage('sheet-table', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      const made = await page.eval(`(async () => {
        const wb = window.__univerAPI.getActiveWorkbook()
        const sheet = wb.getActiveSheet()
        const ok = await sheet.addTable('订单表', { startRow: 0, startColumn: 0, endRow: 3, endColumn: 1 }, 'tbl_orders')
        sheet.getRange('D1').setFormula('=SUM(订单表[金额])')
        return { ok, tables: sheet.getSubTableInfos().map(t => ({ name: t.name, range: t.range })) }
      })()`)
      assert.equal(made.ok, true)
      assert.deepEqual(made.tables, [{ name: '订单表', range: { startRow: 0, startColumn: 0, endRow: 3, endColumn: 1 } }])
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('D1').getValue() === 400`, { what: '结构化引用 =SUM(订单表[金额]) 算出 400' })
      await page.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('未保存')`, { what: '创建表格算一次修改' })
      await page.press('s', { ctrl: true, code: 'KeyS' })
      await page.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { what: '保存' })
    })
    await withPage('sheet-table-reload', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      const tables = await page.eval(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getSubTableInfos().map(t => ({ name: t.name, range: t.range }))`)
      assert.deepEqual(tables, [{ name: '订单表', range: { startRow: 0, startColumn: 0, endRow: 3, endColumn: 1 } }], '重新打开后表格还在')
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('D1').getValue() === 400`, { what: '重新打开后结构化引用仍然算对' })
    })
  })

  it('导入 xlsx：Excel 的表格（含汇总行的）进入表格功能，汇总行提示未带入', async () => {
    const ExcelJS = (await import('exceljs')).default
    const wbx = new ExcelJS.Workbook()
    const ws = wbx.addWorksheet('销售')
    ws.addTable({ name: '销售表', ref: 'B2', headerRow: true, totalsRow: true, style: { theme: 'TableStyleMedium2', showRowStripes: true }, columns: [{ name: '店铺', totalsRowLabel: '合计', filterButton: true }, { name: '金额', totalsRowFunction: 'sum', filterButton: true }], rows: [['A', 10], ['B', 20], ['C', 30]] })
    const base64 = Buffer.from(await wbx.xlsx.writeBuffer()).toString('base64')
    await withPage('xlsx-table', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.eval(`(() => { const bin = atob(${JSON.stringify(base64)}); const bytes = Uint8Array.from(bin, c => c.charCodeAt(0)); const dt = new DataTransfer(); dt.items.add(new File([bytes], '销售.xlsx')); const input = document.querySelector('input[type=file][accept*=xlsx]'); input.files = dt.files; input.dispatchEvent(new Event('change', { bubbles: true })) })()`)
      await page.waitFor(`[...document.querySelectorAll('.ec-message-box')].some(d => d.offsetParent && d.innerText.includes('同时导入'))`, { what: '导入结果', timeout: 30_000 })
      const box = await page.eval(`[...document.querySelectorAll('.ec-message-box')].find(d => d.offsetParent && d.innerText.includes('同时导入')).innerText`)
      assert.match(box, /表格 1 个/)
      assert.match(box, /销售表.*汇总行未带入/, '汇总行没有带入要说明')
      const tables = await page.eval(`(() => { const sheet = window.__univerAPI.getActiveWorkbook().getSheets().find(s => s.getSheetName() === '销售'); return sheet.getSubTableInfos().map(t => ({ name: t.name, range: t.range })) })()`)
      assert.deepEqual(tables, [{ name: '销售表', range: { startRow: 1, startColumn: 1, endRow: 4, endColumn: 2 } }], '汇总行之前的 3 行数据 + 标题行是表格范围')
    })
  })

  it('铺进单元格的数据表「保留手工调整」：手改的格子按主键认行，数据变化、重新排序后仍保留；没有勾选时被覆盖', async () => {
    const made = await api.call('POST', '/ledger/datasets/input', TOKEN.alice, { body: { name: 'UI手改明细', columns: [{ title: '订单号', kind: 'text' }, { title: '店铺', kind: 'text' }, { title: '金额', kind: 'money' }], primaryKey: ['订单号'] } })
    const dsId = made.data
    const upsert = rows => api.call('POST', `/ledger/datasets/${dsId}/writeback`, TOKEN.alice, { body: { mode: 'upsert', columns: ['订单号', '店铺', '金额'], rows } })
    await upsert([['A1', '甲', 100], ['A2', '乙', 200], ['A3', '甲', 300]])
    const spec = (id, keepEdits) => ({ v: 1, id, kind: 'datatable', name: `明细表${id}`, datasetId: dsId, fields: [], filters: [], sort: [{ column: '金额', desc: true }], followSlicers: false, row: 0, col: 0, keepEdits })
    // 同一个工作簿里两张表（不同工作表）：一张保留手工调整，一张不保留
    const json = { id: 'wb', name: '手改', sheetOrder: ['s1', 's2'], sheets: {
      s1: { id: 's1', name: '保留', cellData: {}, rowCount: 100, columnCount: 20, custom: { celltables: [spec('keep', true)] } },
      s2: { id: 's2', name: '不保留', cellData: {}, rowCount: 100, columnCount: 20, custom: { celltables: [spec('plain', false)] } },
    } }
    const zlib = await import('node:zlib')
    const created = await api.call('POST', `/ledger/workbooks?name=${encodeURIComponent('手改')}`, TOKEN.alice, { raw: zlib.gzipSync(Buffer.from(JSON.stringify(json))) })
    const id = created.data.id
    const grid = (page, sheet) => page.eval(`(() => { const s = window.__univerAPI.getActiveWorkbook().getSheetByName(${JSON.stringify(sheet)}); return s.getRange(0, 0, 6, 3).getValues().map(r => r.join('|').replace(/,/g, '').replace(/\\.00(?=\\||$)/g, '')).filter(r => r !== '||') })()`)
    await withPage('celltable-edits', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      for (const sheet of ['保留', '不保留'])
        await page.waitFor(`window.__univerAPI.getActiveWorkbook().getSheetByName(${JSON.stringify(sheet)}).getRange(1, 0).getValue() === 'A3'`, { what: `「${sheet}」表按金额降序写入`, timeout: 30_000 })
      assert.deepEqual(await grid(page, '保留'), ['订单号|店铺|金额', 'A3|甲|300', 'A2|乙|200', 'A1|甲|100'])
      // 表格自己的写入不算手工调整
      assert.equal(await page.eval(`window.__univerAPI.getActiveWorkbook().getSheetByName('保留').getCustomMetadata().celltables[0].edits ?? null`), null)
      // 用户手改：A2 的金额改成 999、A1 的店铺改成「手改店」；两张表各改一遍
      for (const sheet of ['保留', '不保留'])
        await page.eval(`(() => { const s = window.__univerAPI.getActiveWorkbook().getSheetByName(${JSON.stringify(sheet)}); s.getRange(2, 2).setValue(999); s.getRange(3, 1).setValue('手改店') })()`)
      const edits = await page.eval(`JSON.stringify(window.__univerAPI.getActiveWorkbook().getSheetByName('保留').getCustomMetadata().celltables[0].edits)`)
      assert.deepEqual(JSON.parse(edits), { '["A2"]': { 金额: 999 }, '["A1"]': { 店铺: '手改店' } }, '按主键记录手改')
      assert.equal(await page.eval(`window.__univerAPI.getActiveWorkbook().getSheetByName('不保留').getCustomMetadata().celltables[0].edits ?? null`), null, '没勾选的表不记录')
      // 在面板里点「立即刷新」：表格自己的写入不会被当成手工调整；手改仍在表里，面板显示「保留手工调整」已勾选和处数
      await page.press('q', { alt: true, code: 'KeyQ' })
      await page.waitFor(`!!document.querySelector('input[placeholder*="搜索功能"]')`, { what: '命令面板' })
      await page.fill('input[placeholder*="搜索功能"]', '仪表盘')
      await page.waitFor(`[...document.querySelectorAll('[data-active]')].some(e => e.innerText.includes('仪表盘'))`, { what: '搜到仪表盘' })
      await page.press('Enter')
      await page.waitFor(`[...document.querySelectorAll('.side-panel .part')].some(e => e.innerText.includes('明细表keep'))`, { what: '仪表盘面板列出这张表' })
      await page.click('.side-panel .part button', '编辑')
      await page.waitFor(`document.querySelector('.side-panel')?.innerText.includes('保留手工调整') && document.querySelector('.side-panel').innerText.includes('现有 2 处')`, { what: '面板显示保留手工调整与处数' })
      await page.click('.side-panel button', '立即刷新')
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getSheetByName('保留').getRange(2, 2).getValue().toString().startsWith('999')`, { what: '刷新后手改仍在' })
      await new Promise(r => setTimeout(r, 500))
      const after = await page.eval(`JSON.stringify(window.__univerAPI.getActiveWorkbook().getSheetByName('保留').getCustomMetadata().celltables[0].edits)`)
      assert.deepEqual(JSON.parse(after), { '["A2"]': { 金额: 999 }, '["A1"]': { 店铺: '手改店' } }, '刷新时表格自己的写入没有被记成手工调整')
      await page.press('s', { ctrl: true, code: 'KeyS' })
      await page.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { what: '保存' })
    })
    // 数据集变化：A1 金额改 111、新增 A4（444）；重新打开后表格刷新
    await upsert([['A1', '甲', 111], ['A4', '丙', 444]])
    await withPage('celltable-edits-reload', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getSheetByName('不保留').getRange(1, 0).getValue() === 'A4'`, { what: '数据集更新后刷新', timeout: 30_000 })
      // 保留：A2 的 999 排第一（排序按手改后的值？不——排序在服务端按数据集的值，所以位置按 A4 444、A3 300、A2 200、A1 111，A2 的格子里显示 999）
      assert.deepEqual(await grid(page, '保留'), ['订单号|店铺|金额', 'A4|丙|444', 'A3|甲|300', 'A2|乙|999', 'A1|手改店|111'])
      assert.deepEqual(await grid(page, '不保留'), ['订单号|店铺|金额', 'A4|丙|444', 'A3|甲|300', 'A2|乙|200', 'A1|甲|111'], '没勾选：手改被覆盖')
    })
  })

  it('导出 xlsx：单元格模式的数据表在原位置包成 Excel 表格，引用它的 TSUM / TCOUNT 转成 SUM(表[列]) 等；有手工调整的表仍导出数值并写明原因', async () => {
    const made = await api.call('POST', '/ledger/datasets/input', TOKEN.alice, { body: { name: 'UI导出明细', columns: [{ title: '订单号', kind: 'text' }, { title: '店铺', kind: 'text' }, { title: '金额', kind: 'money' }], primaryKey: ['订单号'] } })
    const dsId = made.data
    await api.call('POST', `/ledger/datasets/${dsId}/writeback`, TOKEN.alice, { body: { mode: 'upsert', columns: ['订单号', '店铺', '金额'], rows: [['A1', '甲', 100], ['A2', '乙', 200], ['A3', '甲', 300.5]] } })
    const spec = (id, name, extra = {}) => ({ v: 1, id, kind: 'datatable', name, datasetId: dsId, fields: [], filters: [], sort: [{ column: '金额', desc: true }], followSlicers: false, row: 1, col: 1, ...extra })
    const f = text => ({ f: text })
    const json = { id: 'wb', name: '导出单元格表', sheetOrder: ['s1', 's2', 's3'], sheets: {
      s1: { id: 's1', name: '明细', cellData: {}, rowCount: 100, columnCount: 20, custom: { celltables: [spec('t1', '订单格')] } },
      s2: { id: 's2', name: '手改', cellData: {}, rowCount: 100, columnCount: 20, custom: { celltables: [spec('t2', '手改格', { keepEdits: true, edits: { '["A1"]': { 金额: 5 } } })] } },
      s3: { id: 's3', name: '汇总', rowCount: 100, columnCount: 20, cellData: { 0: { 0: f('=TSUM("订单格[金额]")') }, 1: { 0: f('=TCOUNT("订单格","店铺","甲")') }, 2: { 0: f('=TSUM("订单格[金额]","店铺","甲")') }, 3: { 0: f('=TSUM("手改格[金额]")') } } },
    } }
    const zlib = await import('node:zlib')
    const created = await api.call('POST', `/ledger/workbooks?name=${encodeURIComponent('导出单元格表')}`, TOKEN.alice, { raw: zlib.gzipSync(Buffer.from(JSON.stringify(json))) })
    await withPage('celltable-export', 'alice', `/excel?wb=${created.data.id}`, async (page) => {
      await editorReady(page)
      const wbk = `window.__univerAPI.getActiveWorkbook()`
      for (const s of ['明细', '手改'])
        await page.waitFor(`${wbk}.getSheetByName('${s}').getRange(2, 1).getValue() === 'A3'`, { what: `「${s}」表写入单元格`, timeout: 30_000 })
      await page.waitFor(`Number(String(${wbk}.getSheetByName('汇总').getRange(0, 0).getValue()).replace(/,/g, '')) === 600.5`, { what: 'TSUM 算出 600.5', timeout: 30_000 })
      // 用页面里同一份模块导出（与菜单「导出 .xlsx」走同一个函数），取回文件
      const out = await page.eval(`(async () => {
        const m = await import('/src/features/excel/xlsx/export-xlsx.ts')
        const { blob, summary } = await m.buildWorkbookXlsx(window.__univerAPI, 't.xlsx')
        const bytes = new Uint8Array(await blob.arrayBuffer())
        let bin = ''
        for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
        return { b64: btoa(bin), summary }
      })()`)
      assert.equal(out.summary.cellTables, 1, '一个单元格模式的表包成了表格（有手改的那个没有）')
      assert.equal(out.summary.tableFormulas, 3)
      assert.equal(out.summary.tableFormulasAsValue, 1)
      const ExcelJS = (await import('exceljs')).default
      const x = new ExcelJS.Workbook()
      await x.xlsx.load(Buffer.from(out.b64, 'base64'))
      const detail = x.getWorksheet('明细')
      const tables = Object.values(detail.tables ?? {}).map(t => t.table ?? t.model ?? t)
      assert.equal(tables.length, 1, '「明细」页签上有一个 Excel 表格')
      assert.equal(tables[0].name, '订单格')
      assert.equal(tables[0].tableRef ?? tables[0].ref, 'B2:D5', '表格就在原位置（表头 + 3 行）')
      assert.equal(x.getWorksheet('订单格'), undefined, '没有另外生成数据页签')
      const sum = x.getWorksheet('汇总')
      const cell = r => sum.getCell(r, 1).value
      assert.equal(cell(1).formula, 'SUM(订单格[金额])')
      assert.equal(cell(2).formula, 'COUNTIFS(订单格[店铺], "甲")')
      assert.equal(cell(3).formula, 'SUMIFS(订单格[金额], 订单格[店铺], "甲")')
      // 用表格里的格子独立算一遍，与缓存的结果（平台算的）一致
      const amounts = [3, 4, 5].map(r => Number(detail.getCell(r, 4).value))
      const shops = [3, 4, 5].map(r => detail.getCell(r, 3).value)
      assert.equal(cell(1).result, amounts.reduce((a, b) => a + b, 0))
      assert.equal(cell(2).result, shops.filter(s => s === '甲').length)
      assert.equal(cell(3).result, amounts.filter((_, i) => shops[i] === '甲').reduce((a, b) => a + b, 0))
      // 有手工调整：导出数值，批注写明原因
      assert.equal(typeof cell(4), 'number')
      const note = sum.getCell(4, 1).note
      assert.match(typeof note === 'string' ? note : JSON.stringify(note), /手工调整/)
      assert.equal(Object.keys(x.getWorksheet('手改').tables ?? {}).length, 0, '有手改的表不包成表格')
    })
  })

  it('图表联动：真实鼠标点柱子联动其他图表；按系列联动；Ctrl 多选；随工作簿保存，重新打开仍生效', async () => {
    const csv = ['下单时间,店铺,渠道,金额', '2026-01-05 10:00:00,A,线上,10', '2026-01-06 10:00:00,A,线下,20', '2026-01-07 10:00:00,B,线上,30', '2026-02-05 10:00:00,A,线上,40', '2026-02-06 10:00:00,B,线下,50', '2026-03-05 10:00:00,B,线上,60', '2026-03-06 10:00:00,A,线下,70'].join('\n')
    const dsId = await api.importDataset(TOKEN.alice, 'UI联动明细', csv)
    const id = (await api.createWorkbook(TOKEN.alice, '联动测试')).id
    const sum = [{ column: '金额', agg: 'sum' }]
    const specA = { v: 1, id: 'chA', type: 'column', title: '店铺×渠道', stacked: true, source: { kind: 'dataset', datasetId: dsId, x: { column: '店铺' }, values: sum, splitBy: '渠道' } }
    const specB = { v: 1, id: 'chB', type: 'column', title: '按月金额', source: { kind: 'dataset', datasetId: dsId, x: { column: '下单时间', granularity: 'month' }, values: sum } }
    // 图表里柱子的屏幕坐标：按系列累计值的中点换算（堆叠柱）
    const barAt = (page, chartId, category, series) => page.eval(`(() => {
      const chart = window.__webexcel.chart(${JSON.stringify(chartId)})
      const opt = chart.getOption()
      const cats = opt.xAxis[0].data
      const ci = cats.indexOf(${JSON.stringify(category)})
      let acc = 0
      for (let i = 0; i < opt.series.length; i++) {
        const v = Number((opt.series[i].data[ci]?.value ?? opt.series[i].data[ci]) || 0)
        if (opt.series[i].name === ${JSON.stringify(series ?? null)} || ${JSON.stringify(series ?? null)} === null) {
          const [px, py] = chart.convertToPixel({ seriesIndex: i }, [ci, acc + v / 2])
          const r = chart.getDom().getBoundingClientRect()
          return [r.left + px, r.top + py]
        }
        acc += v
      }
      return null
    })()`)
    const chartData = (page, chartId) => page.eval(`(() => { const o = window.__webexcel.chart(${JSON.stringify(chartId)}).getOption(); return { cats: o.xAxis[0].data, values: o.series.map(s => s.data.map(d => Number(d?.value ?? d))) } })()`)
    const stored = page => page.eval(`JSON.stringify(window.__univerAPI.getActiveWorkbook().getActiveSheet().getCustomMetadata()?.crossfilter ?? null)`).then(JSON.parse)
    await withPage('chart-link', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      await page.eval(`void (window.__webexcel.insertChart(${JSON.stringify(specA)}), window.__webexcel.insertChart(${JSON.stringify(specB)}))`)
      await page.waitFor(`!!window.__webexcel.chart('chA')?.getOption().series?.length && !!window.__webexcel.chart('chB')?.getOption().series?.length`, { what: '两张图表画好', timeout: 30_000 })
      assert.deepEqual((await chartData(page, 'chB')).cats, ['2026-01', '2026-02', '2026-03'], '联动前：三个月')
      // 等柱子长完（图表有入场动画，动画没结束时点不中）
      await new Promise(r => setTimeout(r, 2000))
      // 点「店铺 A」的「线下」柱：按分类 + 系列联动，另一张图只剩 A 店线下的 1 月（20）与 3 月（70）
      const [x1, y1] = await barAt(page, 'chA', 'A', '线下')
      await page.mouseClick(x1, y1)
      await page.waitFor(`document.body.innerText.includes('已筛选 店铺：A · 渠道：线下')`, { what: '图表上显示「已筛选 店铺：A · 渠道：线下」' })
      await page.waitFor(`JSON.stringify(window.__webexcel.chart('chB').getOption().series[0].data.map(d => Number(d?.value ?? d))) === '[20,70]'`, { what: '另一张图只剩 A 店线下的 1 月（20）、3 月（70）' })
      const saved = await stored(page)
      assert.deepEqual([saved.values, saved.series, saved.chartId], [['A'], { column: '渠道', values: ['线下'] }, 'chA'], '联动写进了工作表元数据')
      // ⌘（Meta）+ 点「店铺 B」的「线下」柱（Mac 上 Ctrl + 点击是右键，所以用 ⌘；Windows 上 Ctrl 同样可以）：分类增加，系列不变 → B 店线下 2 月（50）也进来
      await new Promise(r => setTimeout(r, 1500))
      const [x2, y2] = await barAt(page, 'chA', 'B', '线下')
      await page.mouseClick(x2, y2, 4)
      await page.waitFor(`document.body.innerText.includes('已筛选 店铺：A、B · 渠道：线下')`, { what: '多选后显示 A、B' })
      await page.waitFor(`JSON.stringify(window.__webexcel.chart('chB').getOption().series[0].data.map(d => Number(d?.value ?? d))) === '[20,50,70]'`, { what: '另一张图是 A、B 两店线下的 20、50、70' })
      await page.press('s', { ctrl: true, code: 'KeyS' })
      await page.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { what: '保存' })
    }, { allowErrors: [UNIVER_REMOVED_SHEET_RACE] })
    // 重新打开：联动仍然生效
    await withPage('chart-link-reload', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`JSON.stringify(window.__webexcel.chart('chB')?.getOption().series?.[0]?.data.map(d => Number(d?.value ?? d))) === '[20,50,70]'`, { what: '重新打开后联动仍生效', timeout: 30_000 })
      assert.ok(await page.eval(`document.body.innerText.includes('已筛选 店铺：A、B · 渠道：线下')`), '发起联动的图表上仍显示筛选条件')
      // 点「×」取消：联动消失，元数据里也没有了
      await page.click('.pick .x')
      await page.waitFor(`JSON.stringify(window.__webexcel.chart('chB').getOption().series[0].data.map(d => Number(d?.value ?? d))) === '[60,90,130]' && !document.body.innerText.includes('已筛选')`, { what: '取消联动，恢复全量' })
      assert.equal(await stored(page), null, '取消后工作表元数据里的联动也清掉')
    }, { allowErrors: [UNIVER_REMOVED_SHEET_RACE] })
  })

  it('看板部件保存为图片：指标卡、切片器、表格卡片、图表各生成一张 PNG', async () => {
    const dsId = await api.importDataset(TOKEN.alice, 'UI导图明细', '店铺,金额\nA,10\nA,20\nB,30\n')
    const id = (await api.createWorkbook(TOKEN.alice, '导图测试')).id
    const sum = [{ column: '金额', agg: 'sum' }]
    const widgets = [
      ['webexcel-kpi', { v: 1, id: 'k1', kind: 'kpi', title: '总金额', datasetId: dsId, value: { column: '金额', agg: 'sum' }, granularity: 'month', compare: 'none', upIsGood: true, sparkline: false }, '.kpi'],
      ['webexcel-slicer', { v: 1, id: 's1', kind: 'slicer', title: '店铺', datasetId: dsId, column: '店铺', mode: 'values', values: [] }, '.slicer'],
      ['webexcel-table', { v: 1, id: 't1', kind: 'table', title: '各店铺金额', datasetId: dsId, row: { column: '店铺' }, values: sum, top: 0, sort: 'value' }, '.card'],
    ]
    await withPage('widget-images', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      for (const [key, spec] of widgets)
        await page.eval(`void window.__webexcel.insertFloat(${JSON.stringify(key)}, ${JSON.stringify(spec)}, { width: 260, height: 180 })`)
      await page.eval(`void window.__webexcel.insertChart(${JSON.stringify({ v: 1, id: 'c1', type: 'column', title: '图表 / 导出', source: { kind: 'dataset', datasetId: dsId, x: { column: '店铺' }, values: sum } })})`)
      await page.waitFor(`!!document.querySelector('.kpi .value') && !document.querySelector('.kpi .value').innerText.includes('…') && !!window.__webexcel.chart('c1')?.getOption().series?.length`, { what: '部件都画好了', timeout: 30_000 })
      // 截获下载：记下生成的图片
      await page.eval(`(() => { window.__imgs = []; const orig = URL.createObjectURL.bind(URL); URL.createObjectURL = (b) => { window.__imgs.push(b); return orig(b) }; HTMLAnchorElement.prototype.click = function () { window.__names = [...(window.__names ?? []), this.download] } })()`)
      for (const sel of ['.kpi .edit[title="保存为图片"]', '.slicer .edit[title="保存为图片"]', '.card .edit[title="保存为图片"]'])
        await page.eval(`document.querySelector(${JSON.stringify(sel)}).click()`)
      await page.eval(`[...document.querySelectorAll('.chart-root .toolbar button')].find(b => b.innerText.includes('图片')).click()`)
      await page.waitFor(`window.__imgs.length === 4`, { what: '生成 4 张图片', timeout: 30_000 })
      const info = await page.eval(`(async () => Promise.all(window.__imgs.map(async b => { const bmp = await createImageBitmap(b); return { type: b.type, size: b.size, w: bmp.width, h: bmp.height } })))()`)
      for (const i of info) {
        assert.equal(i.type, 'image/png')
        assert.ok(i.size > 1500 && i.w >= 200 && i.h >= 100, `图片应有内容：${JSON.stringify(i)}`)
      }
      assert.deepEqual((await page.eval(`window.__names`)).sort(), ['总金额.png', '店铺.png', '各店铺金额.png', '图表_导出.png'].sort(), '文件名取自部件标题')
    })
  })

  it('顶栏面包屑：显示真实位置（首页 › 文件夹 › 工作簿名），根目录的工作簿不带文件夹，不再出现写死的占位文字', async () => {
    const parent = (await api.call('POST', '/ledger/folders', TOKEN.alice, { body: { name: '财务部', parentId: null } })).data
    const child = (await api.call('POST', '/ledger/folders', TOKEN.alice, { body: { name: '2026 月报', parentId: parent.id } })).data
    const inFolder = (await api.createWorkbook(TOKEN.alice, '十月利润表')).id
    const moved = await api.call('POST', '/ledger/workbooks/move', TOKEN.alice, { body: { ids: [inFolder], folderId: child.id } })
    assert.equal(moved.status, 200)
    const crumb = page => page.eval(`document.querySelector('.breadcrumb').innerText.replace(/\\s+/g, ' ').trim()`)
    await withPage('crumb-folder', 'alice', `/excel?wb=${inFolder}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`document.querySelector('.breadcrumb')?.innerText.includes('2026 月报')`, { what: '文件夹链出现' })
      assert.equal(await crumb(page), '首页 › 财务部 › 2026 月报 › 十月利润表 ▾')
    })
    await withPage('crumb-root', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      assert.equal(await crumb(page), '首页 › Alice 利润表 ▾')
    })
  })

  it('评论与 @提醒：Alice 在格子上 @Bob → Bob 的铃铛有未读 → 点通知定位到格子 → 回复 → Alice 收到回复通知 → 标记已解决', async () => {
    const id = (await api.createWorkbook(TOKEN.alice, '评论协作')).id
    await api.shareWorkbook(TOKEN.alice, id, 'view')
    const openComments = async (page) => {
      await page.press('q', { alt: true, code: 'KeyQ' })
      await page.waitFor(`!!document.querySelector('input[placeholder*="搜索功能"]')`, { what: '命令面板' })
      await page.fill('input[placeholder*="搜索功能"]', '评论')
      await page.waitFor(`[...document.querySelectorAll('[data-active]')].some(e => e.innerText.includes('评论'))`, { what: '搜到评论' })
      await page.press('Enter')
      await page.waitFor(`document.querySelector('.side-panel .panel-title')?.innerText.includes('评论')`, { what: '评论面板' })
    }
    // Alice：选中 B2，写评论并 @Bob（输入 @ 后选人）
    await withPage('comment-alice', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      await page.eval(`void window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('B2').activate()`)
      await openComments(page)
      await page.waitFor(`document.querySelector('.comments')?.innerText.includes('B2')`, { what: '面板显示选中的 B2' })
      await page.fill('.comments textarea', '请 @Bo')
      await page.waitFor(`[...document.querySelectorAll('.comments .menu-item')].some(e => e.innerText.includes('Bob'))`, { what: '@ 候选里有 Bob' })
      await page.click('.comments .menu-item', 'Bob')
      await page.waitFor(`document.querySelector('.comments textarea').value === '请 @Bob '`, { what: '插入了「@Bob 」' })
      await page.fill('.comments textarea', '请 @Bob 核对这个金额')
      await page.click('.comments button', '评论')
      await page.waitFor(`document.querySelectorAll('.comments [data-thread]').length === 1`, { what: '出现一条线程' })
      assert.match(await page.eval(`document.querySelector('.comments [data-thread]').innerText`), /Alice[\s\S]*请 @Bob 核对这个金额/)
      assert.ok(await page.eval(`!!document.querySelector('.comments .at')`), '@Bob 高亮')
    })
    // Bob：铃铛未读 1 → 点开通知 → 定位到 B2，面板里是这条评论
    await withPage('comment-bob', 'bob', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`document.querySelector('[data-unread]')?.innerText === '1'`, { what: '铃铛显示未读 1' })
      await page.click('[data-bell]')
      await page.waitFor(`document.querySelectorAll('[data-notification]').length === 1`, { what: '通知列表有一条' })
      assert.match(await page.eval(`document.querySelector('[data-notification]').innerText`), /Alice[\s\S]*@ 了你[\s\S]*请 @Bob 核对这个金额[\s\S]*评论协作 · B2/)
      await page.click('[data-notification]')
      await page.waitFor(`document.querySelectorAll('.comments [data-thread]').length === 1`, { what: '面板里是这条线程' })
      assert.equal(await page.eval(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getActiveRange().getA1Notation()`), 'B2', '定位到 B2')
      await page.waitFor(`!document.querySelector('[data-unread]')`, { what: '已读后角标消失' })
      // 回复
      await page.click('.comments .acts button', '回复')
      await page.fill('.reply-box textarea', '已核对，没问题')
      await page.click('.reply-box button', '回复')
      await page.waitFor(`document.querySelectorAll('.comments [data-thread] .cm.reply').length === 1`, { what: '出现一条回复' })
    })
    // Alice：铃铛收到「回复了你的评论」；标记已解决
    await withPage('comment-alice-reply', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`document.querySelector('[data-unread]')?.innerText === '1'`, { what: 'Alice 铃铛未读 1' })
      await page.click('[data-bell]')
      await page.waitFor(`document.querySelector('[data-notification]')?.innerText.includes('回复了你的评论')`, { what: '回复通知' })
      await page.click('[data-notification]')
      await page.waitFor(`document.querySelectorAll('.comments [data-thread]').length === 1`, { what: '面板里是这条线程' })
      await page.click('.comments .acts button', '标记已解决')
      await page.waitFor(`document.querySelector('.comments [data-thread] .tag')?.innerText.includes('已解决')`, { what: '线程标为已解决' })
      const threads = (await api.call('GET', `/ledger/workbooks/${id}/comments`, TOKEN.alice)).data
      assert.equal(threads[0].resolved, true)
      assert.equal(threads[0].replies.length, 1)
    })
  })

  it('实时协作：两个用户同时打开同一个工作簿——互相看到修改、同格冲突收敛、行插入同步、光标、各自保存不冲突，晚加入的人补齐未保存的修改', async () => {
    const id = (await api.createWorkbook(TOKEN.alice, '协作测试')).id
    await api.shareWorkbook(TOKEN.alice, id, 'edit')
    const pages = []
    const open = async (user) => {
      const page = await browser.newPage({ isolated: true })
      pages.push(page)
      await page.setCookie(COOKIE, TOKEN[user], WEB)
      await page.goto(`${WEB}/excel?wb=${id}`)
      await editorReady(page)
      return page
    }
    const cell = (page, a1) => page.eval(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(${JSON.stringify(a1)}).getValue()`)
    const set = (page, a1, v) => page.eval(`void window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(${JSON.stringify(a1)}).setValue(${JSON.stringify(v)})`)
    const saveText = page => page.eval(`document.querySelector('.status-indicator')?.innerText ?? ''`)
    try {
      const alice = await open('alice')
      const bob = await open('bob')
      // 互相出现在对方的协作者里
      await alice.waitFor(`!!document.querySelector('[data-peer-avatar][title="Bob"]')`, { what: 'Alice 看到 Bob 在线', timeout: 30_000 })
      await bob.waitFor(`!!document.querySelector('[data-peer-avatar][title="Alice"]')`, { what: 'Bob 看到 Alice 在线', timeout: 30_000 })
      // 1. Alice 改一个格子，Bob 实时看到；Bob 不会因此显示「未保存」
      await set(alice, 'D1', 'hello')
      await bob.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('D1').getValue() === 'hello'`, { what: 'Bob 看到 D1=hello' })
      assert.ok(!(await saveText(bob)).includes('未保存'), `别人的修改不算 Bob 的未保存修改：${await saveText(bob)}`)
      // 2. 反过来
      await set(bob, 'E2', 42)
      await alice.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('E2').getValue() === 42`, { what: 'Alice 看到 E2=42' })
      // 3. 同一个格子同时改：两边最终一致（以服务端顺序为准）
      await Promise.all([set(alice, 'F1', 'a'), set(bob, 'F1', 'b')])
      await new Promise(r => setTimeout(r, 1500))
      const [fa, fb] = [await cell(alice, 'F1'), await cell(bob, 'F1')]
      assert.equal(fa, fb, `同格冲突后两边应一致：${fa} / ${fb}`)
      assert.ok(['a', 'b'].includes(fa))
      // 4. 结构变更：Alice 在最上面插入一行，Bob 的表头下移一行
      await alice.eval(`void window.__univerAPI.getActiveWorkbook().getActiveSheet().insertRowsBefore(0, 1)`)
      await bob.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('A2').getValue() === '订单号'`, { what: '插入行同步到 Bob' })
      // 5. 光标：Alice 选中 C3，Bob 的协作面板里显示 C3
      await alice.eval(`void window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('C3').activate()`)
      await bob.click('.avatars')
      await bob.waitFor(`[...document.querySelectorAll('[data-peer] .cursor-badge')].some(e => e.innerText.trim() === 'C3')`, { what: 'Bob 的面板里 Alice 的光标在 C3' })
      // 6. 各自保存不冲突：Alice 保存后 Bob 改了再保存，版本连续 +1，没有冲突
      const versionOf = async () => (await api.call('GET', `/ledger/workbooks/${id}`, TOKEN.alice)).data.version
      const v0 = await versionOf()
      await alice.press('s', { ctrl: true, code: 'KeyS' })
      await alice.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { what: 'Alice 保存' })
      assert.equal(await versionOf(), v0 + 1)
      await set(bob, 'G5', 'bob 的修改')
      await bob.press('s', { ctrl: true, code: 'KeyS' })
      await bob.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { what: 'Bob 保存（没有冲突）' })
      assert.equal(await versionOf(), v0 + 2)
      assert.ok(!(await bob.eval(`document.body.innerText.includes('保存冲突')`)))
      // Alice 也看到了 Bob 的修改，并且她的下一次保存不会冲突
      await alice.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('G5').getValue() === 'bob 的修改'`, { what: 'Alice 看到 G5' })
      await set(alice, 'H6', '再改一个')
      await alice.press('s', { ctrl: true, code: 'KeyS' })
      await alice.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { what: 'Alice 再次保存' })
      assert.equal(await versionOf(), v0 + 3)
      // 7. 晚加入的人：Alice 有一处还没保存的修改，新打开的页面也能看到
      await set(alice, 'I7', '还没保存')
      const root = await open('root')
      await root.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('I7').getValue() === '还没保存'`, { what: '晚加入的人补齐未保存的修改', timeout: 30_000 })
      assert.equal(await cell(root, 'H6'), '再改一个')
      // 8. 离开：Bob 关掉页面，Alice 这边他不再在线
      await bob.close()
      pages.splice(pages.indexOf(bob), 1)
      await alice.waitFor(`!document.querySelector('[data-peer-avatar][title="Bob"]')`, { what: 'Bob 离开后不再显示', timeout: 20_000 })
    }
    finally {
      for (const p of pages) await p.close().catch(() => {})
    }
  })

  it('AI 助手按钮打开 AI 面板', async () => {
    await withPage('ai-panel', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.click('.tool.ai')
      await page.waitFor(`document.querySelector('.side-panel .panel-title')?.innerText.includes('AI 分析助手')`, { what: 'AI 面板' })
      await page.click('.tool.ai')
      await page.waitFor(`!document.querySelector('.side-panel')`, { what: 'AI 面板关闭' })
    })
  })

  it('数据面板：数据集列表与行数；点开大数据表、返回工作簿', async () => {
    await withPage('dataset-view', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`[...document.querySelectorAll('.data-panel .item')].some(e => e.innerText.includes('订单') && e.innerText.includes('6'))`, { what: '数据面板列出「订单 6」' })
      await page.click('.data-panel .item', '订单')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('6 行')`, { what: '大数据表显示 6 行' })
      assert.ok(await page.eval(`!!document.querySelector('.statusbar')`), '大数据表状态栏存在')
      await page.click('.data-panel .item', 'Alice 利润表')
      await page.waitFor(`getComputedStyle(document.querySelector('.excel-body .absolute.inset-0.z-20') ?? document.body).display === 'none'`, { what: '返回工作簿视图' })
    })
  })

  it('大数据表「透视分析」：新建透视表页签并打开字段面板', async () => {
    await withPage('pivot-from-dataset', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      const sheetsBefore = await page.eval(`window.__univerAPI.getActiveWorkbook().getSheets().length`)
      await page.waitFor(`[...document.querySelectorAll('.data-panel .item')].some(e => e.innerText.includes('订单'))`, { what: '数据面板列出「订单」' })
      await page.click('.data-panel .item', '订单')
      await page.waitFor(`[...document.querySelectorAll('.excel-body button')].some(b => b.innerText.includes('透视分析'))`, { what: '大数据表工具栏' })
      await page.click('.excel-body button', '透视分析')
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getSheets().length === ${sheetsBefore + 1}`, { what: '多出一个透视表页签' })
      await page.waitFor(`document.querySelector('.side-panel .panel-title')?.innerText.includes('数据透视表字段')`, { what: '透视表字段面板' })
    })
  })

  it('审批流程：Alice 发起 → Bob 收到待办并通过', async () => {
    await withPage('approval-start', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await menu(page, '.topbar-right .ec-button', '发起审批')
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog')].some(d => d.offsetParent && d.innerText.includes('发起文件审批'))`, { what: '发起审批弹窗' })
      // 候选人里没有自己；选 Bob
      await page.click('.ec-dialog .ec-select')
      await page.waitFor(`[...document.querySelectorAll('.ec-select-dropdown__item')].some(e => e.innerText.trim() === 'Bob')`, { what: '候选人含 Bob' })
      assert.equal(await page.eval(`[...document.querySelectorAll('.ec-select-dropdown__item')].some(e => e.innerText.trim() === 'Alice')`), false, '候选人不含自己')
      await page.eval(`[...document.querySelectorAll('.ec-select-dropdown__item')].find(e => e.innerText.trim() === 'Bob').click()`)
      await page.fill('.ec-dialog textarea', '界面测试发起的审批')
      await page.click('.ec-dialog button', '确认发起')
      await page.waitFor(`[...document.querySelectorAll('.ec-drawer')].some(d => d.innerText.includes('审批中心') && d.innerText.includes('审批中') && d.innerText.includes('Bob'))`, { what: '审批中心显示审批中单据' })
      const started = (await api.call('GET', '/ledger/approvals?box=started', TOKEN.alice)).data
      assert.equal(started.length, 1)
      assert.equal(started[0].status, 'pending')
      assert.equal(started[0].approverName, 'Bob')
    })
    await withPage('approval-decide', 'bob', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`document.querySelector('.topbar-right .ec-badge sup')?.innerText.trim() === '1'`, { what: 'Bob 的审批角标为 1' })
      // 审批人对这个私有工作簿自动有查看权限：能打开，并带「只读」标记
      assert.match(await topbar(page), /只读/)
      await menu(page, '.topbar-right .ec-button', '审批中心')
      await page.waitFor(`[...document.querySelectorAll('.ec-drawer button')].some(b => b.innerText.trim() === '通过')`, { what: '待我审批里有「通过」按钮' })
      await page.fill('.ec-drawer input[placeholder*="意见"]', '界面测试通过')
      await page.click('.ec-drawer button', '通过')
      await page.waitFor(`document.querySelector('.topbar-right .ec-badge sup') === null || document.querySelector('.topbar-right .ec-badge sup')?.innerText.trim() === '0'`, { what: '角标清零' })
      const done = (await api.call('GET', '/ledger/approvals?box=done', TOKEN.bob)).data
      assert.equal(done[0].status, 'approved')
      assert.equal(done[0].decidedBy, 'Bob')
    })
  })

  it('受限用户（Bob 只能看 A、B 店）：范围标签、数据集行数、隐藏导入与连接入口', async () => {
    await withPage('restricted-user', 'bob', `/excel?wb=${wb.shared}`, async (page) => {
      await editorReady(page)
      assert.match(await topbar(page), /数据范围：A、B/)
      await page.waitFor(`[...document.querySelectorAll('.data-panel .item')].some(e => e.innerText.includes('订单') && /\\b4\\b/.test(e.innerText))`, { what: '订单只剩 4 行' })
      assert.equal(await page.eval(`[...document.querySelectorAll('.data-panel button')].some(b => b.innerText.includes('导入数据'))`), false, '没有「导入数据」')
      await page.click('.data-panel button', '数据中心')
      await page.waitFor(`[...document.querySelectorAll('.ec-dropdown-menu__item')].some(e => e.offsetParent && e.innerText.includes('SQL 查询'))`, { what: '数据中心菜单' })
      const items = await page.eval(`[...document.querySelectorAll('.ec-dropdown-menu__item')].filter(e => e.offsetParent).map(e => e.innerText.trim())`)
      for (const hidden of ['数据连接', '定时推送', '外部 AI 接入', '操作审计', '数据权限'])
        assert.ok(!items.some(i => i.includes(hidden)), `受限用户不应看到「${hidden}」：${items.join('、')}`)
    })
  })

  it('别人共享的只读工作簿：显示「只读」标记与提示', async () => {
    await withPage('readonly-workbook', 'bob', `/excel?wb=${wb.shared}`, async (page) => {
      await editorReady(page)
      assert.match(await topbar(page), /只读/)
      assert.match(await page.eval(`[...document.querySelectorAll('.ec-message')].map(m => m.innerText).join(' ')`), /只读工作簿/)
    })
  })

  it('管理员（Root）：数据中心菜单含数据权限、操作审计、外部 AI 接入', async () => {
    await withPage('admin-menu', 'root', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      assert.match(await topbar(page), /Root/)
      await page.click('.data-panel button', '数据中心')
      await page.waitFor(`[...document.querySelectorAll('.ec-dropdown-menu__item')].some(e => e.offsetParent && e.innerText.includes('数据权限'))`, { what: '数据中心菜单' })
      const items = await page.eval(`[...document.querySelectorAll('.ec-dropdown-menu__item')].filter(e => e.offsetParent).map(e => e.innerText.trim())`)
      for (const want of ['SQL 查询', '数据连接', '定时推送', '外部 AI 接入', '操作审计', '数据权限'])
        assert.ok(items.some(i => i.includes(want)), `管理员应看到「${want}」：${items.join('、')}`)
      await page.click('.ec-dropdown-menu__item', '操作审计')
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog')].some(d => d.offsetParent && d.innerText.includes('操作审计'))`, { what: '操作审计对话框' })
    })
  })

  it('数据连接：新建 HTTP 接口，鉴权选抖店开放平台 → 出现 App Key / 店铺 ID，可填入订单示例', async () => {
    await withPage('connector-doudian', 'root', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.click('.data-panel button', '数据中心')
      await page.waitFor(`[...document.querySelectorAll('.ec-dropdown-menu__item')].some(e => e.offsetParent && e.innerText.includes('数据连接'))`, { what: '数据中心菜单' })
      await page.click('.ec-dropdown-menu__item', '数据连接')
      await page.waitFor(`[...document.querySelectorAll('button')].some(b => b.offsetParent && b.innerText.includes('新建连接'))`, { what: '数据连接对话框' })
      await page.click('button', '新建连接')
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog')].some(d => d.offsetParent && d.innerText.includes('新建数据连接'))`, { what: '新建连接对话框' })
      await page.click('.ec-radio-button', 'HTTP 接口')
      await page.waitFor(`!!document.querySelector('input[placeholder*="erp.example.com"]')`, { what: 'HTTP 接口表单' })
      await page.click('.ec-select__wrapper', '不需要')
      await page.waitFor(`[...document.querySelectorAll('.ec-select-dropdown__item')].some(e => e.getClientRects().length && e.innerText.includes('抖店开放平台'))`, { what: '鉴权下拉选项' })
      await page.click('.ec-select-dropdown__item', '抖店开放平台')
      await page.waitFor(`!!document.querySelector('input[placeholder="App Key"]') && !!document.querySelector('input[placeholder*="店铺 ID"]') && !!document.querySelector('input[placeholder="App Secret"]')`, { what: 'App Key / 店铺 ID / App Secret 输入框' })
      await page.click('button', '填入「订单列表」示例')
      await page.waitFor(`document.querySelector('input[placeholder*="erp.example.com"]').value === 'https://openapi-fxg.jinritemai.com/order/searchList'`, { what: '地址被填入' })
      assert.match(await page.eval(`document.querySelector('textarea').value`), /update_time_start.*\{\{watermark\}\}/)
    })
  })

  it('异常发现：列出故障日与消失的店铺，点「查看明细」打开对应数据集并带上下钻筛选', async () => {
    await withPage('anomaly', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.click('.data-panel button', '数据中心')
      await page.waitFor(`[...document.querySelectorAll('.ec-dropdown-menu__item')].some(e => e.offsetParent && e.innerText.includes('异常发现'))`, { what: '数据中心菜单' })
      await page.click('.ec-dropdown-menu__item', '异常发现')
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog')].some(d => d.offsetParent && d.innerText.includes('2026-09-25') && d.innerText.includes('店铺「C」最近 7 天没有数据'))`, { what: '扫描出故障日与消失的店铺', timeout: 60_000 })
      const text = await page.eval(`[...document.querySelectorAll('.ec-dialog')].find(d => d.offsetParent && d.innerText.includes('异常发现')).innerText`)
      assert.match(text, /日销售/)
      assert.match(text, /行数偏低/)
      assert.doesNotMatch(text, /2026-09-2[678]/, '周末与恢复后的日子不应被报出')
      assert.match(text, /未能比较「订单」/, '数据不足的数据集给出说明')
      // 下钻：打开日销售，顶部出现下钻条，只剩故障日那一天的行（A3 + B3 = 6）
      await page.click('.ec-dialog .finding button', '查看明细')
      await page.waitFor(`!!document.querySelector('.drill-bar') && document.querySelector('.drill-bar').innerText.includes('2026-09-25')`, { what: '数据集打开并带上下钻条' })
      await page.waitFor(`/(^|\\s)6 行 \\/ 共/.test(document.body.innerText)`, { what: '下钻后只有 6 行（共 N 行中的 6 行）', timeout: 15_000 })
    })
  })

  /** 在 AI 面板里提问（模型按 llm 脚本回答），等回答结束 */
  async function askAi(page, text, { wbId = wb.alice, done = '方案已生成' } = {}) {
    // 同一个页面里连续提问时面板已经开着，再点按钮会把它关掉
    if (!await page.eval(`!!document.querySelector('.side-panel .panel-title')?.innerText.includes('AI 分析助手')`))
      await page.click('.tool.ai')
    await page.waitFor(`document.querySelector('.side-panel .panel-title')?.innerText.includes('AI 分析助手')`, { what: 'AI 面板' })
    // 每次从新对话开始，避免读到上一个测试留下的方案卡
    const count = async () => (await api.call('GET', `/ledger/ai/threads?workbookId=${wbId}`, TOKEN.alice)).data.length
    const n = await count()
    // 已有对话时等面板把它加载出来再点「新对话」（没加载完点会被当成「当前对话还是空的」而复用）
    if (n > 0)
      await page.waitFor(`document.querySelectorAll('.ai .card').length > 0`, { what: '面板加载出已有对话' })
    await page.click('.ai button', '新对话')
    for (let i = 0; i < 50 && await count() === n; i++) await new Promise(r => setTimeout(r, 100))
    await page.waitFor(`document.querySelectorAll('.ai .card').length === 0`, { what: '新对话是空的' })
    await page.fill('.ai textarea', text)
    await page.click('.ai button', '发送')
    await page.waitFor(`[...document.querySelectorAll('.ai .answer')].some(a => a.innerText.includes(${JSON.stringify(done)}))`, { what: 'AI 回答结束', timeout: 30_000 })
  }

  const PIVOT_STEP = { tool: 'create_pivot', args: { dataset: '订单', rows: [{ column: '店铺' }], values: [{ agg: 'sum', column: '金额' }] } }
  const CHART_STEP = { tool: 'create_chart', args: { title: '各店铺金额', type: 'column', dataset: '订单', x: { column: '店铺' }, values: [{ agg: 'sum', column: '金额' }] } }
  const sheetNames = page => page.eval(`window.__univerAPI.getActiveWorkbook().getSheets().map(s => s.getSheetName())`)

  it('AI 方案卡：预览多步 → 一次应用（新建透视表页签 + 图表）→ 一次撤销，刷新后状态仍在', async () => {
    llm.reset()
    llm.say({ calls: [{ name: 'propose_plan', args: { title: '各店铺金额分析', steps: [PIVOT_STEP, CHART_STEP] } }] })
    llm.say({ text: '方案已生成：先做各店铺金额透视表，再配一张柱形图。' })
    await withPage('ai-plan', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      const before = await sheetNames(page)
      await askAi(page, '按店铺汇总金额并配一张图')
      // 一张方案卡，两个步骤各带预览
      await page.waitFor(`document.querySelectorAll('.ai .card').length === 1 && document.querySelectorAll('.ai .plan-steps li').length === 2`, { what: '一张方案卡、两个步骤' })
      const card = await page.eval(`document.querySelector('.ai .card').innerText`)
      assert.match(card, /方案/)
      assert.match(card, /透视表/)
      assert.match(card, /图表/)
      assert.equal(await page.eval(`document.querySelectorAll('.ai .plan-steps li .preview table').length >= 1`), true, '透视表步骤带结果预览')
      assert.deepEqual(await sheetNames(page), before, '应用前工作簿不变')
      // 一次应用
      await page.click('.ai .card button', '应用方案')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已应用方案')`, { what: '方案已应用', timeout: 30_000 })
      const after = await sheetNames(page)
      assert.equal(after.length, before.length + 1, `应该新增一个透视表页签：${after}`)
      assert.match(await page.eval(`document.querySelector('.ai .card').innerText`), /插入图表「各店铺金额」/, '第二步的图表也插入了')
      // 一次撤销：页签删除
      await page.click('.ai .card button', '撤销')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已撤销')`, { what: '方案已撤销', timeout: 30_000 })
      assert.deepEqual(await sheetNames(page), before, '撤销后页签恢复')
    }, { allowErrors: [UNIVER_REMOVED_SHEET_RACE] })
    // 刷新页面（重新打开）：已撤销的状态来自服务端
    await withPage('ai-plan-reload', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.click('.tool.ai')
      await page.waitFor(`!!document.querySelector('.ai .el-select, .ai .ec-select')`, { what: 'AI 面板' })
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已撤销')`, { what: '刷新后方案卡仍显示「已撤销」', timeout: 30_000 })
    })
  })

  it('AI 方案卡：中途取消覆盖确认 → 已完成的步骤（含服务端的计算列）自动撤回', async () => {
    llm.reset()
    // 第 1 步给「订单」加计算列（写到服务端），第 2 步往 A1 写公式——Sheet1 的 A1 已有「订单号」，会弹覆盖确认，选取消
    llm.say({ calls: [{ name: 'propose_plan', args: { title: '加计算列并写合计', steps: [{ tool: 'add_calc_column', args: { dataset: '订单', name: '金额x2', formula: '=[金额]*2' } }, { tool: 'write_formula', args: { cell: 'A1', formula: '=1+1', explanation: '示例' } }] } }] })
    llm.say({ text: '方案已生成：先加计算列，再写入公式。' })
    const hasCalc = async () => (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.name === '订单').columns.some(c => c.title === '金额x2')
    await withPage('ai-plan-rollback', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await askAi(page, '给订单加一列金额x2，再在 A1 写个公式')
      await page.waitFor(`document.querySelectorAll('.ai .plan-steps li').length === 2`, { what: '方案卡两步' })
      await page.click('.ai .card button', '应用方案')
      await page.waitFor(`[...document.querySelectorAll('.ec-message-box')].some(d => d.offsetParent && d.innerText.includes('覆盖'))`, { what: '覆盖单元格确认框' })
      assert.equal(await hasCalc(), true, '确认框弹出时，第 1 步的计算列已经写到服务端')
      await page.click('.ec-message-box button', '取消')
      await page.waitFor(`document.querySelector('.ai .card button')?.innerText.includes('应用方案')`, { what: '方案仍是待应用状态', timeout: 30_000 })
      for (let i = 0; i < 50 && await hasCalc(); i++) await new Promise(r => setTimeout(r, 200))
      assert.equal(await hasCalc(), false, '取消后计算列被撤回')
      assert.equal(await page.eval(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('A1').getValue()`), '订单号', 'A1 没有被覆盖')
    })
  })

  it('AI 数据表视图：只看甲店、按金额降序 → 预览命中行数 → 应用后数据表刷新 → 撤销恢复原视图', async () => {
    const made = await api.call('POST', '/ledger/datasets/input', TOKEN.alice, { body: { name: 'UI视图明细', columns: [{ title: '订单号', kind: 'text' }, { title: '店铺', kind: 'text' }, { title: '金额', kind: 'money' }], primaryKey: ['订单号'] } })
    const dsId = made.data
    await api.call('POST', `/ledger/datasets/${dsId}/writeback`, TOKEN.alice, { body: { mode: 'upsert', columns: ['订单号', '店铺', '金额'], rows: [['A1', '甲', 100], ['A2', '乙', 200], ['A3', '甲', 300]] } })
    const spec = { v: 1, id: 'view1', kind: 'datatable', name: '视图表', datasetId: dsId, fields: [], filters: [], sort: [], followSlicers: false, row: 0, col: 0 }
    const json = { id: 'wb', name: '视图', sheetOrder: ['s1'], sheets: { s1: { id: 's1', name: '视图', cellData: {}, rowCount: 100, columnCount: 20, custom: { celltables: [spec] } } } }
    const zlib = await import('node:zlib')
    const created = await api.call('POST', `/ledger/workbooks?name=${encodeURIComponent('视图')}`, TOKEN.alice, { raw: zlib.gzipSync(Buffer.from(JSON.stringify(json))) })
    const id = created.data.id
    const grid = page => page.eval(`(() => { const s = window.__univerAPI.getActiveWorkbook().getActiveSheet(); return s.getRange(0, 0, 5, 3).getValues().map(r => r.join('|').replace(/,/g, '').replace(/\\.00(?=\\||$)/g, '')).filter(r => r !== '||') })()`)
    const original = ['订单号|店铺|金额', 'A1|甲|100', 'A2|乙|200', 'A3|甲|300']
    llm.reset()
    llm.say({ calls: [{ name: 'set_table_view', args: { table: '视图表', dataset: 'UI视图明细', filters: [{ op: 'in', column: '店铺', values: ['甲'] }], sort: [{ column: '金额', desc: true }] } }] })
    llm.say({ text: '视图已生成：只看甲店并按金额降序。' })
    await withPage('ai-table-view', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(3, 0).getValue() === 'A3'`, { what: '数据表按数据集写入', timeout: 30_000 })
      assert.deepEqual(await grid(page), original)
      await askAi(page, '只看甲店，按金额降序', { wbId: id, done: '视图已生成' })
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('命中 2 行 / 共 3 行')`, { what: '建议卡显示命中行数' })
      const card = await page.eval(`document.querySelector('.ai .card').innerText`)
      assert.match(card, /视图表/)
      assert.equal(await page.eval(`!!document.querySelector('.ai .card .preview table')`), true, '带前几行预览')
      assert.deepEqual(await grid(page), original, '应用前数据表不变')
      // 发给模型的提问里带着工作簿里的数据表
      const asked = JSON.stringify(llm.requests[0].messages.at(-1))
      assert.match(asked, /工作簿里的数据表：「视图表」（数据集「UI视图明细」/)
      await page.click('.ai .card button', '应用')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已调整数据表「视图表」')`, { what: '建议已应用', timeout: 30_000 })
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(1, 0).getValue() === 'A3' && !window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(3, 0).getValue()`, { what: '数据表只剩甲店两行、按金额降序', timeout: 30_000 })
      assert.deepEqual(await grid(page), ['订单号|店铺|金额', 'A3|甲|300', 'A1|甲|100'])
      // 撤销：恢复原来的视图，被筛掉的行回来
      await page.click('.ai .card button', '撤销')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已撤销')`, { what: '建议已撤销', timeout: 30_000 })
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(3, 0).getValue() === 'A3'`, { what: '数据表恢复三行', timeout: 30_000 })
      assert.deepEqual(await grid(page), original)
    })
  })

  it('AI 列样式：浮动数据表金额列「超过 150 标红」→ 预览各规则命中行数 → 应用后画布出现底色 → 撤销恢复；单元格模式的表不支持条件格式', async () => {
    const made = await api.call('POST', '/ledger/datasets/input', TOKEN.alice, { body: { name: 'UI样式明细', columns: [{ title: '订单号', kind: 'text' }, { title: '店铺', kind: 'text' }, { title: '金额', kind: 'money' }], primaryKey: ['订单号'] } })
    const dsId = made.data
    await api.call('POST', `/ledger/datasets/${dsId}/writeback`, TOKEN.alice, { body: { mode: 'upsert', columns: ['订单号', '店铺', '金额'], rows: [['A1', '甲', 100], ['A2', '乙', 200], ['A3', '甲', 300]] } })
    const cellSpec = { v: 1, id: 'cell1', kind: 'datatable', name: '单元格表', datasetId: dsId, fields: [], filters: [], sort: [], followSlicers: false, row: 10, col: 0 }
    const json = { id: 'wb', name: '样式', sheetOrder: ['s1'], sheets: { s1: { id: 's1', name: '样式', cellData: {}, rowCount: 100, columnCount: 20, custom: { celltables: [cellSpec] } } } }
    const zlib = await import('node:zlib')
    const created = await api.call('POST', `/ledger/workbooks?name=${encodeURIComponent('样式')}`, TOKEN.alice, { raw: zlib.gzipSync(Buffer.from(JSON.stringify(json))) })
    const id = created.data.id
    const floatSpec = { v: 1, id: 'float1', kind: 'datatable', name: '样式表', datasetId: dsId, fields: [], filters: [], sort: [], followSlicers: false }
    // 画布上与规则底色 #FDE8E8（253, 232, 232）完全相同的像素数：规则命中的格子会铺这个底色
    const tinted = page => page.eval(`(() => { const c = document.querySelector('.body canvas'); if (!c) return -1; const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; let n = 0; for (let i = 0; i < d.length; i += 4) if (d[i] === 253 && d[i + 1] === 232 && d[i + 2] === 232) n++; return n })()`)
    const styles = page => page.eval(`(() => { const s = window.__univerAPI.getActiveWorkbook().getActiveSheet(); const f = s.getAllFloatDoms().find(d => d.data?.id === 'float1'); return JSON.stringify(f?.data?.columnStyles ?? null) })()`)
    llm.reset()
    llm.say({ calls: [{ name: 'set_column_format', args: { table: '样式表', dataset: 'UI样式明细', column: '金额', numFmt: 'wan', rules: [{ op: '>', value: 150, style: { color: '#C00000', bg: '#FDE8E8', bold: true } }] } }] })
    llm.say({ text: '列样式已生成：金额用万元显示，超过 150 标红。' })
    llm.say({ calls: [{ name: 'set_column_format', args: { table: '单元格表', dataset: 'UI样式明细', column: '金额', rules: [{ op: '>', value: 150, style: { bg: '#FDE8E8' } }] } }] })
    llm.say({ text: '单元格表的列样式已生成。' })
    await withPage('ai-column-format', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      await page.eval(`window.__univerAPI.getActiveWorkbook().getActiveSheet().addFloatDomToPosition({ componentKey: 'webexcel-datatable', initPosition: { startX: 20, endX: 740, startY: 20, endY: 380 }, data: ${JSON.stringify(floatSpec)}, allowTransform: true })`)
      await page.waitFor(`!!document.querySelector('.body canvas')`, { what: '浮动数据表画布出现', timeout: 30_000 })
      await page.waitFor(`document.body.innerText.includes('3 行') || !!document.querySelector('.body canvas')`, { what: '数据表打开' })
      assert.equal(await tinted(page), 0, '没设规则时没有底色')
      await askAi(page, '样式表的金额用万元显示，超过 150 的标红', { wbId: id, done: '列样式已生成' })
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('样式表')`, { what: '建议卡出现' })
      const card = await page.eval(`document.querySelector('.ai .card').innerText`)
      assert.match(card, /数字格式 万元/)
      assert.match(card, /金额 > 150/, '预览列出规则')
      assert.match(card, /66\.67%/, '预览给出命中行数占比（3 行里命中 2 行）')
      assert.equal(await styles(page), 'null', '应用前不变')
      await page.click('.ai .card button', '应用')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已设置数据表「样式表」金额列')`, { what: '建议已应用', timeout: 30_000 })
      assert.deepEqual(JSON.parse(await styles(page)), { 金额: { numFmt: 'wan', rules: [{ op: '>', value: 150, style: { color: '#C00000', bg: '#FDE8E8', bold: true } }] } })
      for (let i = 0; i < 50 && await tinted(page) === 0; i++) await new Promise(r => setTimeout(r, 200))
      assert.ok(await tinted(page) > 500, '命中规则的格子在画布上铺了底色')
      await page.screenshot(path.join(ARTIFACTS, 'ai-column-format-applied.png')).catch(() => {})
      // 撤销：样式记录被删掉，底色消失
      await page.click('.ai .card button', '撤销')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已撤销')`, { what: '建议已撤销', timeout: 30_000 })
      assert.equal(await styles(page), 'null')
      for (let i = 0; i < 50 && await tinted(page) > 0; i++) await new Promise(r => setTimeout(r, 200))
      assert.equal(await tinted(page), 0, '撤销后底色消失')
      // 单元格模式的数据表：条件格式不支持，应用时给出明确提示，不改表
      await askAi(page, '单元格表的金额超过 150 标红', { wbId: id, done: '单元格表的列样式已生成' })
      await page.click('.ai .card button', '应用')
      await page.waitFor(`[...document.querySelectorAll('.ec-message')].some(m => m.innerText.includes('单元格模式的数据表，不支持条件格式'))`, { what: '单元格模式的提示', timeout: 30_000 })
      assert.match(await page.eval(`document.querySelector('.ai .card button')?.innerText ?? ''`), /应用/, '建议仍是待应用状态')
    })
  })

  it('AI 新建数据表：浮动表格带筛选排序 → 应用后出现在工作表 → 撤销删除；铺到单元格 → 写入单元格 → 撤销清掉；重名自动加序号', async () => {
    const made = await api.call('POST', '/ledger/datasets/input', TOKEN.alice, { body: { name: 'UI新建明细', columns: [{ title: '订单号', kind: 'text' }, { title: '店铺', kind: 'text' }, { title: '金额', kind: 'money' }], primaryKey: ['订单号'] } })
    const dsId = made.data
    await api.call('POST', `/ledger/datasets/${dsId}/writeback`, TOKEN.alice, { body: { mode: 'upsert', columns: ['订单号', '店铺', '金额'], rows: [['A1', '甲', 100], ['A2', '乙', 200], ['A3', '甲', 300]] } })
    const json = { id: 'wb', name: '新建', sheetOrder: ['s1'], sheets: { s1: { id: 's1', name: '新建', cellData: {}, rowCount: 100, columnCount: 20 } } }
    const zlib = await import('node:zlib')
    const created = await api.call('POST', `/ledger/workbooks?name=${encodeURIComponent('新建')}`, TOKEN.alice, { raw: zlib.gzipSync(Buffer.from(JSON.stringify(json))) })
    const id = created.data.id
    const floats = page => page.eval(`JSON.stringify(window.__univerAPI.getActiveWorkbook().getActiveSheet().getAllFloatDoms().filter(d => d.data?.kind === 'datatable').map(d => ({ name: d.data.name, filters: d.data.filters, sort: d.data.sort })))`)
    const cellTables = page => page.eval(`JSON.stringify((window.__univerAPI.getActiveWorkbook().getActiveSheet().getCustomMetadata()?.celltables ?? []).map(t => ({ name: t.name, row: t.row, col: t.col })))`)
    const grid = page => page.eval(`(() => { const s = window.__univerAPI.getActiveWorkbook().getActiveSheet(); return s.getRange(0, 0, 5, 3).getValues().map(r => r.join('|').replace(/,/g, '').replace(/\\.00(?=\\||$)/g, '')).filter(r => r !== '||') })()`)
    llm.reset()
    llm.say({ calls: [{ name: 'create_data_table', args: { dataset: 'UI新建明细', name: '甲店明细', filters: [{ op: 'in', column: '店铺', values: ['甲'] }], sort: [{ column: '金额', desc: true }] } }] })
    llm.say({ text: '新建数据表已生成：甲店明细。' })
    llm.say({ calls: [{ name: 'create_data_table', args: { dataset: 'UI新建明细', name: '甲店明细' } }] })
    llm.say({ text: '再建一张同名数据表已生成。' })
    llm.say({ calls: [{ name: 'create_data_table', args: { dataset: 'UI新建明细', name: '甲店明细', mode: 'cells', sort: [{ column: '金额', desc: true }] } }] })
    llm.say({ text: '铺到单元格的数据表已生成。' })
    await withPage('ai-table-create', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      assert.equal(await floats(page), '[]')
      // 浮动表格
      await askAi(page, '把甲店的订单放成一张表，按金额降序', { wbId: id, done: '新建数据表已生成' })
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('命中 2 行 / 共 3 行')`, { what: '建议卡显示命中行数' })
      const card = await page.eval(`document.querySelector('.ai .card').innerText`)
      assert.match(card, /新建数据表/)
      assert.match(card, /浮动表格/)
      assert.equal(await page.eval(`!!document.querySelector('.ai .card .preview table')`), true, '带前几行预览')
      assert.equal(await floats(page), '[]', '应用前工作表不变')
      await page.click('.ai .card button', '应用')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已在当前工作表插入数据表「甲店明细」')`, { what: '建议已应用', timeout: 30_000 })
      const made1 = JSON.parse(await floats(page))
      assert.equal(made1.length, 1)
      assert.equal(made1[0].name, '甲店明细')
      assert.equal(made1[0].filters[0].values[0], '甲')
      assert.equal(made1[0].sort[0].desc, true)
      await page.waitFor(`!!document.querySelector('.body canvas')`, { what: '浮动数据表画布出现', timeout: 30_000 })
      // 撤销：浮动表格删除
      await page.click('.ai .card button', '撤销')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已撤销')`, { what: '建议已撤销', timeout: 30_000 })
      assert.equal(await floats(page), '[]', '撤销后浮动表格删除')
      // 再建一张同名的浮动表格（上一张已删除，不冲突），留在工作表里
      await askAi(page, '再放一张订单表', { wbId: id, done: '再建一张同名数据表已生成' })
      await page.click('.ai .card button', '应用')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已在当前工作表插入数据表「甲店明细」')`, { what: '同名表应用', timeout: 30_000 })
      assert.equal(JSON.parse(await floats(page)).length, 1)
      // 铺到单元格：「甲店明细」已被占用，自动改名为「甲店明细2」；写入 A1 起
      await askAi(page, '把全部订单铺到单元格，按金额降序', { wbId: id, done: '铺到单元格的数据表已生成' })
      await page.click('.ai .card button', '应用')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已在当前工作表插入数据表「甲店明细2」')`, { what: '重名自动加序号', timeout: 30_000 })
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(1, 0).getValue() === 'A3'`, { what: '数据写进单元格', timeout: 30_000 })
      assert.deepEqual(await grid(page), ['订单号|店铺|金额', 'A3|甲|300', 'A2|乙|200', 'A1|甲|100'])
      assert.deepEqual(JSON.parse(await cellTables(page)), [{ name: '甲店明细2', row: 0, col: 0 }])
      // 撤销：清掉写入的单元格与定义
      await page.click('.ai .card button', '撤销')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已撤销')`, { what: '铺到单元格已撤销', timeout: 30_000 })
      await page.waitFor(`!window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(1, 0).getValue()`, { what: '单元格被清掉', timeout: 30_000 })
      assert.deepEqual(await grid(page), [])
      assert.equal(await cellTables(page), '[]')
      assert.equal(JSON.parse(await floats(page)).length, 1, '撤销铺到单元格的表不影响另一张浮动表格')
    })
  })

  it('AI 另存为数据集：预览 SQL 与结果 → 应用后数据集与查询创建 → 撤销移入回收站；数据集已被引用时撤销保留不动', async () => {
    const SQL = 'SELECT 店铺, SUM(金额) AS 销售额 FROM 订单 GROUP BY 店铺'
    const datasetNames = async () => (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.map(d => d.name)
    const queries = async () => (await api.call('GET', '/ledger/sql/queries', TOKEN.alice)).data
    llm.reset()
    llm.say({ calls: [{ name: 'save_query_as_dataset', args: { sql: SQL, name: 'UI店铺汇总', purpose: '各店铺销售额' } }] })
    llm.say({ text: '数据集建议已生成：UI店铺汇总。' })
    llm.say({ calls: [{ name: 'save_query_as_dataset', args: { sql: SQL, name: 'UI店铺汇总2' } }] })
    llm.say({ text: '第二个数据集建议已生成。' })
    await withPage('ai-dataset-save', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await askAi(page, '把各店铺销售额存成数据集', { done: '数据集建议已生成' })
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('另存为数据集「UI店铺汇总」')`, { what: '建议卡出现' })
      const card = await page.eval(`document.querySelector('.ai .card').innerText`)
      assert.match(card, /SELECT 店铺, SUM\(金额\) AS 销售额 FROM 订单 GROUP BY 店铺/, '卡片展示 SQL')
      assert.match(card, /各店铺销售额/, '展示用途')
      assert.match(card, /共 \d+ 行 × 2 列/)
      assert.equal(await page.eval(`!!document.querySelector('.ai .card .preview table')`), true, '带前几行预览')
      assert.ok(!(await datasetNames()).includes('UI店铺汇总'), '确认之前不创建数据集')
      await page.click('.ai .card button', '应用')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已另存为数据集「UI店铺汇总」')`, { what: '已另存', timeout: 60_000 })
      assert.ok((await datasetNames()).includes('UI店铺汇总'), '数据集创建了')
      const q = (await queries()).find(x => x.name === 'UI店铺汇总')
      assert.ok(q?.datasetId, '生成了关联数据集的查询')
      assert.equal(q.schedule.kind, 'manual')
      // 撤销：查询删除，数据集进回收站
      await page.click('.ai .card button', '撤销')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已撤销')`, { what: '已撤销', timeout: 60_000 })
      assert.ok(!(await datasetNames()).includes('UI店铺汇总'), '数据集不在列表里了')
      assert.ok(!(await queries()).some(x => x.name === 'UI店铺汇总'), '查询删除了')
      const trash = await api.call('GET', '/ledger/datasets/trash', TOKEN.alice)
      assert.ok(JSON.stringify(trash.data).includes('UI店铺汇总'), '数据集在回收站里，可恢复')
      // 数据集被别的查询引用：撤销时保留不动
      await askAi(page, '再存一个', { done: '第二个数据集建议已生成' })
      await page.click('.ai .card button', '应用')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已另存为数据集「UI店铺汇总2」')`, { what: '第二个已另存', timeout: 60_000 })
      const other = await api.call('POST', '/ledger/sql/queries', TOKEN.alice, { body: { name: '引用汇总2', sql: 'SELECT * FROM UI店铺汇总2', schedule: { kind: 'manual' } } })
      assert.equal(other.code ?? 0, 0, JSON.stringify(other))
      await page.click('.ai .card button', '撤销')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已被') && document.querySelector('.ai .card')?.innerText.includes('保留不动')`, { what: '被引用时保留的说明', timeout: 60_000 })
      assert.ok((await datasetNames()).includes('UI店铺汇总2'), '被引用的数据集还在')
    })
  })

  it('AI 方案卡里的数据表工具：新建数据表 + 设列样式一次应用、一次撤销；重名时后一步跟着新表走；中途失败整体撤回', async () => {
    const made = await api.call('POST', '/ledger/datasets/input', TOKEN.alice, { body: { name: 'UI方案明细', columns: [{ title: '订单号', kind: 'text' }, { title: '店铺', kind: 'text' }, { title: '金额', kind: 'money' }], primaryKey: ['订单号'] } })
    const dsId = made.data
    await api.call('POST', `/ledger/datasets/${dsId}/writeback`, TOKEN.alice, { body: { mode: 'upsert', columns: ['订单号', '店铺', '金额'], rows: [['A1', '甲', 100], ['A2', '乙', 200], ['A3', '甲', 300]] } })
    const json = { id: 'wb', name: '方案', sheetOrder: ['s1'], sheets: { s1: { id: 's1', name: '方案', cellData: {}, rowCount: 100, columnCount: 20 } } }
    const zlib = await import('node:zlib')
    const created = await api.call('POST', `/ledger/workbooks?name=${encodeURIComponent('方案数据表')}`, TOKEN.alice, { raw: zlib.gzipSync(Buffer.from(JSON.stringify(json))) })
    const id = created.data.id
    const floats = page => page.eval(`JSON.stringify(window.__univerAPI.getActiveWorkbook().getActiveSheet().getAllFloatDoms().filter(d => d.data?.kind === 'datatable').map(d => ({ name: d.data.name, styles: d.data.columnStyles ?? null, sort: d.data.sort, filters: d.data.filters })))`)
    // 所有数据表画布上与规则底色 #FDE8E8 完全相同的像素数
    const tinted = page => page.eval(`(() => { let n = 0; for (const c of document.querySelectorAll('.body canvas')) { const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; for (let i = 0; i < d.length; i += 4) if (d[i] === 253 && d[i + 1] === 232 && d[i + 2] === 232) n++ } return n })()`)
    const planArgs = (title, steps) => ({ title, steps })
    const CREATE = { tool: 'create_data_table', args: { dataset: 'UI方案明细', name: '甲店明细', filters: [{ op: 'in', column: '店铺', values: ['甲'] }] } }
    const FMT = { tool: 'set_column_format', args: { table: '甲店明细', dataset: 'UI方案明细', column: '金额', numFmt: 'wan', rules: [{ op: '>', value: 150, style: { color: '#C00000', bg: '#FDE8E8', bold: true } }] } }
    llm.reset()
    llm.say({ calls: [{ name: 'propose_plan', args: planArgs('甲店明细', [CREATE, FMT]) }] })
    llm.say({ text: '方案一已生成。' })
    llm.say({ calls: [{ name: 'propose_plan', args: planArgs('甲店明细（重名）', [CREATE, FMT]) }] })
    llm.say({ text: '方案二已生成。' })
    llm.say({ calls: [{ name: 'propose_plan', args: planArgs('先改视图再铺单元格', [{ tool: 'set_table_view', args: { table: '甲店明细', dataset: 'UI方案明细', sort: [{ column: '金额', desc: true }] } }, { tool: 'create_data_table', args: { dataset: 'UI方案明细', name: '铺开表', mode: 'cells' } }]) }] })
    llm.say({ text: '方案三已生成。' })
    await withPage('ai-plan-tables', 'alice', `/excel?wb=${id}`, async (page) => {
      await editorReady(page)
      // 方案一：新建 + 设样式，一次应用
      await askAi(page, '甲店订单放成一张表，金额万元显示超过 150 标红', { wbId: id, done: '方案一已生成' })
      await page.waitFor(`document.querySelectorAll('.ai .plan-steps li').length === 2`, { what: '方案卡两步' })
      const card = await page.eval(`document.querySelector('.ai .card').innerText`)
      assert.match(card, /新建数据表/)
      assert.match(card, /列样式/)
      assert.equal(await floats(page), '[]', '应用前不变')
      await page.click('.ai .card button', '应用方案')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已应用方案')`, { what: '方案已应用', timeout: 30_000 })
      let list = JSON.parse(await floats(page))
      assert.equal(list.length, 1)
      assert.equal(list[0].name, '甲店明细')
      assert.equal(list[0].styles['金额'].numFmt, 'wan')
      assert.equal(list[0].filters[0].values[0], '甲')
      for (let i = 0; i < 50 && await tinted(page) === 0; i++) await new Promise(r => setTimeout(r, 200))
      assert.ok(await tinted(page) > 200, '命中规则的格子铺了底色')
      // 一次撤销：表连同样式一起没了
      await page.click('.ai .card button', '撤销')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已撤销')`, { what: '方案已撤销', timeout: 30_000 })
      assert.equal(await floats(page), '[]')
      // 方案二：先放一张同名的老表（没有样式），再应用方案：新表自动叫「甲店明细2」，样式只落在新表上
      await page.eval(`(() => { window.__univerAPI.getActiveWorkbook().getActiveSheet().addFloatDomToPosition({ componentKey: 'webexcel-datatable', initPosition: { startX: 20, endX: 500, startY: 20, endY: 200 }, data: ${JSON.stringify({ v: 1, id: 'old1', kind: 'datatable', name: '甲店明细', datasetId: dsId, fields: [], filters: [], sort: [], followSlicers: false })}, allowTransform: true }) })()`)
      await askAi(page, '再来一遍', { wbId: id, done: '方案二已生成' })
      await page.click('.ai .card button', '应用方案')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已应用方案')`, { what: '方案二已应用', timeout: 30_000 })
      list = JSON.parse(await floats(page))
      const old = list.find(t => t.name === '甲店明细')
      const fresh = list.find(t => t.name === '甲店明细2')
      assert.ok(old && fresh, `应有新旧两张表：${JSON.stringify(list.map(t => t.name))}`)
      assert.equal(old.styles, null, '老表没被改')
      assert.equal(fresh.styles['金额'].numFmt, 'wan', '样式落在新表上')
      await page.click('.ai .card button', '撤销')
      await page.waitFor(`document.querySelector('.ai .card')?.innerText.includes('已撤销')`, { what: '方案二已撤销', timeout: 30_000 })
      list = JSON.parse(await floats(page))
      assert.deepEqual(list.map(t => t.name), ['甲店明细'], '只剩老表')
      // 方案三：第 1 步改视图成功，第 2 步铺单元格被 A1 的内容挡住 → 整体撤回，老表的排序恢复
      await page.eval(`(() => { window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange('A1').setValue('占位') })()`)
      await askAi(page, '改视图再铺到单元格', { wbId: id, done: '方案三已生成' })
      await page.click('.ai .card button', '应用方案')
      await page.waitFor(`[...document.querySelectorAll('.ec-message')].some(m => m.innerText.includes('第 2 步') && m.innerText.includes('已撤回前面 1 步'))`, { what: '第 2 步失败、撤回前面 1 步', timeout: 30_000 })
      assert.deepEqual(JSON.parse(await floats(page))[0].sort, [], '第 1 步的排序被撤回')
      assert.match(await page.eval(`document.querySelector('.ai .card button')?.innerText ?? ''`), /应用方案/, '方案仍是待应用状态')
    })
  })

  it('AI 会话：别人的会话不出现、凭链接也打不开；自己的会话摘要可以编辑、清除', async () => {
    llm.reset()
    const wbId = (await api.createWorkbook(TOKEN.alice, 'AI 会话权限')).id
    await api.shareWorkbook(TOKEN.alice, wbId, 'edit')
    const tid = (await api.call('POST', '/ledger/ai/threads', TOKEN.alice, { body: { workbookId: wbId } })).data.id
    const bobTid = (await api.call('POST', '/ledger/ai/threads', TOKEN.bob, { body: { workbookId: wbId } })).data.id
    // 提 8 个问题：超过 6 轮后，最早的一轮被服务端整理进摘要
    for (let i = 1; i <= 8; i++) {
      llm.say({ text: `第 ${i} 个答复` })
      const r = await api.call('POST', `/ledger/ai/threads/${tid}/chat`, TOKEN.alice, { body: { message: `第 ${i} 个问题`, context: {} } })
      assert.equal(r.status, 200)
    }
    llm.say({ text: '我是 Bob 的答复' })
    await api.call('POST', `/ledger/ai/threads/${bobTid}/chat`, TOKEN.bob, { body: { message: 'Bob 的问题', context: {} } })
    const summaryOf = async () => (await api.call('GET', `/ledger/ai/threads/${tid}`, TOKEN.alice)).data.summary
    assert.equal(await summaryOf(), '摘要', '服务端已整理出摘要')

    // Alice：打开链接，摘要分隔线出现 → 编辑 → 清除
    await withPage('ai-summary-edit', 'alice', `/excel?wb=${wbId}&ai=${tid}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`document.querySelector('.side-panel .panel-title')?.innerText.includes('AI 分析助手')`, { what: '链接带 ai 参数，AI 面板自动打开' })
      await page.waitFor(`!!document.querySelector('.ai .summary-bar')`, { what: '摘要分隔线' })
      await page.click('.ai .summary-bar')
      await page.waitFor(`document.querySelector('.ai .summary-text')?.innerText.includes('摘要')`, { what: '摘要内容' })
      await page.click('.ai button', '编辑摘要')
      await page.waitFor(`!!document.querySelector('.ai .summary textarea')`, { what: '摘要编辑框' })
      await page.fill('.ai .summary textarea', '- 用户关心店铺 A\n- 金额单位用万')
      await page.click('.ai button', '保存')
      await page.waitFor(`document.querySelector('.ai .summary-text')?.innerText.includes('金额单位用万')`, { what: '保存后显示新摘要' })
      assert.equal(await summaryOf(), '- 用户关心店铺 A\n- 金额单位用万', '服务端摘要已更新')
      await page.click('.ai button', '清除')
      await page.waitFor(`[...document.querySelectorAll('.ec-message-box')].some(d => d.offsetParent && d.innerText.includes('清除摘要'))`, { what: '清除确认框' })
      await page.click('.ec-message-box button', '清除')
      await page.waitFor(`!document.querySelector('.ai .summary-bar')`, { what: '清除后分隔线消失' })
      assert.equal(await summaryOf(), null, '服务端摘要已清除')
    })

    // Bob：用 Alice 的会话链接打开 → 提示是 Alice 的，界面里看不到她的内容，只有自己的会话
    await withPage('ai-thread-foreign', 'bob', `/excel?wb=${wbId}&ai=${tid}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`document.querySelector('.side-panel .panel-title')?.innerText.includes('AI 分析助手')`, { what: '链接带 ai 参数，AI 面板自动打开' })
      await page.waitFor(`[...document.querySelectorAll('.ec-message')].some(e => e.innerText.includes('Alice') && e.innerText.includes('只有本人和管理员能查看'))`, { what: '提示这是 Alice 的对话' })
      await page.waitFor(`document.querySelector('.side-panel')?.innerText.includes('Bob 的问题')`, { what: '显示的是 Bob 自己的会话' })
      const text = await page.eval(`document.querySelector('.side-panel').innerText`)
      assert.ok(!text.includes('第 8 个问题') && !text.includes('第 8 个答复'), 'Bob 看不到 Alice 会话的内容')
    }, { allowErrors: [/403/, /请求错误/] })
  })

  it('首页：最近的工作簿列表、星标、搜索、新建入口', async () => {
    await withPage('home-recent', 'alice', '/excel/home', async (page) => {
      await page.waitFor(`[...document.querySelectorAll('.list-row')].filter(r => r.innerText.includes('Alice 利润表') || r.innerText.includes('共享只读看板')).length === 2`, { what: '列出 Alice 的工作簿', timeout: 60_000 })
      const text = await page.eval(`document.body.innerText`)
      for (const want of ['空白工作簿', '从模板新建', '导入数据', '最近的工作簿', '名称', '修改时间'])
        assert.ok(text.includes(want), `首页应显示「${want}」`)
      // 搜索
      await page.fill('input[placeholder="搜索工作簿"]', '共享')
      await page.waitFor(`[...document.querySelectorAll('.list-row')].filter(r => r.innerText.includes('Alice 利润表')).length === 0`, { what: '搜索后只剩匹配项' })
      await page.fill('input[placeholder="搜索工作簿"]', '')
      await page.waitFor(`[...document.querySelectorAll('.list-row')].some(r => r.innerText.includes('Alice 利润表'))`, { what: '清空搜索' })
      // 加星标 → 「已加星标」页签只剩它
      await page.eval(`[...document.querySelectorAll('.list-row')].find(r => r.innerText.includes('共享只读看板')).querySelector('.star').click()`)
      await page.waitFor(`[...document.querySelectorAll('.list-row')].find(r => r.innerText.includes('共享只读看板'))?.querySelector('.star.on')`, { what: '星标亮起' })
      await page.click('.nav-item', '已加星标')
      await page.waitFor(`[...document.querySelectorAll('.list-row')].filter(r => r.querySelector('.star')).length === 1`, { what: '已加星标页签只有 1 个' })
      await page.eval(`document.querySelector('.list-row .star.on').click()`)
      await page.waitFor(`document.body.innerText.includes('还没有加星标的工作簿')`, { what: '取消星标后为空' })
      // 打开工作簿：整页跳到编辑器
      await page.click('.nav-item', '最近')
      await page.waitFor(`[...document.querySelectorAll('.list-row')].some(r => r.innerText.includes('Alice 利润表'))`, { what: '回到最近' })
      await page.click('.list-row', 'Alice 利润表')
      await page.waitFor(`location.pathname === '/excel' && location.search.includes('wb=${wb.alice}')`, { what: '跳转到编辑器', timeout: 30_000 })
    })
  })

  it('首页：数据中心（数据集列表、SQL 卡片）与自动化（管理员卡片）', async () => {
    await withPage('home-data', 'root', '/excel/home', async (page) => {
      await page.waitFor(`document.querySelectorAll('.nav-item').length >= 4`, { what: '导航', timeout: 60_000 })
      await page.click('.nav-item', '数据中心')
      await page.waitFor(`[...document.querySelectorAll('.list-row')].some(r => r.innerText.includes('订单') && r.innerText.includes('6')) && [...document.querySelectorAll('.list-row')].some(r => r.innerText.includes('日销售'))`, { what: '数据集列表含订单（6 行）与日销售' })
      const cards = await page.eval(`[...document.querySelectorAll('.tool-card')].map(c => c.innerText.split('\\n')[0].trim())`)
      for (const want of ['导入数据', 'SQL 查询', '数据连接', '回收站'])
        assert.ok(cards.some(c => c.includes(want)), `数据中心应有「${want}」卡片：${cards}`)
      await page.click('.tool-card', 'SQL 查询')
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog, .ec-overlay-dialog')].some(d => d.offsetParent && d.innerText.includes('订单'))`, { what: 'SQL 工作台' })
      await page.press('Escape')
      await page.click('.nav-item', '自动化')
      await page.waitFor(`[...document.querySelectorAll('.tool-card')].some(c => c.innerText.includes('操作审计'))`, { what: '自动化页签' })
      const auto = await page.eval(`[...document.querySelectorAll('.tool-card')].map(c => c.innerText.split('\\n')[0].trim())`)
      for (const want of ['定时推送', '外部 AI 接入', '数据权限', '操作审计'])
        assert.ok(auto.some(c => c.includes(want)), `管理员应看到「${want}」：${auto}`)
    })
  })

  it('首页：受限用户（Bob）看到范围提示，没有导入 / 连接 / 回收站入口', async () => {
    await withPage('home-restricted', 'bob', '/excel/home', async (page) => {
      await page.waitFor(`document.body.innerText.includes('你的数据范围被限制在店铺')`, { what: '范围提示条', timeout: 60_000 })
      await page.click('.nav-item', '数据中心')
      await page.waitFor(`[...document.querySelectorAll('.tool-card')].some(c => c.innerText.includes('SQL 查询'))`, { what: '数据中心卡片' })
      const cards = await page.eval(`[...document.querySelectorAll('.tool-card')].map(c => c.innerText.split('\\n')[0].trim())`)
      assert.deepEqual(cards.filter(c => /导入数据|数据连接|回收站/.test(c)), [], `受限用户不应看到：${cards}`)
    })
  })

  /** 当前月份起第 offset 个月的某一天（YYYY-MM-DD，本机时区，与工作表里 TODAY() 一致） */
  function dayOf(offset, day) {
    const d = new Date()
    const t = new Date(d.getFullYear(), d.getMonth() + offset, day)
    return `${t.getFullYear()}-${String(t.getMonth() + 1).padStart(2, '0')}-${String(t.getDate()).padStart(2, '0')}`
  }
  const monthStart = offset => dayOf(offset, 1)
  const parseMoney = v => (v === '-' ? 0 : Number(String(v).replace(/,/g, '')))

  /** 用内置模板建工作簿并在浏览器里打开，等 sheetName 页里 cell 算出数字后返回（公式 DSUMIFS 在服务端汇总，是异步的） */
  async function openTemplate(page, templateId, datasetKey, datasetId, name) {
    const r = await api.call('POST', `/ledger/templates/${templateId}/use`, TOKEN.alice, { body: { name, bindings: { [datasetKey]: { datasetId } }, operator: 'Alice' } })
    assert.equal(r.status, 200, JSON.stringify(r.body))
    assert.deepEqual(r.data.warnings, [], '模板绑定不应有警告')
    await page.goto(`${WEB}/excel?wb=${r.data.workbook.id}`)
    await page.waitFor(`!!window.__univerAPI?.getActiveWorkbook()`, { timeout: 90_000, what: '模板工作簿打开' })
  }

  it('模板打开时填参数：数据来自参数化查询的模板，在使用对话框里只填「店铺 = B」→ 生成 B 店数据集，透视表按它计算', async () => {
    const SQL = 'SELECT 店铺, 金额 FROM 订单 WHERE 店铺 = {{店铺}}'
    const q = await api.call('POST', '/ledger/sql/queries', TOKEN.alice, { body: { name: 'UI分店', sql: SQL, params: [{ name: '店铺', kind: 'text', choices: [] }], values: { 店铺: 'A' }, saveAsDataset: 'UI分店', schedule: { kind: 'manual' } } })
    assert.equal(q.status, 200, JSON.stringify(q.body))
    const pivot = { version: 1, datasetId: q.data.datasetId, datasetName: 'UI分店', request: { rows: [{ column: '店铺' }], columns: [], values: [{ column: '金额', agg: 'sum' }] } }
    const zlib = await import('node:zlib')
    const wbj = zlib.gzipSync(Buffer.from(JSON.stringify({ name: 'UI分店看板', sheetOrder: ['s1'], sheets: { s1: { id: 's1', name: '透视表1', cellData: {}, custom: { pivot } } } })))
    const made = await api.call('POST', `/ledger/workbooks?name=${encodeURIComponent('UI分店看板')}`, TOKEN.alice, { raw: wbj })
    const pub = await api.call('POST', '/ledger/templates', TOKEN.alice, { body: { workbookId: made.data.id, name: 'UI分店模板' } })
    assert.equal(pub.status, 200, JSON.stringify(pub.body))
    await withPage('tpl-params', 'alice', '/excel/home', async (page) => {
      await page.waitFor(`[...document.querySelectorAll('*')].some(e => e.childElementCount === 0 && e.textContent.trim() === '从模板新建')`, { what: '首页' })
      await page.eval(`[...document.querySelectorAll('*')].find(e => e.childElementCount === 0 && e.textContent.trim() === '从模板新建').click()`)
      await page.waitFor(`[...document.querySelectorAll('.card')].some(c => c.innerText.includes('UI分店模板'))`, { what: '模板中心列出自建模板' })
      await page.eval(`[...[...document.querySelectorAll('.card')].find(c => c.innerText.includes('UI分店模板')).querySelectorAll('button')].find(b => b.innerText.trim() === '使用模板').click()`)
      await page.waitFor(`!!document.querySelector('[data-testid="param-values"] input')`, { what: '默认「按参数生成」并显示参数输入' })
      assert.equal(await page.eval(`document.querySelector('[data-testid="param-values"] input').value`), 'A', '带入发布时的参数值')
      assert.match(await page.eval(`document.querySelector('[data-testid="param-mode"]').innerText`), /来自参数化查询「UI分店」/)
      await page.fill('[data-testid="param-values"] input', 'B')
      await page.click('.ec-dialog__footer button', '创建工作簿')
      await page.waitFor(`!!window.__univerAPI?.getActiveWorkbook()?.getActiveSheet()?.getCustomMetadata()?.pivot`, { what: '打开生成的工作簿', timeout: 60_000 })
      const meta = await page.eval(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getCustomMetadata().pivot`)
      assert.notEqual(meta.datasetId, q.data.datasetId, '改用了新的数据集')
      assert.match(meta.datasetName, /店铺=B/, '新数据集按参数命名')
      const sheet = `window.__univerAPI.getActiveWorkbook().getActiveSheet()`
      await page.waitFor(`${sheet}.getRange(2, 0).getValue() === 'B'`, { what: '透视表按 B 店数据计算', timeout: 30_000 })
      assert.equal(Number(String(await page.eval(`${sheet}.getRange(2, 1).getValue()`)).replace(/,/g, '')), 300, 'B 店 100 + 200')
      assert.equal(await page.eval(`${sheet}.getRange(3, 0).getValue()`), '总计', '只有 B 店一行')
    })
  })

  it('内置模板「应收应付对账」：往来余额月报按「期初 + 增加 - 减少 = 期末」算对', async () => {
    // 往来明细：单位 A、B、供应商 S；日期都在月中（避免月初边界）
    const rows = [
      ['客户A', dayOf(-14, 10), '应收', 2000], // 窗口之前：进入第一个月的期初
      ['客户A', dayOf(-3, 10), '应收', 1000],
      ['客户A', dayOf(-2, 10), '收款', 400],
      ['客户B', dayOf(0, 10), '应收', 500],
      ['客户B', dayOf(0, 12), '收款', 100],
      ['供应商S', dayOf(-1, 10), '应付', 800],
      ['供应商S', dayOf(0, 11), '付款', 300],
    ]
    const csv = `往来单位,日期,类型,金额\n${rows.map(r => r.join(',')).join('\n')}\n`
    const dsId = await api.importDataset(TOKEN.alice, '往来明细', csv)
    // 期望：与公式同口径（日期按文本比较）
    const sum = (type, from, to) => rows.filter(r => r[2] === type && (!from || r[1] >= from) && (!to || r[1] < to)).reduce((n, r) => n + r[3], 0)
    const roll = (add, sub) => Array.from({ length: 12 }, (_, i) => {
      const [from, to] = [monthStart(i - 11), monthStart(i - 10)]
      const open = sum(add, null, from) - sum(sub, null, from)
      const inc = sum(add, from, to)
      const dec = sum(sub, from, to)
      return [open, inc, dec, open + inc - dec]
    })
    const wantAr = roll('应收', '收款')
    const wantAp = roll('应付', '付款')
    await withPage('tpl-ar-ap', 'alice', '/excel/home', async (page) => {
      await openTemplate(page, 'builtin_ar_ap', '$partners', dsId, '往来对账测试')
      // 单元格按数字格式显示：金额是 "1,000.00"，零是 "-"；转回数字再比
      const read = async (sheet, r1, c) => parseMoney(await page.eval(`window.__univerAPI.getActiveWorkbook().getSheetByName(${JSON.stringify(sheet)})?.getRange(${r1 - 1}, ${c}).getValue()`))
      await page.waitFor(`(() => { const v = window.__univerAPI.getActiveWorkbook().getSheetByName('往来余额月报')?.getRange(17, 4).getValue(); return String(v).replace(/,/g, '') === '3000.00' })()`, { what: '应收期末余额算出（服务端汇总）', timeout: 90_000 })
      for (let i = 0; i < 12; i++) {
        for (let c = 0; c < 4; c++) {
          assert.equal(await read('往来余额月报', 6 + i, c + 1), wantAr[i][c], `应收第 ${i + 1} 个月 第 ${c + 1} 列`)
          assert.equal(await read('往来余额月报', 24 + i, c + 1), wantAp[i][c], `应付第 ${i + 1} 个月 第 ${c + 1} 列`)
        }
      }
      assert.equal(await read('往来余额月报', 18, 4), 2000 + 1000 - 400 + 500 - 100, '应收合计行的期末余额')
      assert.equal(await read('往来余额月报', 36, 4), 800 - 300, '应付合计行的期末余额')
    })
  })

  it('内置模板「多平台利润表」：利润月报的收入、毛利、净利润与毛利率算对', async () => {
    // 平台、日期、收入、成本、平台费用、推广费用
    const rows = [
      ['抖店', dayOf(0, 5), 1000, 600, 50, 100],
      ['抖店', dayOf(-1, 5), 2000, 1200, 100, 200],
      ['淘宝', dayOf(0, 6), 500, 300, 20, 30],
    ]
    const csv = `平台,日期,销售收入,销售成本,平台费用,推广费用\n${rows.map(r => r.join(',')).join('\n')}\n`
    const dsId = await api.importDataset(TOKEN.alice, '经营明细', csv)
    const month = (offset) => {
      const [from, to] = [monthStart(offset), monthStart(offset + 1)]
      const inRange = rows.filter(r => r[1] >= from && r[1] < to)
      const col = i => inRange.reduce((n, r) => n + r[i], 0)
      const [rev, cost, fee, ads] = [col(2), col(3), col(4), col(5)]
      return { rev, cost, gross: rev - cost, gm: rev ? (rev - cost) / rev : '-', fee, ads, net: rev - cost - fee - ads, nm: rev ? (rev - cost - fee - ads) / rev : '-' }
    }
    await withPage('tpl-pnl', 'alice', '/excel/home', async (page) => {
      await openTemplate(page, 'builtin_platform_pnl', '$pnl', dsId, '利润表测试')
      const raw = (r1, c) => page.eval(`window.__univerAPI.getActiveWorkbook().getSheetByName('利润月报')?.getRange(${r1 - 1}, ${c}).getValue()`)
      const read = async (r1, c) => parseMoney(await raw(r1, c))
      await page.waitFor(`String(window.__univerAPI.getActiveWorkbook().getSheetByName('利润月报')?.getRange(16, 1).getValue()).replace(/,/g, '') === '3500.00'`, { what: '合计销售收入算出（服务端汇总）', timeout: 90_000 })
      for (const [r1, offset] of [[16, 0], [15, -1], [14, -2]]) {
        const w = month(offset)
        assert.equal(await read(r1, 1), w.rev, `${offset} 月 销售收入`)
        assert.equal(await read(r1, 2), w.cost, `${offset} 月 销售成本`)
        assert.equal(await read(r1, 3), w.gross, `${offset} 月 毛利`)
        assert.equal(await read(r1, 5), w.fee, `${offset} 月 平台费用`)
        assert.equal(await read(r1, 6), w.ads, `${offset} 月 推广费用`)
        assert.equal(await read(r1, 7), w.net, `${offset} 月 净利润`)
        assert.equal(await raw(r1, 4), w.gm === '-' ? '-' : `${(w.gm * 100).toFixed(1)}%`, `${offset} 月 毛利率`)
      }
      assert.equal(await read(17, 1), 1000 + 2000 + 500, '合计销售收入')
      assert.equal(await read(17, 7), (1000 - 600 - 50 - 100) + (2000 - 1200 - 100 - 200) + (500 - 300 - 20 - 30), '合计净利润')
    })
  })

  it('内置模板「库存进销存」：库存月报按「期初 + 入库 - 出库 = 期末」算对，透视里的计算项「期末库存」也对', async () => {
    const rows = [
      ['P1', dayOf(-14, 10), '期初', 100], // 窗口之前：进入第一个月的期初
      ['P1', dayOf(-3, 10), '入库', 50],
      ['P1', dayOf(-2, 10), '出库', 30],
      ['P2', dayOf(0, 10), '入库', 20],
      ['P2', dayOf(0, 12), '出库', 5],
      ['P3', dayOf(-1, 10), '期初', 10], // 窗口之内的「期初」录入算本期入库
    ]
    const csv = `商品,日期,单据类型,数量\n${rows.map(r => r.join(',')).join('\n')}\n`
    const dsId = await api.importDataset(TOKEN.alice, '库存流水', csv)
    const sum = (types, from, to) => rows.filter(r => types.includes(r[2]) && (!from || r[1] >= from) && (!to || r[1] < to)).reduce((n, r) => n + r[3], 0)
    const want = Array.from({ length: 12 }, (_, i) => {
      const [from, to] = [monthStart(i - 11), monthStart(i - 10)]
      const open = sum(['期初', '入库'], null, from) - sum(['出库'], null, from)
      const inc = sum(['入库', '期初'], from, to)
      const dec = sum(['出库'], from, to)
      return [open, inc, dec, open + inc - dec]
    })
    await withPage('tpl-stock', 'alice', '/excel/home', async (page) => {
      await openTemplate(page, 'builtin_stock', '$stock', dsId, '库存测试')
      const read = async (sheet, r1, c) => parseMoney(await page.eval(`window.__univerAPI.getActiveWorkbook().getSheetByName(${JSON.stringify(sheet)})?.getRange(${r1 - 1}, ${c}).getValue()`))
      await page.waitFor(`Number(window.__univerAPI.getActiveWorkbook().getSheetByName('库存月报')?.getRange(17, 4).getValue()) === 145`, { what: '期末库存合计算出（服务端汇总）', timeout: 90_000 })
      for (let i = 0; i < 12; i++) {
        for (let c = 0; c < 4; c++)
          assert.equal(await read('库存月报', 6 + i, c + 1), want[i][c], `第 ${i + 1} 个月 第 ${c + 1} 列`)
      }
      assert.equal(await read('库存月报', 18, 4), 100 + 50 - 30 + 20 - 5 + 10, '合计行的期末库存')
      // 透视页签：计算项「期末库存」= 期初 + 入库 - 出库（P2、P3 没有出库 / 入库记录的格子按 0 算）
      await page.eval(`window.__univerAPI.getActiveWorkbook().getSheetByName('商品 × 单据类型').activate(); 0`)
      await page.waitFor(`JSON.stringify(window.__univerAPI.getActiveWorkbook().getSheetByName('商品 × 单据类型').getRange(0, 0, 8, 8).getValues()).includes('期末库存')`, { what: '透视表出现「期末库存」列', timeout: 90_000 })
      const grid = JSON.parse(await page.eval(`JSON.stringify(window.__univerAPI.getActiveWorkbook().getSheetByName('商品 × 单据类型').getRange(0, 0, 8, 8).getValues())`))
      const col = grid.find(r => r.includes('期末库存')).indexOf('期末库存')
      const stock = Object.fromEntries(['P1', 'P2', 'P3'].map(p => [p, parseMoney(grid.find(r => r[0] === p)[col])]))
      assert.deepEqual(stock, { P1: 120, P2: 15, P3: 10 })
    })
  })

  it('内置模板「银行余额调节表」：未达账项四项、调整后余额与差额核对，与独立算出的一致（含「对方日期晚于截至日」的）', async () => {
    // [日期, 金额, 摘要]：A 方银行、B 方企业账。没有对账键，金额完全相同 + 日期窗口 5 天自动勾对
    const bank = [
      ['2026-09-01', 1000, 'b-in1000'],
      ['2026-09-03', -200, 'b-out200'],
      ['2026-09-10', 500, 'b-in500'],
      ['2026-09-15', 100, 'b-in100'], // 与账面 99 手工勾对，差额 +1（手续费）
      ['2026-09-20', -30, 'b-fee30'], // 银行已扣手续费，企业没记：银行已付企业未付
      ['2026-09-29', -800, 'b-chk800'], // 支票 09-26 开出，09-29 兑付：截至 09-30 已两清
      ['2026-10-02', -300, 'b-chk300'], // 支票 09-30 开出，10-02 才兑付：截至 09-30 是未达账项
    ]
    const book = [
      ['2026-09-01', 1000, 'k-in1000'],
      ['2026-09-03', -200, 'k-out200'],
      ['2026-09-12', 500, 'k-in500'],
      ['2026-09-15', 99, 'k-in99'],
      ['2026-09-26', -800, 'k-chk800'],
      ['2026-09-28', 700, 'k-dep700'], // 在途存款：企业已收银行未收
      ['2026-09-30', -300, 'k-chk300'],
    ]
    const csv = rows => `日期,金额,摘要\n${rows.map(r => r.join(',')).join('\n')}\n`
    await api.importDataset(TOKEN.alice, '调节银行流水', csv(bank))
    await api.importDataset(TOKEN.alice, '调节企业账', csv(book))
    const side = (dataset, label) => ({ dataset, label, amount: '金额', sign: 1, date: '日期', keys: [], memo: '摘要' })
    const rc = await api.createRecon(TOKEN.alice, '银行调节对账', { left: side('调节银行流水', '银行'), right: side('调节企业账', '账面'), tolerance: 0, days: 5, groupByKey: false })
    const auto = await api.autoRecon(TOKEN.alice, rc)
    assert.equal(auto.groups, 5, '自动勾对 5 组（1000、-200、500、-800、-300）')
    await api.matchRecon(TOKEN.alice, rc, {
      left: [await api.reconRowKey(TOKEN.alice, rc, 'left', 'b-in100')],
      right: [await api.reconRowKey(TOKEN.alice, rc, 'right', 'k-in99')],
      force: true,
      note: '银行多收 1 元（测试）',
    })

    // 独立算出期望值：按定义直接数，不经过应用里的任何汇总
    const D = '2026-09-30'
    const OPEN = 10000
    const groups = [[0, 0], [1, 1], [2, 2], [3, 3], [5, 4], [6, 6]].map(([bi, ki]) => ({ bi, ki, done: [bank[bi][0], book[ki][0]].sort().at(-1) }))
    const settledDiff = groups.filter(g => g.done <= D).reduce((n, g) => n + bank[g.bi][1] - book[g.ki][1], 0)
    const outstanding = (rows, sideKey) => rows.map((r, i) => ({ r, i })).filter(({ r, i }) => r[0] <= D && !groups.some(g => g[sideKey] === i && g.done <= D))
    const sumBy = (list, f) => list.filter(({ r }) => f(r[1])).reduce((n, { r }) => n + r[1], 0)
    const outA = outstanding(bank, 'bi')
    const outB = outstanding(book, 'ki')
    const want = {
      bankBal: OPEN + bank.filter(r => r[0] <= D).reduce((n, r) => n + r[1], 0),
      bookBal: OPEN + book.filter(r => r[0] <= D).reduce((n, r) => n + r[1], 0),
      bPos: sumBy(outB, v => v > 0),
      bNeg: -sumBy(outB, v => v < 0),
      aPos: sumBy(outA, v => v > 0),
      aNeg: -sumBy(outA, v => v < 0),
    }
    want.bankAdj = want.bankBal + want.bPos - want.bNeg
    want.bookAdj = want.bookBal + want.aPos - want.aNeg
    assert.deepEqual([want.bPos, want.bNeg, want.aPos, want.aNeg], [700, 300, 0, 30], '手算：在途存款 700、未兑付支票 300、银行未入账手续费 30')

    await withPage('tpl-bank-rec', 'alice', '/excel/home', async (page) => {
      await openTemplate(page, 'builtin_bank_rec', '$rec', rc, '调节表测试')
      const sheet = `window.__univerAPI.getActiveWorkbook().getSheetByName('调节表')`
      // 填入截至日期与两个期初余额
      await page.waitFor(`!!${sheet}`, { what: '调节表页签' })
      await page.eval(`${sheet}.getRange('B4').setValue('${D}'); ${sheet}.getRange('B5').setValue(${OPEN}); ${sheet}.getRange('B6').setValue(${OPEN}); 0`)
      const read = async a1 => parseMoney(await page.eval(`${sheet}.getRange('${a1}').getValue()`))
      await page.waitFor(`Number(String(${sheet}.getRange('B13').getValue()).replace(/,/g, '')) === ${want.bankAdj}`, { what: '调整后的银行存款余额算出（服务端汇总）', timeout: 90_000 })
      const got = {
        bankBal: await read('B10'),
        bPos: await read('B11'),
        bNeg: await read('B12'),
        bankAdj: await read('B13'),
        bookBal: await read('B15'),
        aPos: await read('B16'),
        aNeg: await read('B17'),
        bookAdj: await read('B18'),
      }
      assert.deepEqual(got, { bankBal: want.bankBal, bPos: want.bPos, bNeg: want.bNeg, bankAdj: want.bankAdj, bookBal: want.bookBal, aPos: want.aPos, aNeg: want.aNeg, bookAdj: want.bookAdj })
      // 笔数：企业已收未收 1 笔（700）、已付未付 1 笔（-300，对方日期晚于截至日）、银行已付企业未付 1 笔（-30）
      assert.deepEqual([await read('C11'), await read('C12'), await read('C16'), await read('C17')], [1, 1, 0, 1])
      // 核对：差额 = 期初差异（0）+ 勾对时确认的差额（银行 100 - 账面 99 = 1）
      assert.equal(await read('B21'), want.bankAdj - want.bookAdj, '两个调整后余额的差额')
      assert.equal(await read('B21'), 1)
      assert.equal(await read('B22'), 0, '期初差异')
      assert.equal(await read('B23'), settledDiff, '勾对时确认的差额')
      assert.equal(await read('B24'), 0, '无法解释的差额应为 0')
      assert.equal(await page.eval(`${sheet}.getRange('B25').getValue()`), '✓ 差额均已解释')
      // 把截至日期改到 10-02：支票 300 已兑付，不再是未达账项，银行 10-02 的 -300 进入余额
      await page.eval(`${sheet}.getRange('B4').setValue('2026-10-02'); 0`)
      await page.waitFor(`Number(String(${sheet}.getRange('B12').getValue()).replace(/[,-]/g, '') || 0) === 0`, { what: '截至 10-02：已付未付为 0', timeout: 60_000 })
      assert.equal(await read('B24'), 0, '换了截至日期，无法解释的差额仍为 0')
    })
  })

  it('文件菜单「打开 / 新建 / 管理工作簿」：列出可见的工作簿与共享设置', async () => {
    await withPage('workbook-manager', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await menu(page, '.workbook-name', '管理工作簿')
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog')].some(d => d.offsetParent && d.innerText.includes('Alice 利润表') && d.innerText.includes('共享只读看板'))`, { what: '工作簿管理对话框列出两个工作簿' })
      assert.ok(await page.eval(`[...document.querySelectorAll('.ec-dialog .share-chip')].length >= 2`), '每个工作簿有共享设置')
    })
  })

  it('命令面板：SQL 查询对话框', async () => {
    await withPage('sql-dialog', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.press('q', { alt: true, code: 'KeyQ' })
      await page.waitFor(`!!document.querySelector('input[placeholder*="搜索功能"]')`, { what: '命令面板' })
      await page.fill('input[placeholder*="搜索功能"]', 'SQL')
      await page.waitFor(`[...document.querySelectorAll('[data-active]')].some(e => e.innerText.includes('SQL 查询'))`, { what: '搜索到 SQL 查询' })
      await page.press('Enter')
      await page.waitFor(`[...document.querySelectorAll('.ec-dialog, .ec-overlay-dialog')].some(d => d.offsetParent && d.innerText.includes('订单'))`, { what: 'SQL 工作台列出数据集「订单」' })
    })
  })

  it('演示模式：进入后只读全屏，Esc 退出', async () => {
    await withPage('present-mode', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.press('q', { alt: true, code: 'KeyQ' })
      await page.waitFor(`!!document.querySelector('input[placeholder*="搜索功能"]')`, { what: '命令面板' })
      await page.fill('input[placeholder*="搜索功能"]', '演示')
      await page.waitFor(`[...document.querySelectorAll('[data-active]')].some(e => e.innerText.includes('演示'))`, { what: '搜索到演示' })
      await page.press('Enter')
      await page.waitFor(`!!document.querySelector('.excel-workspace.presenting')`, { what: '进入演示模式' })
      assert.equal(await page.eval(`getComputedStyle(document.querySelector('.excel-topbar')).display`), 'none', '演示时隐藏顶栏')
      const gridHidden = () => page.eval(`window.__univerAPI.getActiveWorkbook().getSheets().map(s => s.hasHiddenGridLines())`)
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getSheets().every(s => s.hasHiddenGridLines())`, { what: '演示时隐藏网格线' })
      await page.press('Escape')
      await page.waitFor(`!document.querySelector('.excel-workspace.presenting')`, { what: 'Esc 退出演示' })
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getSheets().every(s => !s.hasHiddenGridLines())`, { what: '退出后网格线恢复' })
      assert.ok((await gridHidden()).every(h => h === false))
      // 进入 / 退出演示不算修改，也不进撤销栈
      await new Promise(r => setTimeout(r, 500))
      assert.ok(!(await page.eval(`document.querySelector('.status-indicator')?.innerText.includes('未保存')`)), '演示进出后不应显示未保存')
    })
  })

  // ── 大数据体验：等待提示、取消、失败重试、跳转到行、拖入导入、透视一键起步、SQL 结果 ──

  /** 3000 行的数据集（店铺 × 金额），首次用到时导入；返回 { id, widths } */
  let bigCache
  async function bigDataset() {
    if (!bigCache) {
      const rows = ['店铺,金额']
      for (let i = 0; i < 3000; i++)
        rows.push(`店${i % 7},${(i * 37) % 1000}.5`)
      const id = await api.importDataset(TOKEN.alice, '体验测试', `${rows.join('\n')}\n`)
      const info = (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.id === id)
      bigCache = { id, widths: info.columns.map(c => c.width), total: 3000 }
    }
    return bigCache
  }

  /** 点大数据表某一列表头的筛选按钮（画布里没有 DOM，按几何位置算：序号列 + 前面各列宽度 + 本列宽度 - 9） */
  async function clickHeaderButton(page, big, colIndex) {
    const canvas = await page.eval(`(() => { const r = document.querySelector('.scroller').parentElement.querySelector('canvas').getBoundingClientRect(); return [r.left, r.top] })()`)
    const numW = Math.max(48, String(big.total).length * 8 + 20)
    const x = canvas[0] + numW + big.widths.slice(0, colIndex + 1).reduce((a, b) => a + b, 0) - 9
    await page.mouseClick(x, canvas[1] + 34)
  }

  async function sortDesc(page, big, colIndex) {
    await clickHeaderButton(page, big, colIndex)
    await page.waitFor(`[...document.querySelectorAll('button.item')].some(e => e.innerText.includes('降序'))`, { what: '列头菜单' })
    await page.click('button.item', '降序')
  }

  const hasMenuItem = (page, text) => page.eval(`[...document.querySelectorAll('button.item')].some(e => e.innerText.includes(${JSON.stringify(text)}))`)

  it('大数据表：查询变慢时显示已用时间与原因说明，取消后条件恢复到上一次', async () => {
    const big = await bigDataset()
    await withPage('busy-cancel', 'alice', `/excel?ds=${big.id}`, async (page) => {
      await page.waitFor(`!!document.querySelector('.statusbar') && !document.querySelector('[data-busy]')`, { timeout: 90_000, what: '大数据表打开' })
      await page.intercept('/cursor', { delayMs: 6000, times: 1 })
      await sortDesc(page, big, 1)
      await page.waitFor(`document.querySelector('[data-busy="loading"]')?.innerText.includes('已用')`, { what: '等待提示出现（含已用时间）' })
      await page.waitFor(`document.querySelector('[data-busy="loading"]')?.innerText.includes('首次排序')`, { timeout: 8000, what: '3 秒后给出原因说明' })
      await page.waitFor(`[...document.querySelectorAll('[data-busy="loading"] button')].some(b => b.innerText.includes('取消'))`, { what: '出现取消按钮' })
      await page.click('[data-busy="loading"] button', '取消')
      await page.waitFor(`!document.querySelector('[data-busy]')`, { what: '取消后提示消失' })
      // 排序条件回到「没有排序」：再打开列头菜单，不会有「清除排序」
      await clickHeaderButton(page, big, 1)
      await page.waitFor(`[...document.querySelectorAll('button.item')].some(e => e.innerText.includes('降序'))`, { what: '再次打开列头菜单' })
      assert.equal(await hasMenuItem(page, '清除排序'), false, '取消后排序条件已还原')
      await page.clearIntercepts()
    })
  })

  it('大数据表：服务端返回「服务繁忙」时显示原因（只出现一次），重试后成功', async () => {
    const big = await bigDataset()
    await withPage('busy-error', 'alice', `/excel?ds=${big.id}`, async (page) => {
      await page.waitFor(`!!document.querySelector('.statusbar') && !document.querySelector('[data-busy]')`, { timeout: 90_000, what: '大数据表打开' })
      await page.intercept('/cursor', { status: 503, body: { code: 503, data: null, msg: '服务繁忙：已有 2 个大计算在进行，排队超过 120 秒，请稍后再试' }, times: 1 })
      await sortDesc(page, big, 1)
      await page.waitFor(`document.querySelector('[data-busy="error"]')?.innerText.includes('服务繁忙')`, { what: '错误提示显示服务端说明' })
      assert.equal(await page.eval(`document.querySelectorAll('.ec-message').length`), 0, '已在表格上显示，不再重复弹全局提示')
      assert.ok(await page.eval(`[...document.querySelectorAll('[data-busy="error"] button')].some(b => b.innerText.includes('还原条件'))`), '有还原条件按钮')
      await page.click('[data-busy="error"] button', '重试')
      await page.waitFor(`!document.querySelector('[data-busy]')`, { what: '重试成功后提示消失' })
      await clickHeaderButton(page, big, 1)
      await page.waitFor(`[...document.querySelectorAll('button.item')].some(e => e.innerText.includes('降序'))`, { what: '再次打开列头菜单' })
      assert.equal(await hasMenuItem(page, '清除排序'), true, '重试后排序条件生效')
      await page.clearIntercepts()
    }, { allowErrors: [/status of 503/, /请求错误/] })
  })

  it('大数据表：Ctrl+G 跳转到行，越界时标红不跳转', async () => {
    const big = await bigDataset()
    await withPage('goto-row', 'alice', `/excel?ds=${big.id}`, async (page) => {
      await page.waitFor(`!!document.querySelector('.statusbar') && !document.querySelector('[data-busy]')`, { timeout: 90_000, what: '大数据表打开' })
      const canvas = await page.eval(`(() => { const r = document.querySelector('.scroller').parentElement.querySelector('canvas').getBoundingClientRect(); return [r.left, r.top] })()`)
      await page.mouseClick(canvas[0] + 120, canvas[1] + 80)
      await page.press('g', { ctrl: true })
      await page.waitFor(`!!document.querySelector('.goto-input')`, { what: '跳转输入框出现' })
      await page.fill('.goto-input', '99999')
      await page.press('Enter')
      await page.waitFor(`document.querySelector('.goto-input')?.classList.contains('bad')`, { what: '越界标红' })
      assert.equal(await page.eval(`document.querySelector('.scroller').scrollTop`), 0, '越界不跳转')
      await page.fill('.goto-input', '2,500')
      await page.press('Enter')
      await page.waitFor(`!document.querySelector('.goto-input')`, { what: '输入框收起' })
      const top = await page.eval(`document.querySelector('.scroller').scrollTop`)
      assert.ok(top > 2000 * 24 && top < 2500 * 24, `滚动到第 2500 行附近：scrollTop=${top}`)
    })
  })

  it('导入数据：拖入文件显示上传进度并进入列映射；不支持的文件被忽略并提示', async () => {
    await withPage('import-drop', 'alice', '/excel/home', async (page) => {
      await page.waitFor(`[...document.querySelectorAll('.tool-card')].some(c => c.innerText.includes('导入数据'))`, { what: '首页卡片' })
      await page.click('.tool-card', '导入数据')
      await page.waitFor(`!!document.querySelector('.drop')`, { what: '导入对话框' })
      await page.intercept('/import/preview', { delayMs: 3000 })
      const drop = `(() => { const dt = new DataTransfer(); dt.items.add(new File(['店铺,金额\\nA,1\\nB,2\\n'], 'drop.csv', { type: 'text/csv' })); dt.items.add(new File(['x'], 'bad.png')); document.querySelector('.drop').dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt })) })()`
      await page.eval(drop)
      await page.waitFor(`[...document.querySelectorAll('.ec-message')].some(e => e.innerText.includes('已忽略 1 个不支持的文件'))`, { what: '提示已忽略 bad.png' })
      await page.waitFor(`document.querySelector('.drop [role="status"]')?.innerText.match(/正在上传|识别/)`, { what: '显示上传 / 识别进度与取消' })
      assert.ok(await page.eval(`[...document.querySelectorAll('.drop button')].some(b => b.innerText.includes('取消'))`), '有取消按钮')
      await page.waitFor(`document.body.innerText.includes('drop.csv') && document.body.innerText.includes('重新选择')`, { timeout: 15_000, what: '进入列映射' })
      await page.clearIntercepts()
    })
  })

  it('透视面板：空白时给出一键起步，点「按店铺看金额合计」得到按店铺的汇总', async () => {
    await withPage('pivot-suggest', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`[...document.querySelectorAll('.data-panel .item')].some(e => e.innerText.includes('订单') && e.innerText.includes('6'))`, { what: '数据面板列出「订单 6」' })
      await page.click('.data-panel .item', '订单')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('6 行')`, { what: '大数据表' })
      await page.click('button', '透视分析')
      await page.waitFor(`!!document.querySelector('[data-testid="pivot-suggestions"]')`, { what: '空白透视表显示一键起步' })
      await page.click('button.suggest', '店铺')
      await page.waitFor(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(2, 0).getValue() === 'A'`, { what: '透视表第一行是 A 店' })
      const sheet = `window.__univerAPI.getActiveWorkbook().getActiveSheet()`
      // Univer 对带格式的数字返回显示文本（如 "30.00"），先转成数字再比
      assert.equal(Number(String(await page.eval(`${sheet}.getRange(2, 1).getValue()`)).replace(/,/g, '')), 30, 'A 店金额 10 + 20')
      assert.equal(await page.eval(`!!document.querySelector('[data-testid="pivot-suggestions"]')`), false, '有了字段后不再显示起步')
    })
  })

  it('透视面板：添加计算值（公式引用已有的值），表里多出一列并按公式算对', async () => {
    await withPage('pivot-calc', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`[...document.querySelectorAll('.data-panel .item')].some(e => e.innerText.includes('订单') && e.innerText.includes('6'))`, { what: '数据面板列出「订单 6」' })
      await page.click('.data-panel .item', '订单')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('6 行')`, { what: '大数据表' })
      await page.click('button', '透视分析')
      await page.waitFor(`!!document.querySelector('[data-testid="pivot-suggestions"]')`, { what: '一键起步' })
      await page.click('button.suggest', '店铺')
      const sheet = `window.__univerAPI.getActiveWorkbook().getActiveSheet()`
      await page.waitFor(`${sheet}.getRange(2, 0).getValue() === 'A'`, { what: '按店铺汇总完成' })
      await page.click('button.add-calc')
      await page.waitFor(`!!document.querySelector('.ec-dialog textarea')`, { what: '计算值对话框' })
      // 点「可引用」里的名称插入，再点运算符，再补数字
      await page.click('.chip-ref', '金额（求和）')
      await page.waitFor(`document.querySelector('.ec-dialog textarea').value === '[金额（求和）]'`, { what: '点名称插入公式' })
      await page.click('.op', '*')
      await page.type('2')
      await page.fill('.ec-dialog input[placeholder*="毛利率"]', '翻倍')
      await page.waitFor(`document.querySelector('.ec-dialog textarea').value === '[金额（求和）] * 2'`, { what: '公式写好' })
      await page.click('.ec-dialog .ec-button--primary', '添加')
      await page.waitFor(`${sheet}.getRange(1, 2).getValue() === '翻倍'`, { what: '透视表多出「翻倍」列' })
      const num = v => Number(String(v).replace(/,/g, ''))
      assert.equal(num(await page.eval(`${sheet}.getRange(2, 1).getValue()`)), 30, 'A 店金额 10 + 20')
      await page.waitFor(`String(${sheet}.getRange(2, 2).getValue()).replace(/,/g, '') === '60.00' || String(${sheet}.getRange(2, 2).getValue()) === '60'`, { what: 'A 店「翻倍」= 60' })
      // 总计行：金额 6330 → 翻倍 12660（逐格按公式算，不是把各行相加后另算）
      const last = await page.eval(`${sheet}.getLastRow()`)
      assert.equal(num(await page.eval(`${sheet}.getRange(${last}, 2).getValue()`)), 12660, '总计行的「翻倍」')
      // 改公式：编辑 → 改成 / 10
      await page.click('.chip span.cursor-pointer', '编辑')
      await page.waitFor(`document.querySelector('.ec-dialog textarea')?.value === '[金额（求和）] * 2'`, { what: '带入原公式' })
      await page.fill('.ec-dialog textarea', '[金额（求和）] / 10')
      await page.click('.ec-dialog .ec-button--primary', '保存')
      await page.waitFor(`Number(String(${sheet}.getRange(2, 2).getValue()).replace(/,/g, '')) === 3`, { what: '改公式后 A 店 = 3' })
    })
  })

  it('透视表手工格式：整列底色、单格加粗、总计数字格式在重算（换排序）后跟着原来的列 / 行走，保存后重新打开仍在', async () => {
    await withPage('pivot-styles', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`[...document.querySelectorAll('.data-panel .item')].some(e => e.innerText.includes('订单') && e.innerText.includes('6'))`, { what: '数据面板列出「订单 6」' })
      await page.click('.data-panel .item', '订单')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('6 行')`, { what: '大数据表' })
      await page.click('button', '透视分析')
      await page.waitFor(`!!document.querySelector('[data-testid="pivot-suggestions"]')`, { what: '一键起步' })
      await page.click('button.suggest', '店铺')
      const sheet = `window.__univerAPI.getActiveWorkbook().getActiveSheet()`
      await page.waitFor(`${sheet}.getRange(2, 0).getValue() === 'A'`, { what: '按店铺汇总完成' })
      const rowOf = async shop => page.eval(`(() => { const s = ${sheet}; for (let r = 2; r <= s.getLastRow(); r++) if (s.getRange(r, 0).getValue() === '${shop}') return r; return -1 })()`)
      const style = (r, c) => page.eval(`JSON.stringify(${sheet}.getRange(${r}, ${c}).getCellStyleData('cell') ?? {})`).then(JSON.parse)
      const yellow = s => JSON.stringify(s.bg ?? '').toLowerCase().includes('ffff00')
      const shops = await page.eval(`(() => { const s = ${sheet}; const out = []; for (let r = 2; r < s.getLastRow(); r++) out.push(s.getRange(r, 0).getValue()); return out })()`)
      const last = await page.eval(`${sheet}.getLastRow()`)
      assert.equal(await page.eval(`${sheet}.getRange(${last}, 0).getValue()`), '总计')
      // 像用户在工具栏上操作一样改格式：金额列的明细格子底色、A 店那一格加粗、总计格子一位小数
      await page.eval(`void ${sheet}.getRange(2, 1, ${shops.length}, 1).setBackgroundColor('#FFFF00')`)
      await page.eval(`void ${sheet}.getRange(2, 1).setFontWeight('bold')`)
      await page.eval(`void ${sheet}.getRange(${last}, 1).setNumberFormat('0.0')`)
      await page.waitFor(`(() => { const st = ${sheet}.getCustomMetadata()?.pivot?.styles; return !!(st?.value && st?.cell && Object.keys(st.cell).length === 2) })()`, { what: '格式按含义记进透视表定义（值级底色 + 两个单格）' })

      // 换成「按名称 Z→A」：重算后 A 店换到最后一行
      await page.eval(`[...document.querySelectorAll('.section')].find(s => s.querySelector('.section-title')?.innerText.trim() === '选项').querySelector('.ec-select__wrapper').click()`)
      await page.waitFor(`[...document.querySelectorAll('.ec-select-dropdown__item')].some(e => e.innerText.trim() === '按名称 Z→A')`, { what: '排序选项' })
      await page.eval(`[...document.querySelectorAll('.ec-select-dropdown__item')].find(e => e.innerText.trim() === '按名称 Z→A').click()`)
      await page.waitFor(`${sheet}.getRange(2, 0).getValue() !== 'A'`, { what: '重算后顺序变了' })
      const check = async (when) => {
        const a = await rowOf('A')
        assert.ok(a > 2, `${when}：A 店不在第一行`)
        const first = await style(2, 1)
        assert.ok(yellow(first), `${when}：换到第一行的店铺仍有金额列底色`)
        assert.ok(!first.bl, `${when}：加粗没有留在原来的位置`)
        const sa = await style(a, 1)
        assert.ok(yellow(sa) && sa.bl, `${when}：A 店那一格跟着 A 店走（底色 + 加粗）`)
        assert.ok(!yellow(await style(1, 1)), `${when}：表头不套用明细的底色`)
        const total = await page.eval(`${sheet}.getLastRow()`)
        assert.equal((await style(total, 1)).n?.pattern, '0.0', `${when}：总计格子的数字格式还在`)
        assert.ok(!yellow(await style(total, 1)), `${when}：总计格子没有底色`)
      }
      await check('重算后')

      // 保存、重新打开：打开时按最新数据重算，格式仍在
      await page.press('s', { ctrl: true })
      await page.waitFor(`document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { what: '保存' })
      await page.eval(`location.reload()`)
      await editorReady(page)
      // 切到透视表页签（打开工作簿后首次切过去时按最新数据重算）
      await page.eval(`(() => { const w = window.__univerAPI.getActiveWorkbook(); w.setActiveSheet(w.getSheets().find(x => x.getCustomMetadata()?.pivot)) })()`)
      await page.waitFor(`(() => { const s = ${sheet}; return !!s.getCustomMetadata()?.pivot })()`, { what: '切到透视表页签' })
      await page.waitFor(`(() => { const s = ${sheet}; for (let r = 2; r <= s.getLastRow(); r++) if (s.getRange(r, 0).getValue() === 'A') return r > 2; return false })()`, { what: '重新打开后透视表已重算' })
      // 再点面板「刷新」：本次打开后的版式里没有上一版的格子，格式只能来自记在定义里的那份
      await page.click('button', '刷新')
      await page.waitFor(`/计算 \\d+ ms/.test(document.body.innerText)`, { what: '面板刷新完成（显示计算耗时）' })
      await check('重新打开后')
    })
  })

  it('透视面板：添加计算项（列字段「单据类型」里新增「期末」），没有出库记录的商品按 0 算', async () => {
    await api.importDataset(TOKEN.alice, '进销存', '商品,单据类型,数量\nP1,期初,10\nP1,入库,5\nP1,出库,3\nP2,期初,7\nP2,入库,1\n')
    await withPage('pivot-items', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`[...document.querySelectorAll('.data-panel .item')].some(e => e.innerText.includes('进销存') && e.innerText.includes('5'))`, { what: '数据面板列出「进销存」' })
      await page.click('.data-panel .item', '进销存')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('5 行')`, { what: '大数据表' })
      await page.click('button', '透视分析')
      await page.waitFor(`!!document.querySelector('[data-testid="pivot-suggestions"]')`, { what: '一键起步' })
      await page.click('button.suggest', '商品')
      const sheet = `window.__univerAPI.getActiveWorkbook().getActiveSheet()`
      await page.waitFor(`${sheet}.getRange(2, 0).getValue() === 'P1'`, { what: '按商品汇总完成' })
      // 把「单据类型」加到列
      await page.eval(`[...document.querySelectorAll('.field')].find(f => f.innerText.includes('单据类型')).querySelector('.add').click()`)
      await page.waitFor(`[...document.querySelectorAll('.ec-dropdown-menu__item')].some(e => e.offsetParent && e.innerText.includes('添加到列'))`, { what: '字段菜单' })
      await page.click('.ec-dropdown-menu__item', '添加到列')
      await page.waitFor(`JSON.stringify(${sheet}.getRange(0, 0, 4, 8).getValues()).includes('期初')`, { what: '列出现「期初」' })
      // 计算项
      await page.click('[data-testid="pivot-items"] button.add-calc')
      await page.waitFor(`!!document.querySelector('.ec-dialog textarea')`, { what: '计算项对话框' })
      assert.ok(await page.eval(`[...document.querySelectorAll('.ec-dialog .chip-ref')].map(e => e.innerText).join(',') === '入库,出库,期初'`), '可引用列出该字段现有的取值')
      await page.fill('.ec-dialog input[placeholder*="期末"]', '期末')
      await page.fill('.ec-dialog textarea', '[期初] + [入库] - [出库]')
      await page.click('.ec-dialog .ec-button--primary', '添加')
      await page.waitFor(`JSON.stringify(${sheet}.getRange(0, 0, 4, 8).getValues()).includes('期末')`, { what: '透视表多出「期末」列' })
      const num = v => Number(String(v).replace(/,/g, ''))
      const grid = JSON.parse(await page.eval(`JSON.stringify(${sheet}.getRange(0, 0, 8, 8).getValues())`))
      const header = grid.find(r => r.includes('期末'))
      const col = header.indexOf('期末')
      const p1 = grid.find(r => r[0] === 'P1')
      const p2 = grid.find(r => r[0] === 'P2')
      assert.equal(num(p1[col]), 12, 'P1 期末 = 10 + 5 - 3')
      assert.equal(num(p2[col]), 8, 'P2 没有出库记录：期末 = 7 + 1 - 0')
      // 普通格子旁有「查看明细」，计算项那一格没有对应的明细所以没有
      const activate = async (c) => {
        await page.eval(`window.__univerAPI.getActiveWorkbook().getActiveSheet().getRange(${grid.indexOf(p1)}, ${c}).activate(); 0`)
        await new Promise(r => setTimeout(r, 900))
        return page.eval(`[...document.querySelectorAll('button')].some(b => b.innerText.includes('查看明细'))`)
      }
      assert.equal(await activate(header.indexOf('入库')), true, '普通格子有「查看明细」')
      assert.equal(await activate(col), false, '计算项的格子不能下钻')
    })
  })

  it('逐笔对账：新建 → 自动勾对 → 手工勾对（有差额须写原因）→ 取消 → 截至日的未达账项 → 结果明细', async () => {
    const bank = '日期,金额,单号,摘要\n2026-09-01,100.00,R1,货款A\n2026-09-02,-50.00,R2,付款B\n2026-09-03,300.00,S1,汇款合并\n2026-09-05,-3.00,,手续费\n2026-10-02,-500.00,R9,支票\n'
    const book = '日期,金额,单号,摘要\n2026-09-01,100.00,R1,凭证1\n2026-09-02,-50.00,R2,凭证2\n2026-09-02,100.00,S1,凭证3a\n2026-09-03,200.00,S1,凭证3b\n2026-09-30,-500.00,R9,支票已开\n2026-09-15,80.00,K1,在途收款\n'
    await api.importDataset(TOKEN.alice, '对账银行', bank)
    await api.importDataset(TOKEN.alice, '对账企业账', book)
    const text = page => page.eval(`document.querySelector('.excel-body')?.innerText.replace(/\\s+/g, ' ') ?? ''`)
    /** 点开第 idx 个下拉（对话框内）并选择含 option 文字的项 */
    const visibleItems = `[...document.querySelectorAll('.ec-select-dropdown__item')].filter(e => e.offsetParent)`
    const pick = async (page, idx, option) => {
      await page.waitFor(`${visibleItems}.length === 0`, { what: '上一个下拉已收起' })
      await page.eval(`document.querySelectorAll('.ec-dialog .ec-select__wrapper')[${idx}].click()`)
      await page.waitFor(`${visibleItems}.some(e => e.innerText.includes(${JSON.stringify(option)}))`, { what: `下拉选项「${option}」` })
      await page.click('.ec-select-dropdown__item', option)
    }
    await withPage('recon', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.waitFor(`[...document.querySelectorAll('.data-panel button')].some(b => b.innerText.includes('新建对账'))`, { what: '数据面板有「新建对账」' })
      await page.click('.data-panel button', '新建对账')
      await page.waitFor(`!!document.querySelector('.ec-dialog')`, { what: '新建对账对话框' })
      await pick(page, 0, '对账银行')
      await pick(page, 1, '对账企业账')
      const guessed = await page.eval(`[...document.querySelectorAll('.ec-dialog .ec-select__selected-item')].map(e => e.innerText.trim()).filter(Boolean).join('|')`)
      assert.ok(guessed.includes('金额') && guessed.includes('日期') && guessed.includes('摘要'), `选了数据集后猜出金额 / 日期 / 摘要列：${guessed}`)
      assert.equal(await page.eval(`document.querySelector('.ec-dialog input[maxlength="40"]').value`), '对账银行对账', '默认名称')
      // 对账键：单号 = 单号；勾上「合计相等的多笔也自动勾对」
      await page.click('.ec-dialog button', '添加对账键')
      await page.waitFor(`document.querySelectorAll('.ec-dialog .ec-select__wrapper').length === 10`, { what: '出现一行对账键' })
      await pick(page, 8, '单号')
      await pick(page, 9, '单号')
      await page.click('.ec-dialog .ec-checkbox', '合计相等的多笔')
      await page.click('.ec-dialog button', '检查两侧数据')
      await page.waitFor(`document.querySelector('.ec-dialog')?.innerText.includes('5 行') && document.querySelector('.ec-dialog')?.innerText.includes('6 行')`, { what: '预览两侧行数' })
      assert.ok(await page.eval(`document.querySelector('.ec-dialog').innerText.includes('对账键为空')`), '提示手续费那行对账键为空')
      await page.click('.ec-dialog .ec-button--primary', '创建对账')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('对账工作台') === false && document.querySelector('.excel-body')?.innerText.includes('自动勾对')`, { what: '进入对账工作台' })
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('0 组')`, { what: '汇总出现（还没勾对）' })

      // 自动勾对：先试算 → 确认文案 → 执行
      await page.click('.excel-body button', '自动勾对')
      await page.waitFor(`document.querySelector('.ec-message-box')?.innerText.includes('将自动勾对')`, { what: '试算后的确认框' })
      const confirm = await page.eval(`document.querySelector('.ec-message-box').innerText`)
      assert.ok(confirm.includes('4 组') && confirm.includes('金额相同 3 组') && confirm.includes('合计相等的多笔 1 组') && confirm.includes('9 笔'), `试算：${confirm}`)
      await page.click('.ec-message-box button', '执行勾对')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('4 组')`, { what: '勾对完成' })
      const body = await text(page)
      assert.ok(body.includes('自动 3 容差 0 汇总 1 手工 0'), `各轮组数：${body.slice(0, 400)}`)
      // 两侧各剩一笔：银行手续费 -3.00，账面在途收款 80.00
      await page.waitFor(`[...document.querySelectorAll('.ec-table__body tr')].map(r => r.innerText.replace(/\\s+/g, ' ')).some(t => t.includes('手续费')) && [...document.querySelectorAll('.ec-table__body tr')].some(r => r.innerText.includes('在途收款'))`, { what: '两侧各剩一笔' })

      // 手工勾对：选两边各一笔，差额 -83 超过容差 → 必须写原因
      await page.eval(`[...document.querySelectorAll('.ec-table__body tr')].find(r => r.innerText.includes('手续费')).click()`)
      await page.eval(`[...document.querySelectorAll('.ec-table__body tr')].find(r => r.innerText.includes('在途收款')).click()`)
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('差额 -83.00')`, { what: '勾对条显示差额 -83.00' })
      await page.click('.excel-body button', '勾对（有差额）')
      await page.waitFor(`document.querySelector('.ec-message-box')?.innerText.includes('请写明差额的原因')`, { what: '要求写原因' })
      await page.click('.ec-message-box button', '确认勾对')
      assert.ok(await page.eval(`document.querySelector('.ec-message-box')?.innerText.includes('请写明原因')`), '原因为空时不能确认')
      await page.fill('.ec-message-box input', '手续费与在途款，测试核销')
      await page.click('.ec-message-box button', '确认勾对')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('5 组') && document.querySelector('.excel-body')?.innerText.includes('1 组有差额')`, { what: '手工勾对完成：5 组、1 组有差额' })
      assert.ok((await text(page)).includes('没有未勾对的了'), '两边都勾完了')

      // 已勾对页：看到备注，取消这一组
      await page.click('.excel-body .ec-tabs__item', '已勾对')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('手续费与在途款，测试核销')`, { what: '勾对组列表显示备注' })
      await page.click('.excel-body .group-card button', '取消勾对')
      await page.waitFor(`document.querySelector('.ec-message-box')?.innerText.includes('取消勾对组')`, { what: '取消确认' })
      await page.click('.ec-message-box button', '取消勾对')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('4 组')`, { what: '回到 4 组' })
      await page.click('.excel-body .ec-tabs__item', '未勾对')

      // 截至 2026-09-30 的未达账项：银行 -3（手续费）；账面 +80（在途）和 -500（9-30 已开支票，银行 10-02 才扣）
      await page.eval(`document.querySelector('.excel-body .ec-date-editor input').focus()`)
      await page.type('2026-09-30')
      await page.press('Enter')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('截至 2026-09-30')`, { what: '汇总按截至日重算' })
      const cards = await page.eval(`[...document.querySelectorAll('.excel-body .card')].map(c => c.innerText.replace(/\\s+/g, ' '))`)
      assert.ok(/增加 0 笔 0\.00/.test(cards[0]) && /减少 1 笔 -3\.00/.test(cards[0]), `银行未达：${cards[0]}`)
      assert.ok(/增加 1 笔 80\.00/.test(cards[1]) && /减少 1 笔 -500\.00/.test(cards[1]), `账面未达（含对方日期晚于截至日的 -500）：${cards[1]}`)

      // 结果明细：数据表 10 + ... 行，能回到工作台
      await page.click('.excel-body button', '结果明细')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('11 行') && document.querySelector('.excel-body')?.innerText.includes('← 对账工作台')`, { what: '结果明细 11 行（5 + 6）' })
      await page.click('.excel-body button', '← 对账工作台')
      await page.waitFor(`document.querySelector('.excel-body')?.innerText.includes('自动勾对')`, { what: '回到工作台' })
      await page.screenshot(path.join(ARTIFACTS, 'recon-final.png'))
    })
  })

  it('SQL 工作台：结果表边框正常（不是浏览器默认的 3px），出错只在结果区显示一次', async () => {
    await withPage('sql-result', 'alice', '/excel/home', async (page) => {
      await page.waitFor(`[...document.querySelectorAll('.nav-item')].some(e => e.innerText.includes('数据中心'))`, { what: '首页导航' })
      await page.click('.nav-item', '数据中心')
      await page.click('.tool-card', 'SQL 查询')
      await page.waitFor(`!!document.querySelector('.sql-dialog textarea')`, { what: 'SQL 工作台' })
      await page.fill('.sql-dialog textarea', 'SELECT 店铺, sum(金额) AS 合计 FROM 订单 GROUP BY 店铺 ORDER BY 店铺')
      await page.click('.sql-dialog button', '运行')
      await page.waitFor(`document.querySelectorAll('.grid-wrap tbody tr').length === 4`, { what: '4 家店铺的结果' })
      const border = await page.eval(`(() => { const cs = getComputedStyle(document.querySelector('.grid-wrap th:nth-child(2)')); return [cs.borderTopWidth, cs.borderLeftWidth] })()`)
      assert.deepEqual(border, ['0px', '0px'], '表头只有下边框和右边框')
      assert.ok(await page.eval(`document.querySelector('.result .status')?.innerText.includes('下载 CSV')`), '结果区有复制 / 下载')
      await page.fill('.sql-dialog textarea', 'SELECT 不存在的列 FROM 订单')
      await page.click('.sql-dialog button', '运行')
      await page.waitFor(`document.querySelector('.result')?.innerText.includes('出错了')`, { what: '结果区显示错误' })
      assert.equal(await page.eval(`document.querySelectorAll('.ec-message').length`), 0, '不再额外弹全局提示')
    }, { allowErrors: [/status of 400|Bad Request/i, /请求错误/] })
  })

  it('SQL 编辑器：语法高亮、表名 / 列名补全（含别名与中文）、Tab 确认、Ctrl+Enter 运行', async () => {
    await withPage('sql-editor', 'alice', '/excel/home', async (page) => {
      await page.waitFor(`[...document.querySelectorAll('.nav-item')].some(e => e.innerText.includes('数据中心'))`, { what: '首页导航' })
      await page.click('.nav-item', '数据中心')
      await page.click('.tool-card', 'SQL 查询')
      await page.waitFor(`!!document.querySelector('.sql-editor textarea')`, { what: 'SQL 编辑器' })
      await page.eval(`(() => { const t = document.querySelector('.sql-editor textarea'); t.focus(); t.select() })()`)
      await page.type('SELECT 店铺, sum(金额) AS 合计 FROM 订')
      await page.waitFor(`[...document.querySelectorAll('.sql-editor .popup .row')].some(r => r.innerText.includes('订单'))`, { what: '弹出数据集补全「订单」' })
      assert.ok(await page.eval(`document.querySelectorAll('.sql-editor .tk-keyword').length >= 3 && !!document.querySelector('.sql-editor .tk-function')`), '关键字与函数有着色')
      await page.press('Tab')
      await page.waitFor(`document.querySelector('.sql-editor textarea').value.endsWith('FROM 订单') && !document.querySelector('.sql-editor .popup')`, { what: 'Tab 确认并关闭补全' })
      await page.type(' o WHERE o.')
      await page.waitFor(`[...document.querySelectorAll('.sql-editor .popup .row')].map(r => r.innerText).join('|').includes('店铺') && [...document.querySelectorAll('.sql-editor .popup .row')].some(r => r.innerText.includes('金额'))`, { what: '别名 o. 之后列出订单的列' })
      await page.press('ArrowDown')
      await page.press('Enter')
      await page.waitFor(`document.querySelector('.sql-editor textarea').value.endsWith('WHERE o.金额')`, { what: '方向键选「金额」、回车确认' })
      await page.type(' > 50 GROUP BY 店铺 ORDER BY 店铺')
      await page.press('Escape')
      assert.ok(await page.eval(`!!document.querySelector('.sql-dialog')`), 'Esc 只关补全，不关对话框')
      await page.press('Enter', { ctrl: true })
      await page.waitFor(`document.querySelectorAll('.grid-wrap tbody tr').length === 3`, { what: 'Ctrl+Enter 运行（金额 > 50 的 B、C、D 三家）' })
      // 字符串里不补全
      await page.eval(`(() => { const t = document.querySelector('.sql-editor textarea'); t.focus(); t.select() })()`)
      await page.type('SELECT \'订')
      await new Promise(r => setTimeout(r, 400))
      assert.equal(await page.eval(`!!document.querySelector('.sql-editor .popup')`), false, '字符串里不弹补全')
    })
  })

  it('SQL 参数：{{名称}} 出现参数栏，按类型代入运行（注入式的值只是文本），保存后「生成实例」得到独立的数据集', async () => {
    await api.importDataset(TOKEN.alice, '参数订单', '店铺,金额\nA,10\nA,20\nA,300\nB,100\nB,400\n')
    await withPage('sql-params', 'alice', '/excel/home', async (page) => {
      await page.waitFor(`[...document.querySelectorAll('.nav-item')].some(e => e.innerText.includes('数据中心'))`, { what: '首页导航' })
      await page.click('.nav-item', '数据中心')
      await page.click('.tool-card', 'SQL 查询')
      await page.waitFor(`!!document.querySelector('.sql-editor textarea')`, { what: 'SQL 编辑器' })
      await page.eval(`(() => { const t = document.querySelector('.sql-editor textarea'); t.focus(); t.select() })()`)
      await page.type('SELECT 金额 FROM 参数订单 WHERE 店铺 = {{店铺}} AND 金额 >= {{下限}} ORDER BY 金额')
      await page.press('Escape')
      await page.waitFor(`[...document.querySelectorAll('[data-testid="sql-params"] .name')].map(e => e.innerText).join(',') === '店铺,下限'`, { what: '参数栏列出 店铺、下限' })
      const input = (i, v) => page.eval(`(() => { const el = document.querySelectorAll('[data-testid="sql-params"] .param input.ec-input__inner')[${i}]; const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(el, ${JSON.stringify(v)}); el.dispatchEvent(new Event('input', { bubbles: true })) })()`)
      // 没填值：不发请求，说明缺哪个
      await page.click('.sql-dialog button', '运行')
      await page.waitFor(`document.querySelector('.sql-dialog')?.innerText.includes('请填写参数「店铺」')`, { what: '提示缺参数' })
      // 「下限」改成数字类型
      await page.eval(`document.querySelectorAll('[data-testid="sql-params"] .ec-select__wrapper')[1].click()`)
      await page.waitFor(`[...document.querySelectorAll('.ec-select-dropdown__item')].some(e => e.offsetParent && e.innerText.includes('数字'))`, { what: '类型下拉' })
      await page.click('.ec-select-dropdown__item', '数字')
      await input(0, 'A')
      await input(1, '15')
      await page.click('.sql-dialog button', '运行')
      await page.waitFor(`document.querySelectorAll('.grid-wrap tbody tr').length === 2`, { what: '店铺 A 且金额 ≥ 15：20、300' })
      // 注入式的值：只是一个普通文本，查不到行也不出错
      await input(0, `A' OR '1'='1`)
      await page.click('.sql-dialog button', '运行')
      await page.waitFor(`document.querySelectorAll('.grid-wrap tbody tr').length === 0 && !document.querySelector('.sql-dialog')?.innerText.includes('错误')`, { what: '没有行、没有错误' })
      await input(0, 'A')
      await page.click('.sql-dialog button', '运行')
      await page.waitFor(`document.querySelectorAll('.grid-wrap tbody tr').length === 2`, { what: '改回 A' })
      // 保存 → 生成实例（B 店）
      await page.click('.sql-dialog button', '保存查询')
      await page.waitFor(`!!document.querySelector('.ec-dialog input[maxlength="60"]')`, { what: '保存对话框' })
      await page.fill('.ec-dialog input[maxlength="60"]', '分店金额')
      await page.click('.ec-dialog .ec-button--primary', '保存')
      await page.waitFor(`[...document.querySelectorAll('.sql-dialog .saved')].some(e => e.innerText.includes('分店金额') && e.innerText.includes('参数'))`, { what: '侧栏出现参数化查询' })
      await page.click('.sql-dialog button', '生成实例')
      await page.waitFor(`document.querySelector('.param-instance')?.innerText.includes('生成实例')`, { what: '生成实例对话框' })
      const filled = await page.eval(`[...document.querySelectorAll('.param-instance input.ec-input__inner')].slice(0, 2).map(e => e.value).join('|')`)
      assert.equal(filled, 'A|15', '带入当前的参数值')
      await page.eval(`(() => { const el = document.querySelectorAll('.param-instance input.ec-input__inner')[0]; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, 'B'); el.dispatchEvent(new Event('input', { bubbles: true })) })()`)
      await page.click('.param-instance .ec-button--primary', '生成数据集')
      await page.waitFor(`[...document.querySelectorAll('.sql-dialog .saved.indent')].some(e => e.innerText.includes('分店金额（店铺=B，下限=15）'))`, { what: '实例缩进显示在模板下面' })
      const ds = (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.name === '分店金额（店铺=B，下限=15）')
      assert.equal(ds?.rowCount, 2, 'B 店金额 ≥ 15：100、400')
    })
  })

  it('填报：载入现有数据 → 选中一行「删除选中的行」→ 数据集少一行、表格行同步删除 → 提交记录里撤销，行回到数据集', async () => {
    const made = await api.call('POST', '/ledger/datasets/input', TOKEN.alice, { body: { name: 'UI预算', columns: [{ title: '店铺', kind: 'text' }, { title: '月份', kind: 'text' }, { title: '预算', kind: 'money' }], primaryKey: ['店铺', '月份'] } })
    const dsId = made.data
    await api.call('POST', `/ledger/datasets/${dsId}/writeback`, TOKEN.alice, { body: { mode: 'upsert', columns: ['店铺', '月份', '预算'], rows: [['A', '2026-10', 1], ['B', '2026-10', 2], ['C', '2026-10', 3]] } })
    const count = async () => (await api.call('POST', `/ledger/datasets/${dsId}/aggregate`, TOKEN.alice, { body: { calls: [{ func: 'count', criteria: [] }] } })).data.values[0]
    assert.equal(await count(), 3)
    await withPage('writeback-delete', 'alice', `/excel?wb=${wb.alice}`, async (page) => {
      await editorReady(page)
      await page.press('q', { alt: true, code: 'KeyQ' })
      await page.fill('input[placeholder*="搜索功能"]', '填报')
      await page.waitFor(`[...document.querySelectorAll('[data-active]')].some(e => e.innerText.includes('填报'))`, { what: '搜索结果含「填报」' })
      await page.press('Enter')
      await page.waitFor(`!!document.querySelector('.wb-panel')`, { what: '填报面板' })
      await page.click('.wb-panel button', '新建填报区')
      await page.waitFor(`!!document.querySelector('.wb-panel .ec-select__wrapper')`, { what: '新建表单' })
      await page.eval(`document.querySelector('.wb-panel .ec-select__wrapper').click()`)
      await page.waitFor(`[...document.querySelectorAll('.ec-select-dropdown__item')].some(e => e.offsetParent && e.innerText.includes('UI预算'))`, { what: '选项「UI预算」' })
      await page.click('.ec-select-dropdown__item', 'UI预算')
      await page.waitFor(`document.querySelector('.wb-panel')?.innerText.includes('数据集登记的主键')`, { what: '带出主键' })
      await page.click('.wb-panel button', '放置填报区')
      // A1 起已有内容：确认覆盖表头
      await page.waitFor(`document.querySelector('.ec-message-box')?.innerText.includes('已有内容') || [...document.querySelectorAll('.wb-panel button')].some(b => b.innerText.includes('提交到数据集'))`, { what: '放置完成或要求确认' })
      if (await page.eval(`!!document.querySelector('.ec-message-box')`))
        await page.click('.ec-message-box button', '继续')
      await page.waitFor(`[...document.querySelectorAll('.wb-panel button')].some(b => b.innerText.includes('提交到数据集'))`, { what: '填报区放好' })
      // 载入现有数据（3 行）
      await page.click('.wb-panel button', '载入现有数据')
      await page.waitFor(`[...document.querySelectorAll('.wb-panel button')].some(b => b.innerText.trim() === '载入')`, { what: '载入表单' })
      await page.eval(`[...document.querySelectorAll('.wb-panel button')].find(b => b.innerText.trim() === '载入').click()`)
      // 示例工作簿里 A2:B3 本来就有内容，算作填报区的数据行：载入要先确认替换
      await page.waitFor(`document.querySelector('.ec-message-box')?.innerText.includes('替换填报区里现有的内容')`, { what: '确认替换' })
      await page.click('.ec-message-box button', '替换')
      const sheet = `window.__univerAPI.getActiveWorkbook().getActiveSheet()`
      const shops = () => page.eval(`JSON.stringify(${sheet}.getRange(1, 0, 4, 1).getValues().map(r => r[0]))`)
      await page.waitFor(`${sheet}.getRange(3, 0).getValue() === 'C'`, { what: '载入 3 行（A、B、C）' })
      assert.equal(await shops(), '["A","B","C",null]')
      // 选中 B 那一行（表头在第 1 行，A 在第 2 行，B 在第 3 行 = 索引 2）并删除
      await page.eval(`${sheet}.getRange(2, 0, 1, 3).activate(); 0`)
      await new Promise(r => setTimeout(r, 400))
      await page.click('.wb-panel button', '删除选中的行')
      await page.waitFor(`document.querySelector('.ec-message-box')?.innerText.includes('删除选中的 1 行（B / 2026-10）')`, { what: '确认框列出主键' })
      await page.click('.ec-message-box button', '删除')
      await page.waitFor(`${sheet}.getRange(2, 0).getValue() === 'C'`, { what: '表格里 B 行已删除，C 上移' })
      assert.equal(await shops(), '["A","C",null,null]')
      assert.equal(await count(), 2, '数据集里少了 B')
      // 提交记录：删除 1，撤销后行回到数据集
      await page.waitFor(`[...document.querySelectorAll('.wb-panel .rec')].some(e => e.innerText.includes('删除 1'))`, { what: '提交记录有「删除 1」' })
      await page.click('.wb-panel .rec button', '撤销')
      await page.waitFor(`document.querySelector('.ec-message-box')?.innerText.includes('插回数据集')`, { what: '撤销确认写明「插回」' })
      await page.click('.ec-message-box button', '撤销')
      await page.waitFor(`document.querySelector('.ec-message')?.innerText.includes('插回 1 行')`, { what: '撤销完成' })
      assert.equal(await count(), 3, '撤销后回到 3 行')
    })
  })
})
