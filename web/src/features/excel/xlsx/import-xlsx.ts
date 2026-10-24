import type { ICellData, IStyleData, IWorksheetData } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'
import type { FWorkbook } from '@univerjs/sheets/facade'
import type { Cell, Workbook as ExcelWorkbook, Worksheet } from 'exceljs'
import type { FeatureStats } from './sheet-features'
import { functionProblems } from '@/features/excel/formula/function-check'
import { emptyOutline, groupsOf, hasOutline, MAX_LEVEL } from '@/features/excel/outline/outline'
import { t as tr } from '@/i18n'
import { fromExcelFormula } from './formula-compat'
import { emptyStats, importSheetFeatures } from './sheet-features'
import { excelToUniverStyle, excelWidthToPx, ptToPx } from './style-map'

/** 单个页签超过这个行数，建议走「导入数据」成为数据集（表格内存模型承载不了） */
export const MAX_SHEET_ROWS = 200_000

export interface ImportedSheet {
  name: string
  rows: number
  cols: number
  formulas: number
  merges: number
  /** 行列分组个数 */
  outlineGroups: number
}

export interface ImportSummary {
  sheets: ImportedSheet[]
  /** 因行数过多未导入的页签 */
  skipped: { name: string, rows: number }[]
  /** 文件里有、但当前未支持导入的内容 */
  unsupported: { images: number }
  /** 导入的条件格式、数据验证、批注、超链接，以及无法转换的规则 */
  features: FeatureStats
  /** 导入后计算结果为 #NAME? 的公式数（使用了不支持的函数） */
  unknownFunctions: number
  /** 这些公式里不支持 / 不认识的函数名（按出现次数从多到少） */
  unknownFunctionNames?: string[]
  /** 命名区域：导入个数与跳过的名称（指向多个区域、未导入的页签或重名） */
  definedNames?: { imported: number, skipped: string[] }
}

/** Excel 日期 → 序列号（1900 日期系统；exceljs 按 UTC 解析日期） */
function dateToSerial(d: Date) {
  return (d.getTime() - Date.UTC(1899, 11, 30)) / 86_400_000
}

const DATE_FORMAT = 'yyyy-mm-dd hh:mm:ss'

function cellToUniver(cell: Cell): { data: ICellData, formula: boolean, arrayRef?: string } | null {
  const v = cell.value as unknown
  const data: ICellData = {}
  let formula = false
  let arrayRef: string | undefined
  if (v == null) {
    // 仅有样式的空单元格（边框、底色）
  }
  else if (typeof v === 'number' || typeof v === 'boolean') {
    data.v = typeof v === 'boolean' ? (v ? 1 : 0) : v
    if (typeof v === 'boolean')
      data.t = 3
  }
  else if (typeof v === 'string') {
    data.v = v
    data.t = 1
  }
  else if (v instanceof Date) {
    data.v = dateToSerial(v)
  }
  else if (typeof v === 'object') {
    const o = v as Record<string, unknown>
    if ('formula' in o || 'sharedFormula' in o) {
      const f = cell.formula
      if (f) {
        // 去掉 Excel 新函数的 _xlfn. / _xlws. / _xlpm. 前缀
        data.f = `=${fromExcelFormula(f)}`
        formula = true
      }
      // 数组公式（含动态数组）：溢出区里是 Excel 存的缓存值，导入后由表格重新计算溢出
      if (o.shareType === 'array' && typeof o.ref === 'string' && o.ref.includes(':'))
        arrayRef = o.ref
      const result = o.result
      if (typeof result === 'number' || typeof result === 'string')
        data.v = result
      else if (result instanceof Date)
        data.v = dateToSerial(result)
      else if (result && typeof result === 'object' && 'error' in result)
        data.v = String((result as { error: string }).error)
    }
    else if ('richText' in o) {
      data.v = (o.richText as { text: string }[]).map(t => t.text).join('')
      data.t = 1
    }
    else if ('text' in o) {
      // 超链接：保留显示文字
      data.v = typeof o.text === 'string' ? o.text : String((o.text as { richText?: { text: string }[] })?.richText?.map(t => t.text).join('') ?? '')
      data.t = 1
    }
    else if ('error' in o) {
      data.v = String(o.error)
    }
  }
  let style = excelToUniverStyle(cell.style)
  if (v instanceof Date && !style?.n)
    style = { ...style, n: { pattern: DATE_FORMAT } }
  if (style)
    data.s = style as IStyleData
  if (data.v === undefined && !data.f && !data.s)
    return null
  return { data, formula, arrayRef }
}

/** "A1:C3" → 0 起的区间 */
function parseRange(ref: string) {
  const m = ref.match(/^([A-Z]+)(\d+):([A-Z]+)(\d+)$/i)
  if (!m)
    return null
  const col = (s: string) => s.toUpperCase().split('').reduce((n, ch) => n * 26 + ch.charCodeAt(0) - 64, 0) - 1
  return { startRow: Number(m[2]) - 1, startColumn: col(m[1]!), endRow: Number(m[4]) - 1, endColumn: col(m[3]!) }
}

