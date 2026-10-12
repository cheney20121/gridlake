import type { DatasetFilter, PivotAgg } from '@/api/dataset'

/**
 * 图表定义：保存在 Univer 浮动对象（float DOM）的 data 中，随工作簿快照保存 / 版本 / 冲突处理
 *
 * 设计约束（来自可视化规范，不提供开关）：
 * - 只有一个数值轴（不做双轴：两个量纲不同的指标请做两张图）
 * - 系列最多 8 个，超出的合并为「其他」；饼 / 环形图最多 6 块
 * - 数据标签默认关闭，只标注末端 / 最大值；每张图都能切到「数据表」查看全部数值
 */

export type ChartType = 'column' | 'bar' | 'line' | 'area' | 'pie' | 'donut' | 'scatter' | 'waterfall' | 'combo' | 'histogram'

/** 来源一：单元格区域（当前工作簿任意工作表，含透视表结果） */
export interface RangeSource {
  kind: 'range'
  /** 工作表 ID（改名不影响） */
  sheetId: string
  /** A1 表示法，如 A1:D13 */
  range: string
  /** 第一行是系列名 */
  headerRow: boolean
  /** 第一列是分类（X 轴） */
  firstColLabels: boolean
  /** 系列按列（默认）或按行 */
  seriesIn: 'columns' | 'rows'
}

/** 来源二：数据集（千万行在服务端聚合，只返回聚合结果） */
export interface DatasetSource {
  kind: 'dataset'
  datasetId: string
  /** 分类轴字段；日期字段可按年 / 月 / 日分组 */
  x: { column: string, granularity?: 'year' | 'month' | 'day' | null }
  /** 数值（可多个，每个一条系列） */
  values: { column?: string | null, agg: PivotAgg }[]
  /** 按此字段拆分系列（与多个数值二选一） */
  splitBy?: string | null
  filters?: DatasetFilter[]
  /** 分类排序：按分类名（日期默认）或按第一个数值降序 */
  sort?: 'x' | 'value'
  /** 只取前 N 个分类（按第一个数值） */
  top?: number | null
}

export type ChartSource = RangeSource | DatasetSource

export type LabelMode = 'none' | 'key' | 'all'

export interface ChartSpec {
  /** 版本号，便于以后迁移 */
  v: 1
  id: string
  type: ChartType
  title: string
  source: ChartSource
  /** 柱 / 条 / 面积图堆叠 */
  stacked?: boolean
  /** 图例位置；单系列时不显示 */
  legend?: 'top' | 'bottom' | 'right'
  /** 数据标签：不显示 / 关键点（末端、最大值）/ 全部（系列少时） */
  labels?: LabelMode
  /** 组合图：每个系列画成柱或线（同一数值轴） */
  seriesTypes?: Record<string, 'bar' | 'line'>
  /** 数值单位：自动（≥ 1 万显示「万」）/ 原值 / 百分比 */
  unit?: 'auto' | 'raw' | 'percent'
  /** 瀑布图：末尾追加「合计」柱 */
  waterfallTotal?: boolean
  /** 直方图分组数（空为自动） */
  bins?: number | null
}

/** 解析后的图表数据：分类 × 系列 */
export interface ChartData {
  categories: string[]
  series: { name: string, values: (number | null)[] }[]
  /** 来源的数值是否为金额（用于格式化） */
  money?: boolean
  /** 提示（如「系列超过 8 个，已合并为其他」） */
  notes: string[]
}

export const CHART_COMPONENT_KEY = 'webexcel-chart'
export const MAX_SERIES = 8
export const MAX_SLICES = 6
/** 散点图同时出现的系列上限（多于 3 个时颜色在色盲情况下难以区分） */
export const MAX_SCATTER_SERIES = 3

export const CHART_TYPE_LABEL: Record<ChartType, string> = {
  column: '柱形图',
  bar: '条形图',
  line: '折线图',
  area: '面积图',
  pie: '饼图',
  donut: '环形图',
  scatter: '散点图',
  waterfall: '瀑布图',
  combo: '组合图',
  histogram: '直方图',
}
