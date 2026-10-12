import type { LedgerCommand } from './use-ledger'
import type { RibbonTab } from '@/features/excel/ribbon/app-ribbon'

/** 功能区需要的页面动作（由页面传入，这里只负责声明有哪些入口） */
export interface RibbonActions {
  openPanel: (name: 'pivot' | 'ai' | 'writeback' | 'style' | 'formula' | 'collaboration' | 'snapshots' | 'comments') => void
  openChart: () => void
  openDashboard: () => void
  present: () => void
  importDataset: () => void
  openSql: () => void
  diagnose: () => void
  ledger: (c: LedgerCommand) => void
}

/**
 * 平台功能的功能区页签（功能区与命令面板共用）：
 * Excel 用户习惯的「插入 → 数据透视表 / 图表」，平台功能集中在「分析」页签，台账功能在上下文页签「台账」
 */
export function appRibbonTabs(a: RibbonActions): RibbonTab[] {
  const panel = (name: Parameters<RibbonActions['openPanel']>[0]) => () => a.openPanel(name)
  const ledger = (c: LedgerCommand) => () => a.ledger(c)
  return [
    {
      key: 'ribbon.insert',
      groups: [{
        key: 'ribbon.insert.app',
        items: [
          { id: 'app.insert.pivot', title: '数据透视表', tooltip: '按数据集分组汇总，可下钻明细', run: panel('pivot') },
          { id: 'app.insert.chart', title: '图表', tooltip: '用选中的数据或数据集插入图表', run: a.openChart },
          { id: 'app.insert.dashboard', title: '看板部件', tooltip: '切片器、指标卡、表格卡片', run: a.openDashboard },
        ],
      }],
    },
    {
      key: 'ribbon.analysis',
      title: '分析',
      order: 2.5,
      groups: [
        {
          key: 'ribbon.analysis.analyze',
          items: [
            { id: 'app.pivot', title: '数据透视表', tooltip: '按数据集分组汇总，可下钻明细', run: panel('pivot') },
            { id: 'app.chart', title: '图表', tooltip: '用选中的数据或数据集插入图表', run: a.openChart },
            { id: 'app.dashboard', title: '仪表盘', tooltip: '切片器、指标卡、图表、表格，可全屏演示', run: a.openDashboard },
            { id: 'app.present', title: '演示', tooltip: '全屏只读展示当前工作表（仪表盘），切片器与图表联动照常可用，Esc 退出', run: a.present },
            { id: 'app.ai', title: 'AI 助手', tooltip: '用中文提问，查询数据并生成透视表、图表、公式建议', run: panel('ai') },
          ],
        },
        {
          key: 'ribbon.analysis.data',
          items: [
            { id: 'app.import', title: '导入数据集', tooltip: '导入任意 CSV / XLSX 为数据集（支持千万行）', run: a.importDataset },
            { id: 'app.sql', title: 'SQL 查询', tooltip: '对数据集写 SQL，结果可另存为数据集并定时重算', run: a.openSql },
            { id: 'app.writeback', title: '填报', tooltip: '在工作表里放填报区，填好后提交写入数据集', run: panel('writeback') },
          ],
        },
        {
          key: 'ribbon.analysis.tools',
          items: [
            { id: 'app.style', title: '单元格样式', run: panel('style') },
            { id: 'app.formula', title: '公式函数', run: panel('formula') },
            { id: 'app.diagnose', title: '公式诊断', tooltip: '找出拖慢计算的公式（整列引用、易失函数、大量查找）和出错的公式，实测重算耗时', run: a.diagnose },
            { id: 'app.comment', title: '评论', tooltip: '给选中的格子写评论，@ 同事提醒他们', run: panel('comments') },
            { id: 'app.collab', title: '实时协作', run: panel('collaboration') },
            { id: 'app.snapshots', title: '版本历史', run: panel('snapshots') },
          ],
        },
      ],
    },
    {
      key: 'ribbon.ledger',
      title: '台账',
      order: 2.6,
      contextual: true,
      groups: [
        {
          key: 'ribbon.ledger.data',
          items: [
            { id: 'app.ledger.import', title: '导入店铺明细', tooltip: '批量导入店铺资金流水', run: ledger('import') },
            { id: 'app.ledger.detail', title: '全部流水明细', tooltip: '当前月份的全部流水', run: ledger('detail') },
          ],
        },
        {
          key: 'ribbon.ledger.close',
          items: [
            { id: 'app.ledger.shops', title: '店铺档案', run: ledger('shops') },
            { id: 'app.ledger.coverage', title: '月结完整性', run: ledger('coverage') },
            { id: 'app.ledger.close', title: '月结中心', tooltip: '勾稽、对账、锁账', run: ledger('close') },
          ],
        },
      ],
    },
  ]
}
