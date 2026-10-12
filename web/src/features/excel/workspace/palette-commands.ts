import type { LedgerCommand } from './use-ledger'
import type { DatasetInfo } from '@/api/dataset'
import type { WorkbookMeta } from '@/api/workbook'
import type { PaletteCommand } from '@/components/excel/command-palette/CommandPalette.vue'
import type { RibbonTab } from '@/features/excel/ribbon/app-ribbon'
import { refreshImportRanges } from '@/features/excel/formula/import-range'
import { t } from '@/i18n'

/** 功能区条目在命令面板里的搜索关键字（拼写、同义词） */
const KEYWORDS: Record<string, string> = {
  'app.pivot': 'pivot 透视 汇总 分组',
  'app.chart': 'chart 图 柱状 折线 饼图',
  'app.dashboard': 'dashboard 看板 切片器 指标卡 slicer',
  'app.present': 'present 演示模式 全屏 投屏 只读 仪表盘 看板',
  'app.ai': 'ai 助手 问答 chat',
  'app.import': 'import csv xlsx 上传 导入',
  'app.sql': 'sql 查询 query',
  'app.writeback': '回写 录入 表单 writeback',
  'app.style': 'style 格式 颜色',
  'app.formula': 'formula 函数 fx',
  'app.diagnose': '诊断 慢 卡 性能 错误 #REF #N/A 检查公式',
  'app.collab': '协作 多人 在线',
  'app.comment': 'comment 批注 评论 提醒 at 通知 讨论',
  'app.snapshots': 'history 历史 快照 版本 恢复',
}

export type DataCenterCommand = 'sql' | 'anomaly' | 'connector' | 'report' | 'mcp' | 'audit' | 'datascope' | 'template' | 'trash' | LedgerCommand
export type FileCommand = 'manage' | 'template' | 'publish' | 'import' | 'export' | 'snapshots' | 'save'

/** 命令面板需要的页面状态与动作 */
export interface PaletteContext {
  tabs: RibbonTab[]
  isLedgerWorkbook: boolean
  isAdmin: boolean
  authRequired: boolean
  connectorFailures: number
  homeHref: string
  sheets: { id: string, name: string }[]
  activeSheetId?: string
  workspaceView: 'workbook' | 'dataset'
  datasets: DatasetInfo[]
  currentWorkbookId?: string
  workbooks: WorkbookMeta[]
  save: () => void
  onFile: (c: FileCommand) => void
  onDataCenter: (c: DataCenterCommand) => void
  onLedger: (c: LedgerCommand) => void
  onApproval: (c: 'start' | 'center') => void
  activateSheet: (id: string) => void
  openDataset: (id: string) => void
  openWorkbook: (w: WorkbookMeta) => void
}

