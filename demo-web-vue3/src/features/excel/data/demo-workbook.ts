import type { FUniver } from '@univerjs/core/facade'
import { generateWorkbookData } from '@/features/excel/data/data-service'

/**
 * 默认的演示工作簿：80 行订单数据，Excel 风格的绿色表头。
 * 服务端没有任何工作簿（首次使用）或表格服务连不上时作为起点使用。
 */

const SHEET_ID = 'sheet-orders'

/** 列宽：订单编号 / 客户 / 产品 / 数量 / 单价 / 金额 / 日期 / 状态 */
const COLUMN_WIDTHS = [130, 150, 160, 72, 96, 112, 100, 88]

/** 各列对应的数据样式 id（表头统一用 header） */
const COLUMN_STYLE_IDS = ['text', 'text', 'text', 'int', 'money', 'money', 'date', 'status']

const EXCEL_FONT = { ff: 'Microsoft YaHei', fs: 11 }
const THIN_BORDER = { s: 1, cl: { rgb: '#D4D4D4' } }

/** 复刻 Excel 默认表格风格：绿色表头 + 细边框 + 数字右对齐 + 千分位金额 */
const EXCEL_STYLES = {
  header: {
    ...EXCEL_FONT,
    bl: 1,
    cl: { rgb: '#FFFFFF' },
    bg: { rgb: '#107C41' },
    ht: 2, // 水平居中
    vt: 2, // 垂直居中
    bd: { b: { s: 1, cl: { rgb: '#0B5A2E' } }, r: { s: 1, cl: { rgb: '#3A9A66' } } },
  },
  text: { ...EXCEL_FONT, ht: 1, vt: 2, bd: { b: THIN_BORDER, r: THIN_BORDER } },
  int: { ...EXCEL_FONT, ht: 3, vt: 2, n: { pattern: '#,##0' }, bd: { b: THIN_BORDER, r: THIN_BORDER } },
  money: { ...EXCEL_FONT, ht: 3, vt: 2, n: { pattern: '#,##0.00' }, bd: { b: THIN_BORDER, r: THIN_BORDER } },
  date: { ...EXCEL_FONT, ht: 2, vt: 2, bd: { b: THIN_BORDER, r: THIN_BORDER } },
  status: { ...EXCEL_FONT, ht: 2, vt: 2, bd: { b: THIN_BORDER, r: THIN_BORDER } },
}

type CellValue = string | number | { f: string }
type CellData = Record<number, Record<number, { v?: string | number, f?: string, s?: string }>>

/** 将二维数组转为 Univer cellData 格式（附带样式 id） */
function buildCellData(rows: CellValue[][]): CellData {
  const cellData: CellData = {}
  rows.forEach((row, r) => {
    const rowData: CellData[number] = {}
    row.forEach((val, c) => {
      const s = r === 0 ? 'header' : COLUMN_STYLE_IDS[c]
      rowData[c] = typeof val === 'object' && val !== null && 'f' in val ? { f: val.f, s } : { v: val, s }
    })
    cellData[r] = rowData
  })
  return cellData
}

/** 在 Univer 里创建演示工作簿 */
export function loadDemoWorkbook(api: FUniver, fileName: string) {
  const wbData = generateWorkbookData(80)
  // 注意：sheets 的 key、sheet.id、sheetOrder 必须一致，否则 Univer 会给 sheet 补默认 id，
  // 渲染层按 sheetOrder 找不到对应骨架，导致网格空白
  return api.createWorkbook({
    id: 'order-workbook',
    name: fileName,
    sheetOrder: [SHEET_ID],
    styles: EXCEL_STYLES,
    sheets: {
      [SHEET_ID]: {
        id: SHEET_ID,
        name: wbData.name,
        cellData: buildCellData(wbData.cellData),
        rowCount: Math.max(wbData.rowCount + 20, 200),
        columnCount: Math.max(wbData.columnCount + 6, 26),
        defaultRowHeight: 22,
        defaultColumnWidth: 88,
        columnData: Object.fromEntries(COLUMN_WIDTHS.map((w, i) => [i, { w }])),
        rowData: { 0: { h: 28, hd: 0 } },
        // 冻结首行（表头），与 Excel「冻结首行」一致
        freeze: { xSplit: 0, ySplit: 1, startRow: 1, startColumn: 0 },
        showGridlines: 1,
        rightToLeft: 0,
      },
    },
  })
}
