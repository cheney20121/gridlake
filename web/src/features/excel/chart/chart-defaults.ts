import type { FUniver } from '@univerjs/core/facade'
import type { ChartData, ChartType, RangeSource } from './types'

/** 列号 → 字母（0 → A） */
export function colName(c: number) {
  let s = ''
  let n = c + 1
  while (n > 0) {
    const m = (n - 1) % 26
    s = String.fromCharCode(65 + m) + s
    n = Math.floor((n - 1) / 26)
  }
  return s
}

const blank = (v: unknown) => v == null || (typeof v === 'string' && v.trim() === '')

/**
 * 当前选区作为图表数据：选了多个单元格就用选区；只选了一个单元格时，扩展到它所在的连续数据块
 * （向上下左右扩展直到整行 / 整列为空，与 Excel 插入图表的习惯一致）
 */
export function selectionSource(api: FUniver): RangeSource | null {
  const sheet = api.getActiveWorkbook()?.getActiveSheet()
  const sel = sheet?.getSelection()?.getActiveRange()
  if (!sheet || !sel)
    return null
  let r0 = sel.getRow()
  let c0 = sel.getColumn()
  let r1 = sel.getLastRow()
  let c1 = sel.getLastColumn()
  if (r0 === r1 && c0 === c1) {
    const maxR = Math.min(sheet.getMaxRows() - 1, r0 + 5000)
    const maxC = Math.min(sheet.getMaxColumns() - 1, c0 + 60)
    const top = Math.max(0, r0 - 5000)
    const left = Math.max(0, c0 - 60)
    const vals = sheet.getRange(top, left, maxR - top + 1, maxC - left + 1).getValues() as unknown[][]
    const at = (r: number, c: number) => vals[r - top]?.[c - left]
    if (blank(at(r0, c0)))
      return null
    const rowEmpty = (r: number) => {
      for (let c = c0; c <= c1; c++) {
        if (!blank(at(r, c)))
          return false
      }
      return true
    }
    const colEmpty = (c: number) => {
      for (let r = r0; r <= r1; r++) {
        if (!blank(at(r, c)))
          return false
      }
      return true
    }
    let grew = true
    while (grew) {
      grew = false
      if (r0 > top && !rowEmpty(r0 - 1)) {
        r0--
        grew = true
      }
      if (r1 < maxR && !rowEmpty(r1 + 1)) {
        r1++
        grew = true
      }
      if (c0 > left && !colEmpty(c0 - 1)) {
        c0--
        grew = true
      }
      if (c1 < maxC && !colEmpty(c1 + 1)) {
        c1++
        grew = true
      }
    }
  }
  // 首行有文字、首列是分类：按 Excel 默认识别表头
  const head = sheet.getRange(r0, c0, Math.min(2, r1 - r0 + 1), c1 - c0 + 1).getValues() as unknown[][]
  const firstRowText = (head[0] ?? []).slice(1).some(v => typeof v === 'string' && !blank(v) && Number.isNaN(Number(v)))
  const firstCol = sheet.getRange(r0, c0, Math.min(20, r1 - r0 + 1), 1).getValues() as unknown[][]
  const firstColText = firstCol.slice(1).some(([v]) => typeof v === 'string' && !blank(v) && Number.isNaN(Number(String(v).replace(/,/g, ''))))
  return {
    kind: 'range',
    sheetId: sheet.getSheetId(),
    range: `${colName(c0)}${r0 + 1}:${colName(c1)}${r1 + 1}`,
    headerRow: firstRowText || r1 > r0,
    firstColLabels: firstColText || c1 > c0,
    seriesIn: 'columns',
  }
}

/** 推荐图表类型：分类像日期 / 月份 → 折线；否则柱形 */
export function suggestType(data: ChartData): ChartType {
  const dateLike = data.categories.length >= 3 && data.categories.filter(c => /^\d{4}(?:[-/年.]\d{1,2})?(?:[-/月.]\d{1,2})?/.test(c) || /^\d{1,2}月/.test(c)).length >= data.categories.length * 0.8
  if (dateLike)
    return 'line'
  return data.categories.length > 12 && data.series.length === 1 ? 'bar' : 'column'
}