function convertSheet(ws: Worksheet): { sheet: Partial<IWorksheetData>, info: ImportedSheet } {
  const cellData: Record<number, Record<number, ICellData>> = {}
  let formulas = 0
  let maxRow = 0
  let maxCol = 0
  const arrays: { row: number, col: number, ref: string }[] = []
  ws.eachRow({ includeEmpty: false }, (row, rowNumber) => {
    row.eachCell({ includeEmpty: false }, (cell, colNumber) => {
      const converted = cellToUniver(cell)
      if (!converted)
        return
      cellData[rowNumber - 1] ??= {}
      cellData[rowNumber - 1]![colNumber - 1] = converted.data
      if (converted.formula)
        formulas++
      if (converted.arrayRef)
        arrays.push({ row: rowNumber - 1, col: colNumber - 1, ref: converted.arrayRef })
      maxRow = Math.max(maxRow, rowNumber)
      maxCol = Math.max(maxCol, colNumber)
    })
  })

  // 溢出区的缓存值去掉（保留样式），否则会挡住公式的溢出结果（#SPILL!）
  for (const a of arrays) {
    const r = parseRange(a.ref)
    if (!r)
      continue
    for (let row = r.startRow; row <= r.endRow; row++) {
      for (let col = r.startColumn; col <= r.endColumn; col++) {
        const cell = cellData[row]?.[col]
        if (!cell || (row === a.row && col === a.col) || cell.f)
          continue
        delete cell.v
        delete cell.t
      }
    }
  }

  const merges = ((ws.model as { merges?: string[] }).merges ?? []).map(parseRange).filter((m): m is NonNullable<typeof m> => !!m)
  const columnData: NonNullable<IWorksheetData['columnData']> = {}
  // 行列分组：读出每行 / 列的大纲级别，全部隐藏的分组记为折叠
  const outline = emptyOutline()
  ws.columns?.forEach((col, i) => {
    if (!col)
      return
    if (col.outlineLevel)
      outline.cols[i] = Math.min(col.outlineLevel, MAX_LEVEL)
    const entry: { w?: number, hd?: 0 | 1 } = {}
    if (col.width)
      entry.w = excelWidthToPx(col.width)
    if (col.hidden)
      entry.hd = 1
    if (Object.keys(entry).length)
      columnData[i] = entry
  })
  const rowData: NonNullable<IWorksheetData['rowData']> = {}
  ws.eachRow({ includeEmpty: true }, (row, rowNumber) => {
    const entry: { h?: number, hd?: 0 | 1 } = {}
    if (row.outlineLevel)
      outline.rows[rowNumber - 1] = Math.min(row.outlineLevel, MAX_LEVEL)
    if (row.height)
      entry.h = ptToPx(row.height)
    if (row.hidden)
      entry.hd = 1
    if (Object.keys(entry).length)
      rowData[rowNumber - 1] = entry
  })

  for (const dim of ['rows', 'cols'] as const) {
    const hidden = (i: number) => (dim === 'rows' ? rowData[i]?.hd : columnData[i]?.hd) === 1
    outline.collapsed[dim] = groupsOf(outline, dim)
      .filter((g) => {
        for (let i = g.start; i <= g.end; i++) {
          if (!hidden(i))
            return false
        }
        return true
      })
      .map(g => `${g.level}@${g.end}`)
  }
  const outlineGroups = groupsOf(outline, 'rows').length + groupsOf(outline, 'cols').length

  const view = ws.views?.[0] as { state?: string, xSplit?: number, ySplit?: number, showGridLines?: boolean } | undefined
  const frozen = view?.state === 'frozen' && ((view.xSplit ?? 0) > 0 || (view.ySplit ?? 0) > 0)
  const sheet: Partial<IWorksheetData> = {
    cellData,
    mergeData: merges,
    columnData,
    rowData,
    rowCount: Math.max(maxRow + 50, 100),
    columnCount: Math.max(maxCol + 5, 26),
    defaultRowHeight: ws.properties.defaultRowHeight ? ptToPx(ws.properties.defaultRowHeight) : 24,
    defaultColumnWidth: ws.properties.defaultColWidth ? excelWidthToPx(ws.properties.defaultColWidth) : 88,
    showGridlines: view?.showGridLines === false ? 0 : 1,
    ...(frozen
      ? { freeze: { xSplit: view!.xSplit ?? 0, ySplit: view!.ySplit ?? 0, startRow: view!.ySplit ?? 0, startColumn: view!.xSplit ?? 0 } }
      : {}),
    hidden: ws.state === 'hidden' || ws.state === 'veryHidden' ? 1 : 0,
    ...(hasOutline(outline) ? { custom: { outline } } : {}),
  }
  return { sheet, info: { name: ws.name, rows: maxRow, cols: maxCol, formulas, merges: merges.length, outlineGroups } }
}

