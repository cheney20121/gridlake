import type { ICellData, IWorksheetData } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'

export type SheetCell = string | number | null

export interface AnalysisColumn {
  title: string
  kind?: 'text' | 'money' | 'percent' | 'int'
  width?: number
}

/** 分析面板 → 工作表 */
export interface AnalysisSheetPayload {
  name: string
  title: string
  columns: AnalysisColumn[]
  rows: SheetCell[][]
}

const HEADER_STYLE = { bl: 1, bg: { rgb: '#107C41' }, cl: { rgb: '#FFFFFF' }, ht: 2, vt: 2 }
const TITLE_STYLE = { bl: 1, fs: 13 }
const NUMBER_FORMAT = {
  money: '#,##0.00;-#,##0.00;"-"',
  percent: '0.0%;-0.0%;"-"',
  int: '#,##0;-#,##0;"-"',
} as const

/** 工作表名不能包含 [ ] : * ? / \，且不超过 31 个字符 */
function safeSheetName(name: string) {
  return name.replace(/[[\]:*?/\\]/g, '-').slice(0, 31)
}

/**
 * 把分析结果写成一个新工作表（同名表先删除再写入），方便财务在表格里继续加工、打印或导出 Excel。
 * 写入的是数值而非公式：分析口径来自后端聚合，汇总表的公式联动仍以「汇总表」页签为准
 */
export function writeAnalysisSheet(api: FUniver, name: string, title: string, columns: AnalysisColumn[], rows: SheetCell[][]) {
  const workbook = api.getActiveWorkbook()
  if (!workbook)
    throw new Error('工作簿尚未就绪')
  const sheetName = safeSheetName(name)

  const cellData: Record<number, Record<number, ICellData>> = {
    0: { 0: { v: title, s: TITLE_STYLE } },
    1: Object.fromEntries(columns.map((c, i) => [i, { v: c.title, s: HEADER_STYLE }])),
  }
  rows.forEach((row, r) => {
    cellData[r + 2] = Object.fromEntries(row.map((v, c) => {
      const kind = columns[c]?.kind ?? 'text'
      const cell: ICellData = { v: v ?? '' }
      if (kind !== 'text' && typeof v === 'number')
        cell.s = { n: { pattern: NUMBER_FORMAT[kind] }, ht: 3 }
      return [c, cell]
    }))
  })

  const sheet: Partial<IWorksheetData> = {
    name: sheetName,
    rowCount: Math.max(rows.length + 20, 100),
    columnCount: Math.max(columns.length + 2, 20),
    cellData,
    columnData: Object.fromEntries(columns.map((c, i) => [i, { w: c.width ?? (c.kind === 'text' || !c.kind ? 160 : 110) }])),
    defaultRowHeight: 24,
    freeze: { xSplit: 1, ySplit: 2, startRow: 2, startColumn: 1 },
  }

  const existing = workbook.getSheetByName(sheetName)
  if (existing)
    workbook.deleteSheet(existing)
  const created = workbook.insertSheet(sheetName, { sheet })
  workbook.setActiveSheet(created)
  return sheetName
}
