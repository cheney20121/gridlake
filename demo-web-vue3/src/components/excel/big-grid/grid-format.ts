import type { GridColumn } from './types'
import type { CellValue } from '@/api/dataset'
import { formatNumber } from './column-style'

/** 单元格与行号、列字母的显示格式 */
const money = new Intl.NumberFormat('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
export const integer = new Intl.NumberFormat('zh-CN')
/** 数值列：整数不带小数，比率等小数最多 4 位 */
const decimal = new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 4 })

export function formatCell(v: CellValue, col: GridColumn): string {
  if (v == null || v === '')
    return ''
  if (typeof v === 'number') {
    const custom = col.numFmt ? formatNumber(v, col.numFmt) : null
    if (custom !== null)
      return custom
  }
  if (typeof v === 'number')
    return col.kind === 'money' ? money.format(v) : col.kind === 'number' ? decimal.format(v) : String(v)
  return v
}

export function columnLetter(i: number) {
  let s = ''
  let n = i + 1
  while (n > 0) {
    const m = (n - 1) % 26
    s = String.fromCharCode(65 + m) + s
    n = Math.floor((n - 1) / 26)
  }
  return s
}
