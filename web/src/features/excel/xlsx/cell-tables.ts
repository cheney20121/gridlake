/**
 * 导出 xlsx：单元格模式的数据表（结果已写成普通单元格）在原位置包成 Excel 表格，
 * 引用它的 TSUM / TCOUNT 等平台函数据此转成 `SUM(表[列])` 等原生写法，在 Excel 里照样能算。
 *
 * 只有格子里的数据与平台算的一致时才这样做，否则公式仍导出为数值（批注里写明原因）：
 * - 写全了：符合条件的行数超过单元格模式上限（只写了前 1 万行）时，Excel 按格子算会少算
 * - 没有手工调整：平台的 TSUM 按数据集算、不含在表里手改的格子，Excel 按格子算会把手改的算进去
 * - 表头还是列名：表头被改过就对不上公式里的列名
 * - 不与工作表上的表格（超级表）重叠：Excel 里两个表格不能重叠
 */

interface IRange {
  startRow: number
  startColumn: number
  endRow: number
  endColumn: number
}

export interface CellTableCandidate {
  sheetId: string
  /** 数据表名（公式里引用的名字） */
  name: string
  /** 表头左上角 */
  row: number
  col: number
  /** 写入的区域（含表头） */
  extent?: { rows: number, cols: number }
  /** 符合条件的总行数 */
  total?: number
  /** 有没有记下的手工调整 */
  edited: boolean
  /** 写入时的列名（按顺序） */
  columns: string[]
  /** 工作表上表头那一行实际的文字 */
  header: string[]
  /** 同一工作表上的表格（超级表）区域 */
  occupied: IRange[]
}

export interface CellTableExport {
  sheetId: string
  name: string
  excelName: string
  range: IRange
  columns: Set<string>
}

const overlaps = (a: IRange, b: IRange) => a.startRow <= b.endRow && b.startRow <= a.endRow && a.startColumn <= b.endColumn && b.startColumn <= a.endColumn

/** 判断每个单元格模式数据表能否包成 Excel 表格；返回可以的（已分配 Excel 表名）与不可以的原因（按表名小写） */
export function planCellTables(list: CellTableCandidate[], tableName: (name: string) => string): { exports: CellTableExport[], reasons: Map<string, string> } {
  const exports: CellTableExport[] = []
  const reasons = new Map<string, string>()
  for (const t of list) {
    const key = t.name.trim().toLowerCase()
    const dataRows = (t.extent?.rows ?? 0) - 1
    const range = { startRow: t.row, startColumn: t.col, endRow: t.row + dataRows, endColumn: t.col + (t.extent?.cols ?? 0) - 1 }
    let reason: string | null = null
    if (!t.extent || dataRows < 1)
      reason = '单元格模式的数据表还没有数据'
    else if ((t.total ?? dataRows) > dataRows)
      reason = `单元格模式的数据表只写了前 ${dataRows.toLocaleString()} 行（共 ${(t.total ?? 0).toLocaleString()} 行）`
    else if (t.edited)
      reason = '单元格模式的数据表里有手工调整，平台按数据集计算、不含手改的格子，Excel 按格子计算结果会不同'
    else if (t.columns.length !== t.extent.cols || t.columns.some((c, i) => (t.header[i] ?? '').trim() !== c))
      reason = '单元格模式的数据表表头被改过，与列名对不上'
    else if (t.occupied.some(o => overlaps(o, range)))
      reason = '单元格模式的数据表与工作表上的表格重叠'
    if (reason) {
      reasons.set(key, reason)
      continue
    }
    exports.push({ sheetId: t.sheetId, name: t.name, excelName: tableName(t.name), range, columns: new Set(t.columns) })
  }
  return { exports, reasons }
}
