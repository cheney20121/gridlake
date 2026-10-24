import { Buffer } from 'node:buffer'
// 重新生成 README 里的两张截图（docs/images/big-grid.png、pivot.png）：
//   LEDGER_BIN=../server/target/release/gridlake node tests/ui/readme-shots.mjs [行数，默认 2000000]
// 起一套测试服务 → 用 scripts/seed-demo.py 造 N 行合成订单 → 打开数据集和透视表截图（1440 × 900）
import { spawnSync } from 'node:child_process'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import zlib from 'node:zlib'
import { launchChrome } from './cdp.mjs'
import { api, API, startServices, TOKEN, WEB } from './infra.mjs'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const OUT = path.join(ROOT, 'docs/images')
const rows = process.argv[2] ?? '2000000'
const sleep = ms => new Promise(r => setTimeout(r, ms))

async function main() {
  const services = await startServices()
  try {
    await api.registerUsers()
    const seed = spawnSync('python3', [path.join(ROOT, 'scripts/seed-demo.py'), '--url', API.replace(/\/api$/, ''), '--rows', rows, '--token', TOKEN.alice], { stdio: 'inherit' })
    if (seed.status !== 0)
      throw new Error('造数据失败')
    const datasets = (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data
    const orders = datasets.find(d => d.name === '订单')
    const pivot = { version: 1, datasetId: orders.id, datasetName: '订单', request: { rows: [{ column: '渠道' }], columns: [{ column: '状态' }], values: [{ column: '金额', agg: 'sum' }] } }
    const wbj = zlib.gzipSync(Buffer.from(JSON.stringify({ name: '经营分析', sheetOrder: ['s1'], sheets: { s1: { id: 's1', name: '透视表1', cellData: {}, custom: { pivot } } } })))
    const made = await api.call('POST', `/ledger/workbooks?name=${encodeURIComponent('经营分析')}`, TOKEN.alice, { raw: wbj })
    const browser = await launchChrome()
    try {
      const page = await browser.newPage()
      await page.setCookie('gridlake-token', TOKEN.alice, WEB)
      await page.goto(`${WEB}/excel?wb=${made.data.id}`)
      await page.waitFor(`!!window.__univerAPI?.getActiveWorkbook() && document.querySelector('.status-indicator')?.innerText.includes('已保存')`, { timeout: 120_000, what: '编辑器加载' })
      await page.waitFor(`[...document.querySelectorAll('.data-panel .item')].some(e => e.innerText.includes('订单'))`, { what: '数据面板' })
      // 数据集大表
      await page.click('.data-panel .item', '订单')
      await page.waitFor(`/2,000,000 行/.test(document.body.innerText) && document.body.innerText.includes('店铺')`, { timeout: 60_000, what: '大表数据' })
      await sleep(1500)
      await page.screenshot(path.join(OUT, 'big-grid.png'))
      // 透视表：回到工作表，打开字段面板
      await page.eval(`[...document.querySelectorAll('button, .tab, .item, span')].find(e => e.innerText?.trim() === '透视表1')?.click()`)
      await sleep(2000)
      await page.press('q', { alt: true })
      await page.fill('input[placeholder*="搜索功能"]', '透视')
      await sleep(600)
      await page.press('Enter')
      await page.waitFor(`document.querySelector('.side-panel .panel-title')?.innerText.includes('数据透视表字段')`, { what: '透视表字段面板' })
      await sleep(2500)
      await page.screenshot(path.join(OUT, 'pivot.png'))
      console.log('已写入', OUT)
    }
    finally {
      await browser.close()
    }
  }
  finally {
    services.stop()
  }
}

main().catch((e) => {
  console.error(e)
  process.exit(1)
})
