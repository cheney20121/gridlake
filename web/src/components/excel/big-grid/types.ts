import type { NumFmt, RuleMatcher } from './column-style'
import type { DatasetColumnKind } from '@/api/dataset'

export interface GridColumn {
  /** 数据集列名；贡献列为 __contribution */
  key: string
  title: string
  kind: DatasetColumnKind
  width: number
  /** 加粗显示（下钻时的贡献金额列） */
  emphasis?: boolean
  /** 数字格式（数据表列样式）；省略按列类型默认显示 */
  numFmt?: NumFmt
  /** 条件格式：按值求出命中规则的样式（compileRules 的结果）；省略为没有规则 */
  style?: RuleMatcher
}

/** 数据表底部汇总行 */
export interface GridFooter {
  /** 行号列里的文字 */
  label: string
  /** 悬停在行号列上的说明（按多少行计算、耗时） */
  tip?: string
  /** 计算出错的原因 */
  error?: string
  /** 与列一一对应；label 为空表示这一列不汇总 */
  cells: { label: string, value: string }[]
}

export interface GridCell {
  row: number
  col: number
}

export interface GridSelection {
  anchor: GridCell
  focus: GridCell
  /** 分类汇总时：选区里的数据行区间（行号是数据行，不含小计行）；只选了小计行时为 null。没有分类汇总时省略 */
  dataRows?: [number, number] | null
}

/** 分类汇总：每组的行数（按数据表的顺序）与小计行各列的文字 */
export interface GridGroups {
  sizes: number[]
  cells: (group: number) => (string | null)[]
}