/** 页签名在引用里的写法：含空格、符号时加单引号 */
function sheetRef(name: string) {
  return /^[A-Z_\u4E00-\u9FFF][\w.\u4E00-\u9FFF]*$/i.test(name) ? name : `'${name.replace(/'/g, '\'\'')}'`
}

/**
 * 导入命名区域（如 税率 = 参数!$B$2）：只导入指向导入页签的单个区域；Excel 内置名称（打印区域、筛选区域）跳过；
 * 与工作簿里已有的名称重名时跳过
 */
function importDefinedNames(workbook: FWorkbook, book: ExcelWorkbook, renamed: Map<string, string>): { imported: number, skipped: string[] } {
  const out = { imported: 0, skipped: [] as string[] }
  const model = (book.definedNames as unknown as { model?: { name: string, ranges: string[] }[] }).model ?? []
  const existing = new Set(workbook.getDefinedNames().map(n => n.getName().toLowerCase()))
  for (const { name, ranges } of model) {
    if (!name || name.startsWith('_xlnm.') || name.startsWith('_'))
      continue
    const ref = ranges?.length === 1 ? ranges[0]! : null
    const m = ref?.match(/^'?(.+?)'?!(\$?[A-Z]+\$?\d+(?::\$?[A-Z]+\$?\d+)?)$/i)
    const sheet = m && renamed.get(m[1]!.replace(/''/g, '\''))
    if (!m || !sheet || existing.has(name.toLowerCase())) {
      out.skipped.push(name)
      continue
    }
    try {
      workbook.insertDefinedName(name, `${sheetRef(sheet)}!${m[2]}`)
      existing.add(name.toLowerCase())
      out.imported++
    }
    catch {
      out.skipped.push(name)
    }
  }
  return out
}

/** 与已有页签重名时加序号 */
function uniqueName(name: string, taken: Set<string>) {
  const base = name.slice(0, 28) || 'Sheet'
  let candidate = base
  for (let i = 2; taken.has(candidate); i++)
    candidate = `${base} (${i})`
  taken.add(candidate)
  return candidate
}

/**
 * 导入 .xlsx：每个页签追加为当前工作簿的新页签（不覆盖现有页签，如汇总表）。
 * 保留：值、公式（共享公式展开为各自的相对引用）、样式、数字格式、合并、行高列宽、冻结、隐藏
 */
export async function importXlsx(api: FUniver, data: ArrayBuffer): Promise<ImportSummary> {
  const workbook = api.getActiveWorkbook()
  if (!workbook)
    throw new Error(tr('工作簿尚未就绪'))
  const ExcelJS = (await import('exceljs')).default
  const book = new ExcelJS.Workbook()
  await book.xlsx.load(data)

  const summary: ImportSummary = { sheets: [], skipped: [], unsupported: { images: 0 }, features: emptyStats(), unknownFunctions: 0 }
  const taken = new Set(workbook.getSheets().map(s => s.getSheetName()))
  const created: string[] = []
  /** 原页签名 → 导入后的页签名（重名时加了序号） */
  const renamed = new Map<string, string>()
  for (const ws of book.worksheets) {
    if (ws.actualRowCount > MAX_SHEET_ROWS) {
      summary.skipped.push({ name: ws.name, rows: ws.actualRowCount })
      continue
    }
    summary.unsupported.images += ws.getImages?.().length ?? 0

    const { sheet, info } = convertSheet(ws)
    const name = uniqueName(ws.name, taken)
    renamed.set(ws.name, name)
    const fsheet = workbook.insertSheet(name, { sheet: { ...sheet, name } })
    created.push(fsheet.getSheetId())
    await importSheetFeatures(api, fsheet, ws, summary.features)
    summary.sheets.push({ ...info, name })
  }
  if (created[0])
    workbook.setActiveSheet(created[0])
  summary.definedNames = importDefinedNames(workbook, book, renamed)

  // 统计使用了不支持函数的公式（计算完成后结果为 #NAME?），并点名是哪些函数
  const nameCounts = new Map<string, number>()
  await api.getFormula().onCalculationResultApplied(30_000).catch(() => {})
  for (const id of created) {
    const s = workbook.getSheetBySheetId(id)
    if (!s)
      continue
    const lastRow = s.getLastRow()
    const lastCol = s.getLastColumn()
    if (lastRow < 0 || lastCol < 0)
      continue
    const range = s.getRange(0, 0, lastRow + 1, lastCol + 1)
    const formulas = range.getFormulas()
    range.getRawValues().forEach((row, r) => row.forEach((v, c) => {
      if (v !== '#NAME?')
        return
      summary.unknownFunctions++
      const p = functionProblems(formulas[r]?.[c] ?? '')
      for (const name of [...p.unsupported.map(x => x.name), ...p.unknown])
        nameCounts.set(name, (nameCounts.get(name) ?? 0) + 1)
    }))
  }
  summary.unknownFunctionNames = [...nameCounts.entries()].sort((a, b) => b[1] - a[1]).map(([n]) => n)
  return summary
}
