import type { DatasetColumnKind } from '@/api/dataset'

export interface GridColumn {
  /** 数据集列名；贡献列为 __contribution */
  key: string
  title: string
  kind: DatasetColumnKind
  width: number
  /** 加粗显示（下钻时的贡献金额列） */
  emphasis?: boolean
}

export interface GridCell {
  row: number
  col: number
}

export interface GridSelection {
  anchor: GridCell
  focus: GridCell
}
