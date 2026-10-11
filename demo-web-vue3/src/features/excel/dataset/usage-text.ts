import type { DatasetUsage } from '@/api/dataset'

/** 部件类型的中文名 */
export const PART_LABEL: Record<string, string> = {
  'pivot': '透视表',
  'webexcel-chart': '图表',
  'webexcel-slicer': '切片器',
  'webexcel-kpi': '指标卡',
  'webexcel-table': '表格卡片',
  'webexcel-datatable': '数据表',
  'input': '填报区',
  'formula': '公式',
}

/** 访问来源的中文名 */
export const ACCESS_LABEL: Record<string, string> = {
  browse: '明细浏览',
  pivot: '透视 / 图表 / 看板',
  formula: '公式',
  export: '导出',
  ai: 'AI 助手',
  mcp: '外部 AI（MCP）',
  report: '定时推送',
  sql: 'SQL 查询',
}

export function partsText(parts: Record<string, number>) {
  return Object.entries(parts).map(([k, n]) => `${PART_LABEL[k] ?? k} ${n}`).join('、')
}

/** 引用它的地方的个数（不含访问次数） */
export function referenceCount(u: DatasetUsage) {
  return u.workbooks.length + u.templates.length + u.views.length + u.sqlQueries.length + u.connectors.length + u.reports.length
}

/** 一句话影响范围（如「3 个工作簿、1 个定时推送、1 个数据连接」），没有引用时为空 */
export function impactSummary(u: DatasetUsage) {
  return [
    u.workbooks.length && `${u.workbooks.length} 个工作簿`,
    u.reports.length && `${u.reports.length} 个定时推送`,
    u.views.length && `${u.views.length} 个关联视图`,
    u.sqlQueries.length && `${u.sqlQueries.length} 个 SQL 查询`,
    u.connectors.length && `${u.connectors.length} 个数据连接`,
    u.templates.length && `${u.templates.length} 个模板`,
  ].filter(Boolean).join('、')
}
