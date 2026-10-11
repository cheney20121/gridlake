import type { DatasetFilter, PivotAgg, PivotShow } from '@/api/dataset'

/**
 * 仪表盘部件：与图表一样是浮动在工作表上的对象，定义存放在工作簿快照里
 *
 * - 切片器：按一个字段筛选同一工作表上所有数据集部件（只要该数据集有同名列）
 * - 指标卡：一个汇总值 + 同比 / 环比 + 趋势小图
 * - 表格卡片：按一个字段分组的汇总表（如销售额前 10 的店铺）
 */

export const SLICER_KEY = 'webexcel-slicer'
export const KPI_KEY = 'webexcel-kpi'
export const TABLE_KEY = 'webexcel-table'

export type DatePreset = 'this_month' | 'last_month' | 'this_quarter' | 'last_quarter' | 'this_year' | 'last_year' | 'last_7' | 'last_30' | 'last_90'

export interface SlicerSpec {
  v: 1
  id: string
  kind: 'slicer'
  title: string
  /** 取选项的数据集 */
  datasetId: string
  /** 字段（列名）；筛选同一工作表上所有含同名列的数据集部件 */
  column: string
  /** values：按值多选；date：按日期范围（日期时间列） */
  mode: 'values' | 'date'
  /** 选中的值；空为全部 */
  values: string[]
  /** 日期范围：相对时间（如本月）或固定起止日期；空为全部 */
  range?: { preset?: DatePreset | null, min?: string | null, max?: string | null } | null
}

export type KpiCompare = 'none' | 'yoy' | 'mom'

export interface KpiSpec {
  v: 1
  id: string
  kind: 'kpi'
  title: string
  datasetId: string
  value: { column?: string | null, agg: PivotAgg }
  /** 同比 / 环比、趋势小图用的日期字段（日期时间列） */
  dateColumn?: string | null
  /** 比较与趋势的粒度 */
  granularity: 'year' | 'month' | 'day'
  compare: KpiCompare
  /** 上升是好事（销售额）还是坏事（成本、退款） */
  upIsGood: boolean
  /** 显示趋势小图（最近 12 期） */
  sparkline: boolean
  /** 部件自己的筛选（与切片器叠加） */
  filters?: DatasetFilter[]
}

export interface TableCardSpec {
  v: 1
  id: string
  kind: 'table'
  title: string
  datasetId: string
  row: { column: string, granularity?: 'year' | 'month' | 'day' | null }
  values: { column?: string | null, agg: PivotAgg, show?: PivotShow }[]
  /** 按第一个值排名取前 N 项（0 为全部） */
  top: number
  /** 排序：按第一个值降序，或按分组名 */
  sort: 'value' | 'key'
  filters?: DatasetFilter[]
}

export type DashboardSpec = SlicerSpec | KpiSpec | TableCardSpec

export const DATE_PRESETS: { value: DatePreset, label: string }[] = [
  { value: 'this_month', label: '本月' },
  { value: 'last_month', label: '上月' },
  { value: 'this_quarter', label: '本季度' },
  { value: 'last_quarter', label: '上季度' },
  { value: 'this_year', label: '今年' },
  { value: 'last_year', label: '去年' },
  { value: 'last_7', label: '近 7 天' },
  { value: 'last_30', label: '近 30 天' },
  { value: 'last_90', label: '近 90 天' },
]
