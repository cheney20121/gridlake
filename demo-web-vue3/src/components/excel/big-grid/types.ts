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

export interface GridCell {
  row: number
  col: number
}

export interface GridSelection {
  anchor: GridCell
  focus: GridCell
}
