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
  'formula': t('公式'),
}

/** 访问来源的中文名 */
export const ACCESS_LABEL: Record<string, string> = {
  browse: t('明细浏览'),
  pivot: t('透视 / 图表 / 看板'),
  formula: t('公式'),
  export: t('导出'),
  ai: t('AI 助手'),
  mcp: t('外部 AI（MCP）'),
  report: t('定时推送'),
  sql: t('SQL 查询'),
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
    u.workbooks.length && t('{0} 个工作簿', [u.workbooks.length]),
    u.reports.length && t('{0} 个定时推送', [u.reports.length]),
    u.views.length && t('{0} 个关联视图', [u.views.length]),
    u.sqlQueries.length && t('{0} 个 SQL 查询', [u.sqlQueries.length]),
    u.connectors.length && t('{0} 个数据连接', [u.connectors.length]),
    u.templates.length && t('{0} 个模板', [u.templates.length]),
  ].filter(Boolean).join('、')
}
