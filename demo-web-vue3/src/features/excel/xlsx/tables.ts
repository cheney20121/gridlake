import type { Worksheet } from 'exceljs'

/**
 * 表格（Excel 的「超级表」）与 xlsx 的互转：名称、位置、列名、筛选按钮随文件走，样式统一用 Excel 的「中等深浅 2」（斑马纹）。
 * 单元格里的值与公式保持原样；表格本身的样式、汇总行、每列的公式规则不带出 / 不带入。
 */

interface IRange {
  startRow: number
  startColumn: number
  endRow: number
  endColumn: number
}

/** Univer 表格插件保存在工作簿快照里的一个表格（只列用到的字段） */
export interface UniverTable {
  id?: string
  name: string
  range: IRange
  options?: { showHeader?: boolean, showAutoFilter?: boolean }
  columns?: { displayName?: string, name?: string }[]
  /** name 已是分配好的 Excel 表名（导出前公式已按它改写），原样使用 */
  exactName?: boolean
}

function colName(c: number) {
  let s = ''
  let n = c + 1
  while (n > 0) {
    const m = (n - 1) % 26
    s = String.fromCharCode(65 + m) + s
    n = Math.floor((n - 1) / 26)
  }
  return s
}

/** Excel 对表格名的要求：字母、数字、下划线、点，不能以数字开头，不能像单元格地址（A1、R1C1）或叫 R、C，不能重名 */
export function excelTableName(raw: string, used: Set<string>) {
  let name = raw.replace(/[^\p{L}\p{N}_.]/gu, '_').replace(/^(?=[\p{N}.])/u, '_')
  if (!name || /^[A-Z]{1,3}\d+$/i.test(name) || /^[RC]$/i.test(name) || /^R\d*C\d*$/i.test(name))
    name = `_${name || '表'}`
  let unique = name
  for (let i = 2; used.has(unique.toLowerCase()); i++)
    unique = `${name}_${i}`
  used.add(unique.toLowerCase())
  return unique
}

/** 单元格的文本（富文本、公式结果都取显示文字） */
function cellText(v: unknown): string {
  if (v == null)
    return ''
  if (typeof v === 'object') {
    const o = v as { text?: string, richText?: { text: string }[], result?: unknown }
    if (o.richText)
      return o.richText.map(x => x.text).join('')
    if (o.text !== undefined)
      return String(o.text)
    if (o.result !== undefined)
      return cellText(o.result)
    return ''
  }
  return String(v)
}

/** 把表格写进 exceljs 工作表（单元格里的值已经写好）；返回写出的个数，做不到的原因记入 skipped */
export function exportTables(tables: UniverTable[], ws: Worksheet, sheetName: string, used: Set<string>, skipped: string[]): number {
  let n = 0
  for (const t of tables) {
    const r = t.range
    if (t.options?.showHeader === false) {
      skipped.push(`「${sheetName}」表格「${t.name}」（没有标题行）`)
      continue
    }
    const width = r.endColumn - r.startColumn + 1
    if (r.endRow <= r.startRow || width < 1) {
      skipped.push(`「${sheetName}」表格「${t.name}」（只有标题行）`)
      continue
    }
    // 列名：以标题行单元格的文字为准（Excel 要求非空且不重复）
    const seen = new Set<string>()
    const columns = Array.from({ length: width }, (_, j) => {
      const header = ws.getCell(r.startRow + 1, r.startColumn + 1 + j)
      let name = cellText(header.value).trim() || t.columns?.[j]?.displayName || `列${j + 1}`
      for (let i = 2; seen.has(name.toLowerCase()); i++)
        name = `${cellText(header.value).trim() || `列${j + 1}`}${i}`
      seen.add(name.toLowerCase())
      return { name, filterButton: t.options?.showAutoFilter !== false }
    })
    // 数据行：照搬现有单元格（保留公式）；exceljs 会把它们再写一遍
    const rows: unknown[][] = []
    for (let rr = r.startRow + 1; rr <= r.endRow; rr++)
      rows.push(Array.from({ length: width }, (_, j) => ws.getCell(rr + 1, r.startColumn + 1 + j).value))
    try {
      ws.addTable({
        name: t.exactName ? t.name : excelTableName(t.name, used),
        ref: `${colName(r.startColumn)}${r.startRow + 1}`,
        headerRow: true,
        totalsRow: false,
        style: { theme: 'TableStyleMedium2', showRowStripes: true },
        columns,
        rows: rows as never,
      })
      n++
    }
    catch (e) {
      skipped.push(`「${sheetName}」表格「${t.name}」（${e instanceof Error ? e.message : String(e)}）`)
    }
  }
  return n
}

export interface ExcelTableInfo {
  name: string
  range: IRange
  showAutoFilter: boolean
  /** 导入时有内容没带过来（汇总行） */
  note?: string
}

/** 读出 exceljs 工作表里的表格；没有标题行的跳过（原因进 skipped） */
export function readExcelTables(ws: Worksheet, sheetName: string, skipped: string[]): ExcelTableInfo[] {
  const map = (ws as unknown as { tables?: Record<string, { model?: { name?: string, tableRef?: string, headerRow?: boolean, totalsRow?: boolean, columns?: { filterButton?: boolean }[] } }> }).tables ?? {}
  const out: ExcelTableInfo[] = []
  for (const t of Object.values(map)) {
    const m = t.model
    if (!m?.name || !m.tableRef)
      continue
    const ref = parseRef(m.tableRef)
    if (!ref)
      continue
    if (m.headerRow === false) {
      skipped.push(`「${sheetName}」表格「${m.name}」（没有标题行）`)
      continue
    }
    const range = { ...ref }
    let note: string | undefined
    if (m.totalsRow) {
      range.endRow -= 1
      note = '汇总行未带入'
    }
    if (range.endRow <= range.startRow) {
      skipped.push(`「${sheetName}」表格「${m.name}」（没有数据行）`)
      continue
    }
    out.push({ name: m.name, range, showAutoFilter: (m.columns ?? []).some(c => c.filterButton !== false), note })
  }
  return out
}

function parseRef(ref: string): IRange | null {
  const m = ref.replace(/\$/g, '').match(/^([A-Z]+)(\d+):([A-Z]+)(\d+)$/i)
  if (!m)
    return null
  const col = (s: string) => s.toUpperCase().split('').reduce((a, ch) => a * 26 + ch.charCodeAt(0) - 64, 0) - 1
  return { startRow: Number(m[2]) - 1, startColumn: col(m[1]!), endRow: Number(m[4]) - 1, endColumn: col(m[3]!) }
}
