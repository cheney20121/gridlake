import type { PivotAgg, PivotShow } from '@/api/dataset'

/** 仪表盘部件编辑表单用的选项 */
export const AGGS: { value: PivotAgg, label: string }[] = [
  { value: 'sum', label: '求和' },
  { value: 'count', label: '计数' },
  { value: 'avg', label: '平均' },
  { value: 'max', label: '最大' },
  { value: 'min', label: '最小' },
  { value: 'count_distinct', label: '去重计数' },
]

export const SHOWS: { value: PivotShow, label: string }[] = [
  { value: 'raw', label: '原值' },
  { value: 'pct_total', label: '占合计' },
  { value: 'yoy', label: '同比' },
  { value: 'mom', label: '环比' },
]
