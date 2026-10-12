import type { DatasetUsage } from '@/api/dataset'
import { t } from '@/i18n'

/** 部件类型的中文名 */
export const PART_LABEL: Record<string, string> = {
  'pivot': t('透视表'),
  'webexcel-chart': t('图表'),
  'webexcel-slicer': t('切片器'),
  'webexcel-kpi': t('指标卡'),
  'webexcel-table': t('表格卡片'),
  'webexcel-datatable': t('数据表'),
  'input': t('填报区'),
  'formula': '公式',
}

/** 访问来源的中文名 */
export const ACCESS_LABEL: Record<string, string> = {
  browse: t('明细浏览'),
  pivot: t('透视 / 图表 / 看板'),
  formula: '公式',
  export: t('导出'),
  ai: t('AI 助手'),
  mcp: t('外部 AI（MCP）'),
  report: t('定时推送'),
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
