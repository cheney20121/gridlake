import { FILTER_BTN, ROW_H } from './grid-constants'

/** 鼠标位置命中了画布的哪一块 */
export type Hit
  = | { area: 'corner' }
    | { area: 'col', col: number }
    | { area: 'filter', col: number }
    | { area: 'resize', col: number }
    | { area: 'row', row: number }
    | { area: 'cell', row: number, col: number }
    | { area: 'none' }

/** 命中测试需要的几何信息 */
export interface HitGeometry {
  numW: number
  scrollLeft: number
  /** 各列左边缘（含末尾的总宽） */
  colX: number[]
  colCount: number
  letterH: number
  headH: number
  firstRow: number
  total: number
}

/** 横坐标（相对画布左边缘）落在第几列；落在行号列或最后一列之外返回 -1 */
export function colAt(g: HitGeometry, px: number): number {
  const x = px - g.numW + g.scrollLeft
  if (x < 0)
    return -1
  for (let i = 0; i < g.colCount; i++) {
    if (x < g.colX[i + 1]!)
      return i
  }
  return -1
}

/** px / py 为相对画布左上角的坐标 */
export function hitTest(g: HitGeometry, px: number, py: number): Hit {
  if (py < g.headH && px < g.numW)
    return { area: 'corner' }
  if (py < g.headH) {
    // 列右边缘 4px 内为调整列宽
    const x = px - g.numW + g.scrollLeft
    for (let i = 0; i < g.colCount; i++) {
      if (Math.abs(x - g.colX[i + 1]!) <= 4)
        return { area: 'resize', col: i }
    }
    const col = colAt(g, px)
    if (col < 0)
      return { area: 'none' }
    const right = g.numW + g.colX[col + 1]! - g.scrollLeft
    if (py >= g.letterH && px >= right - FILTER_BTN - 4)
      return { area: 'filter', col }
    return { area: 'col', col }
  }
  const row = g.firstRow + Math.floor((py - g.headH) / ROW_H)
  if (row >= g.total)
    return { area: 'none' }
  if (px < g.numW)
    return { area: 'row', row }
  const col = colAt(g, px)
  return col < 0 ? { area: 'none' } : { area: 'cell', row, col }
}
