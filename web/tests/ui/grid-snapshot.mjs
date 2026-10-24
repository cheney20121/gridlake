// 大数据表画布（BigGrid）的像素快照：改画布绘制 / 拆分文件前后各跑一次，比对每个状态的画面是否逐像素一致。
//   node tests/ui/grid-snapshot.mjs <输出目录>          起服务、造数据、按几个状态截取画布，写 PNG 与 hashes.json
//   node tests/ui/grid-snapshot.mjs --compare <目录A> <目录B>   逐状态比对哈希，有差异时退出码为 1
// 画面与本机字体有关，所以不把基线提交进仓库：同一台机器上「改之前」「改之后」各跑一次再比。
// 与 test:ui 用同一组端口，不能同时运行
import { Buffer } from 'node:buffer'
import crypto from 'node:crypto'
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { findChrome, launchChrome } from './cdp.mjs'
import { api, startServices, TOKEN, WEB } from './infra.mjs'

const sleep = ms => new Promise(r => setTimeout(r, ms))

if (process.argv[2] === '--compare') {
  const [a, b] = [process.argv[3], process.argv[4]].map(d => JSON.parse(fs.readFileSync(path.join(d, 'hashes.json'), 'utf8')))
  const names = [...new Set([...Object.keys(a), ...Object.keys(b)])]
  const diff = names.filter(n => a[n] !== b[n])
  for (const n of names)
    console.log(`${diff.includes(n) ? '✖ 不同' : '✔ 一致'}  ${n}`)
  console.log(diff.length ? `\n${diff.length} / ${names.length} 个状态的画面有差异` : `\n全部 ${names.length} 个状态逐像素一致`)
  process.exit(diff.length ? 1 : 0)
}

const out = process.argv[2]
if (!out || !findChrome()) {
  console.error('用法：node tests/ui/grid-snapshot.mjs <输出目录>（需要本机有 Chrome）')
  process.exit(2)
}
fs.mkdirSync(out, { recursive: true })

function csv() {
  const rows = ['订单号,店铺,下单时间,数量,金额']
  for (let i = 0; i < 400; i++) {
    const amount = i % 11 === 0 ? '' : ((i * 37) % 900 + 0.25) * (i % 7 === 3 ? -1 : 1)
    rows.push(`O${String(i).padStart(5, '0')},店铺${i % 9},2026-0${1 + (i % 9)}-${String(1 + (i % 28)).padStart(2, '0')} 1${i % 10}:30:00,${1 + (i % 6)},${amount}`)
  }
  return `${rows.join('\n')}\n`
}

async function main() {
  const services = await startServices()
  const browser = await launchChrome()
  const hashes = {}
  try {
    await api.registerUsers()
    const id = await api.importDataset(TOKEN.alice, '快照数据', csv())
    const info = (await api.call('GET', '/ledger/datasets', TOKEN.alice)).data.find(d => d.id === id)
    const widths = info.columns.map(c => c.width)
    const page = await browser.newPage()
    await page.setCookie('gridlake-token', TOKEN.alice, WEB)
    await page.goto(`${WEB}/excel?ds=${id}`)
    await page.waitFor(`!!document.querySelector('.statusbar') && !document.querySelector('[data-busy]') && !!document.querySelector('.scroller')`, { timeout: 180_000 })
    await sleep(1500)

    const rect = () => page.eval(`(() => { const r = document.querySelector('.scroller').parentElement.querySelector('canvas').getBoundingClientRect(); return [r.left, r.top, r.width, r.height] })()`)
    const numW = Math.max(48, String(400).length * 8 + 20)
    const cellXY = async (row, col) => {
      const [l, t] = await rect()
      return [l + numW + widths.slice(0, col).reduce((a, b) => a + b, 0) + 20, t + 20 + 28 + row * 24 + 12]
    }
    async function snap(name) {
      await sleep(700)
      const url = await page.eval(`document.querySelector('.scroller').parentElement.querySelector('canvas').toDataURL('image/png')`)
      const buf = Buffer.from(url.split(',')[1], 'base64')
      fs.writeFileSync(path.join(out, `${name}.png`), buf)
      hashes[name] = crypto.createHash('sha256').update(buf).digest('hex')
      console.log(`${name}  ${hashes[name].slice(0, 12)}`)
    }

    await snap('01-初始')
    // 行还没加载出来时的骨架：拦截取行请求，再滚动到还没缓存过的位置（第 2 段）
    await page.intercept('/rows', { delayMs: 3500 })
    await page.eval(`document.querySelector('.scroller').scrollTop = 24 * 330`)
    await snap('02-骨架占位')
    await sleep(3500)
    await page.clearIntercepts()
    await snap('02b-骨架之后的真实数据')
    await page.eval(`document.querySelector('.scroller').scrollTop = 24 * 150`)
    await snap('02c-滚动到第150行')
    await page.eval(`document.querySelector('.scroller').scrollTop = 0`)
    await sleep(300)
    // 选区：点一格，再 Shift + 方向键扩展
    const [x, y] = await cellXY(2, 1)
    await page.mouseClick(x, y)
    await snap('03-单格选中')
    for (let i = 0; i < 3; i++) await page.press('ArrowDown', { shift: true })
    await page.press('ArrowRight', { shift: true })
    await snap('04-多格选区')
    await page.press('a', { ctrl: true })
    await snap('05-全选')
    await page.press('Home', { ctrl: true })
    await sleep(300)
    // 横向滚动
    await page.eval(`document.querySelector('.scroller').scrollLeft = 120`)
    await snap('06-横向滚动')
    await page.eval(`document.querySelector('.scroller').scrollLeft = 0`)
    // 排序（金额降序）：表头筛选按钮
    const [l, t] = await rect()
    const btn = col => [l + numW + widths.slice(0, col + 1).reduce((a, b) => a + b, 0) - 9, t + 34]
    await page.mouseClick(...btn(4))
    await page.waitFor(`[...document.querySelectorAll('button.item')].some(e => e.innerText.includes('降序'))`)
    await page.click('button.item', '降序')
    await page.waitFor(`!document.querySelector('[data-busy]')`)
    await snap('07-金额降序（含负数、空值）')
    // 筛选（店铺：只看店铺1）
    await page.mouseClick(...btn(1))
    await page.waitFor(`document.querySelectorAll('.menu .value-row').length > 0`)
    await page.eval(`(() => { const rows = [...document.querySelectorAll('.menu .value-row')]; const only = rows.find(r => r.innerText.includes('店铺1')); document.querySelector('.menu button.text-\\\\[\\\\#217346\\\\]')?.click(); })()`)
    await page.eval(`(() => { for (const r of document.querySelectorAll('.menu .value-row input')) { if (r.checked) r.click() } const r = [...document.querySelectorAll('.menu .value-row')].find(x => x.innerText.includes('店铺1')); r.querySelector('input').click() })()`)
    await page.click('.menu button', '确定')
    await page.waitFor(`!document.querySelector('[data-busy]')`)
    await snap('08-店铺筛选（漏斗图标）')
    // 窄窗口
    await page.send('Emulation.setDeviceMetricsOverride', { width: 800, height: 600, deviceScaleFactor: 1, mobile: false })
    await sleep(800)
    await snap('10-窄窗口')
    fs.writeFileSync(path.join(out, 'hashes.json'), JSON.stringify(hashes, null, 2))
    console.log(`\n已写入 ${out}（${Object.keys(hashes).length} 个状态）`)
  }
  finally {
    await browser.close()
    services.stop()
  }
}

main()
