import type { DatasetColumn, DatasetFilter, PivotAgg, PivotGranularity, PivotShow } from '@/api/dataset'

/** 透视表字段面板的区域、条目与选项 */
export type Area = 'filters' | 'columns' | 'rows' | 'values'
export interface Item {
  uid: string
  column: string
  kind: DatasetColumn['kind']
  granularity?: PivotGranularity | null
  /** 表头别名（在透视表格子里改名得到） */
  label?: string
  agg?: PivotAgg
  /** 值的显示方式（占比 / 同比 / 环比） */
  show?: PivotShow
  /** 计算值（agg 为 calc）的公式与数字格式；名称放在 label 里 */
  formula?: string
  format?: 'money' | 'number' | 'percent'
  filter?: DatasetFilter | null
}

export const AREAS: { key: Area, label: string, hint: string }[] = [
  { key: 'filters', label: '筛选', hint: '拖入字段按值筛选' },
  { key: 'columns', label: '列', hint: '拖入字段作为列' },
  { key: 'rows', label: '行', hint: '拖入字段作为行（可多级）' },
  { key: 'values', label: '值', hint: '拖入字段求和 / 计数 / 平均…' },
]
export const AGG_OPTIONS: { value: PivotAgg, label: string, numeric: boolean }[] = [
  { value: 'sum', label: '求和', numeric: true },
  { value: 'count', label: '计数', numeric: false },
  { value: 'avg', label: '平均值', numeric: true },
  { value: 'max', label: '最大值', numeric: true },
  { value: 'min', label: '最小值', numeric: true },
  { value: 'first', label: '首个（按行顺序第一个非空值，如期初余额）', numeric: true },
  { value: 'last', label: '末个（按行顺序最后一个非空值，如期末余额）', numeric: true },
  { value: 'count_distinct', label: '去重计数', numeric: false },
  { value: 'approx_distinct', label: '近似去重计数（快，误差通常在 5% 以内）', numeric: false },
  { value: 'median', label: '中位数', numeric: true },
  { value: 'approx_median', label: '近似中位数（快、省内存，大数据集用）', numeric: true },
  { value: 'p10', label: 'P10 分位', numeric: true },
  { value: 'p25', label: 'P25 分位', numeric: true },
  { value: 'p75', label: 'P75 分位', numeric: true },
  { value: 'p90', label: 'P90 分位', numeric: true },
  { value: 'p95', label: 'P95 分位', numeric: true },
  { value: 'p99', label: 'P99 分位', numeric: true },
  { value: 'approx_p90', label: '近似 P90（快、省内存）', numeric: true },
  { value: 'approx_p95', label: '近似 P95（快、省内存）', numeric: true },
  { value: 'approx_p99', label: '近似 P99（快、省内存）', numeric: true },
  { value: 'stddev', label: '标准差', numeric: true },
  { value: 'var', label: '方差', numeric: true },
]
/** 服务端生成表头用的汇总方式名称（与 pivot.rs 的 agg_label 一致）：计算值公式里按「列名（求和）」引用 */
const SERVER_AGG_LABEL: Record<string, string> = {
  sum: '求和',
  count: '计数',
  avg: '平均',
  min: '最小',
  max: '最大',
  first: '首个',
  last: '末个',
  count_distinct: '去重计数',
  approx_distinct: '近似去重计数',
  median: '中位数',
  approx_median: '近似中位数',
  approx_p90: '近似 P90',
  approx_p95: '近似 P95',
  approx_p99: '近似 P99',
  p10: 'P10',
  p25: 'P25',
  p75: 'P75',
  p90: 'P90',
  p95: 'P95',
  p99: 'P99',
  stddev: '标准差',
  var: '方差',
}

/** 值字段在透视表里的表头文字（自定义的优先）；计算值的公式用它来引用 */
export function valueLabel(it: Pick<Item, 'column' | 'agg' | 'label'>): string {
  if (it.label)
    return it.label
  if (it.column === '（行数）')
    return '计数'
  return `${it.column}（${SERVER_AGG_LABEL[it.agg ?? 'sum'] ?? ''}）`
}

export const GRANULARITY_OPTIONS: { value: PivotGranularity, label: string }[] = [
  { value: 'year', label: '按年' },
  { value: 'quarter', label: '按季度' },
  { value: 'month', label: '按月' },
  { value: 'week', label: '按周' },
  { value: 'day', label: '按日' },
]
export const SHOW_OPTIONS: { value: PivotShow, label: string, hint: string }[] = [
  { value: 'raw', label: '原值', hint: '' },
  { value: 'pct_total', label: '占总计', hint: '占全部合计的百分比' },
  { value: 'pct_row', label: '占行合计', hint: '占所在行（跨列）合计的百分比，需要有列字段' },
  { value: 'pct_col', label: '占列合计', hint: '占所在列合计的百分比，如各店铺在当月的占比' },
  { value: 'pct_parent', label: '占上级', hint: '占上一级行汇总的百分比，如店铺占所在主体的比例' },
  { value: 'yoy', label: '同比', hint: '与上年同期比的增长率；日期字段需放在行或列' },
  { value: 'yoy_diff', label: '同比增减', hint: '与上年同期相比增加 / 减少的金额' },
  { value: 'mom', label: '环比', hint: '与上一期（上月 / 上年 / 前一天）比的增长率' },
  { value: 'mom_diff', label: '环比增减', hint: '与上一期相比增加 / 减少的金额' },
]

export const SORT_OPTIONS = [
  { value: 'key-asc', label: '按名称 A→Z' },
  { value: 'key-desc', label: '按名称 Z→A' },
  { value: 'value-desc', label: '按第一个值 从大到小' },
  { value: 'value-asc', label: '按第一个值 从小到大' },
]

export const KIND_ICON: Record<string, string> = { text: 'Aa', id: '#', money: '¥', number: '12', datetime: '📅' }

export const isNumeric = (kind: string) => kind === 'money' || kind === 'number'

/** 从字段列表拖出时生成区域条目 */
export function cloneField(col: DatasetColumn): Item {
  return { uid: `${col.key}-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`, column: col.title, kind: col.kind }
}
