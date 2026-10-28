// 手动查看界面用：起好模拟网关 + 表格服务 + 前端，造一批演示数据后一直运行（Ctrl+C 退出）。
//   node tests/ui/serve.mjs
// 然后在浏览器里先设置 Cookie：document.cookie = 'gridlake-token=tok-alice; path=/'，再打开 http://127.0.0.1:5291/excel/home
// 注意：和 npm run test:ui 用同一组端口，两者不能同时运行
import process from 'node:process'
import { api, startServices, TOKEN, WEB } from './infra.mjs'

async function main() {
  await startServices()
  const ids = await api.registerUsers()
  const wb = await api.createWorkbook(TOKEN.alice, 'Alice 利润表')
  await api.createWorkbook(TOKEN.alice, '9 月销售看板')
  const shared = await api.createWorkbook(TOKEN.alice, '共享只读看板')
  await api.shareWorkbook(TOKEN.alice, shared.id, 'view')
  // BIG_ROWS=2000000：再造一个大数据集，看大表的滚动 / 排序 / 筛选体验（行数多时用 UI_DB 指向放大过的库更快）
  if (process.env.BIG_ROWS) {
    const n = Number(process.env.BIG_ROWS)
    const shops = Array.from({ length: 40 }, (_, i) => `店铺${String(i).padStart(2, '0')}`)
    const lines = ['订单号,店铺,下单时间,渠道,数量,金额']
    for (let i = 0; i < n; i++)
      lines.push(`O${String(i).padStart(9, '0')},${shops[i % 40]},2026-0${1 + (i % 9)}-${String(1 + (i % 28)).padStart(2, '0')} 10:${String(i % 60).padStart(2, '0')}:00,${['直播', '商品卡', '短视频'][i % 3]},${1 + (i % 5)},${((i * 7919) % 100000) / 100}`)
    await api.importDataset(TOKEN.alice, '大订单', `${lines.join('\n')}\n`)
  }
  const orders = await api.importDataset(TOKEN.alice, '订单', '店铺,金额\nA,10\nA,20\nB,100\nB,200\nC,1000\nD,5000\n')
  await api.bindDataset(orders, { column: '店铺' })
  await api.setScope(ids.Bob, ['A', 'B'])
  console.log(`就绪：${WEB}/excel?wb=${wb.id}（首页 ${WEB}/excel/home）`)
  // Ctrl+C 退出时 infra.mjs 负责停掉服务、删掉临时数据
  await new Promise(() => {})
}

main()