/** Alt+Q 命令面板里的全部条目：功能、其他入口、工作表、数据集、工作簿 */
export function buildPaletteCommands(c: PaletteContext): PaletteCommand[] {
  const seen = new Map<string, PaletteCommand>()
  const features: PaletteCommand[] = []
  const add = (cmd: Omit<PaletteCommand, 'group'>) => {
    // 同名的功能区按钮（如「插入」和「分析」页签里各有一个数据透视表）只留一条，搜索词合并
    const prev = seen.get(cmd.title)
    if (prev) {
      prev.keywords = `${prev.keywords ?? ''} ${cmd.keywords ?? ''}`
      return
    }
    const item = { ...cmd, group: '功能' }
    seen.set(cmd.title, item)
    features.push(item)
  }
  for (const tab of c.tabs) {
    if (tab.contextual && !c.isLedgerWorkbook)
      continue
    for (const g of tab.groups) {
      for (const item of g.items)
        add({ id: item.id, title: item.title, keywords: `${KEYWORDS[item.id] ?? ''} ${item.tooltip ?? ''} ${tab.title ?? ''}`, run: item.run })
    }
  }
  const more: Omit<PaletteCommand, 'group'>[] = [
    { id: 'file.save', title: t('保存'), hint: 'Ctrl+S', keywords: 'save', run: c.save },
    { id: 'file.manage', title: t('打开 / 新建 / 管理工作簿'), keywords: 'open new 文件夹', run: () => c.onFile('manage') },
    { id: 'file.template', title: t('从模板新建'), keywords: 'template 模板', run: () => c.onFile('template') },
    { id: 'file.publish', title: t('发布为模板'), keywords: 'template publish', run: () => c.onFile('publish') },
    { id: 'file.import', title: t('导入 Excel 文件（.xlsx）'), keywords: 'xlsx excel 打开', run: () => c.onFile('import') },
    { id: 'file.export', title: t('导出为 Excel（.xlsx）'), keywords: 'xlsx excel 下载 export', run: () => c.onFile('export') },
    { id: 'dc.anomaly', title: t('异常发现'), keywords: 'anomaly 异常 偏离 骤降 暴跌 骤增 不对劲 检查 巡检', run: () => c.onDataCenter('anomaly') },
    { id: 'dc.connector', title: t('数据连接'), hint: c.connectorFailures ? `${c.connectorFailures} 个同步失败` : undefined, keywords: 'connector 数据库 同步 mysql 接口', run: () => c.onDataCenter('connector') },
    { id: 'dc.report', title: t('定时推送'), keywords: 'report 推送 飞书 邮件 企业微信 订阅', run: () => c.onDataCenter('report') },
    ...(c.isAdmin ? [{ id: 'dc.mcp', title: t('外部 AI 接入（MCP）'), keywords: 'mcp claude 令牌 token', run: () => c.onDataCenter('mcp') }] : []),
    ...(c.authRequired && c.isAdmin ? [{ id: 'dc.datascope', title: t('数据权限（按店铺）'), keywords: 'scope 权限 行级 店铺 范围 可见', run: () => c.onDataCenter('datascope') }] : []),
    ...(c.authRequired && c.isAdmin ? [{ id: 'dc.audit', title: t('操作审计'), keywords: 'audit 审计 日志 谁改了 操作记录', run: () => c.onDataCenter('audit') }] : []),
    { id: 'formula.refresh-import', title: t('刷新跨工作簿引用（IMPORTRANGE）'), keywords: 'importrange 外部链接 更新链接 引用其他工作簿', run: () => {
      refreshImportRanges()
      ElMessage.success(t('已重新读取被引用工作簿的最新保存版本'))
    } },
    { id: 'dc.trash', title: t('数据集回收站'), keywords: 'trash 恢复 删除', run: () => c.onDataCenter('trash') },
    { id: 'approval.start', title: t('发起审批'), keywords: 'approval 审批', run: () => c.onApproval('start') },
    { id: 'approval.center', title: t('审批中心'), keywords: 'approval 待办', run: () => c.onApproval('center') },
    { id: 'nav.home', title: t('返回首页'), keywords: 'home 首页 最近', run: () => window.location.assign(c.homeHref) },
  ]
  if (!c.isLedgerWorkbook) {
    more.push(
      { id: 'dc.ledger.import', title: t('导入店铺明细'), keywords: '台账 资金 流水', run: () => c.onLedger('import') },
      { id: 'dc.ledger.shops', title: t('店铺档案'), keywords: '台账', run: () => c.onLedger('shops') },
      { id: 'dc.ledger.close', title: t('月结中心'), keywords: '台账 对账 锁账', run: () => c.onLedger('close') },
    )
  }
  more.forEach(add)

  const sheets: PaletteCommand[] = c.sheets.map(s => ({
    id: `sheet.${s.id}`,
    title: s.name,
    group: '工作表',
    hint: s.id === c.activeSheetId && c.workspaceView === 'workbook' ? '当前' : undefined,
    keywords: 'sheet 工作表',
    run: () => c.activateSheet(s.id),
  }))
  const data: PaletteCommand[] = c.datasets.map(d => ({
    id: `dataset.${d.id}`,
    title: d.name,
    group: '数据集',
    hint: t('{0} 行', [d.rowCount.toLocaleString()]),
    keywords: `数据集 dataset ${d.columns.map(col => col.title).join(' ')}`,
    run: () => c.openDataset(d.id),
  }))
  const books: PaletteCommand[] = c.workbooks
    .filter(w => w.id !== c.currentWorkbookId)
    .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
    .map(w => ({ id: `workbook.${w.id}`, title: w.name, group: '工作簿', keywords: '工作簿 打开 workbook', run: () => c.openWorkbook(w) }))
  return [...features, ...sheets, ...data, ...books]
}
