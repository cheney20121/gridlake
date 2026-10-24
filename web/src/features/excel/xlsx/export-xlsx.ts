import type { ICellData, IRange, IStyleData, IWorkbookData, IWorksheetData } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'
import type { CellValue as ExcelCellValue } from 'exceljs'
import type { FeatureStats } from './sheet-features'
import type { UniverTable } from './tables'
import type { FloatLocation } from '@/features/excel/chart/chart-manager'
import type { DataTableSpec } from '@/features/excel/datatable/types'
import type { SheetOutline } from '@/features/excel/outline/outline'
import { getRowsApi, openCursorApi } from '@/api/dataset'
import { chartImages } from '@/features/excel/chart/chart-images'
import { sheetHeaderSize } from '@/features/excel/chart/chart-manager'
import { datasetById } from '@/features/excel/dashboard/slicer'
import { TABLE_FUNCTION, tableColumnsUsed, toExcelFormula } from '@/features/excel/datatable/formula-syntax'
import { listCellTables, listDataTables, tableColumns, tableQuery } from '@/features/excel/datatable/types'
import { groupsOf, maxLevel } from '@/features/excel/outline/outline'
import { t as tr } from '@/i18n'
import { planCellTables } from './cell-tables'
import { toExcelCompatFormula } from './formula-compat'
import { emptyStats, exportSheetFeatures, resource } from './sheet-features'
import { pxToExcelWidth, pxToPt, univerToExcelStyle } from './style-map'
import { exportTables } from './tables'

/** 平台函数：Excel 无法计算，导出时写入计算结果，原公式记在批注里 */
const PLATFORM_FUNCTION = /\b(?:DSUMIFS|DCOUNTIFS|DAVERAGEIFS|DMAXIFS|DMINIFS|DCOUNTDISTINCTIFS|DLOOKUP|IMPORTRANGE)\s*\(/i

export interface ExportSummary {
  sheets: number
  cells: number
  formulas: number
  /** 平台函数公式（已转为数值） */
  platformFormulas: number
  /** 以图片嵌入的图表数 */
  charts: number
  /** 未能导出的图表（标题） */
  chartsFailed: string[]
  /** 条件格式、数据验证、批注、超链接 */
  features: FeatureStats
  /** 导出为 Excel 表格（单独页签）的数据表 */
  dataTables: number
  /** 在原位置包成 Excel 表格的单元格模式数据表 */
  cellTables: number
  /** 行数超过上限、只导出了前若干行的数据表 */
  dataTablesTruncated: string[]
  /** 转为 Excel 原生公式（SUM(表[列]) 等）的数据表函数 */
  tableFormulas: number
  /** 未能转换、导出为数值的数据表函数 */
  tableFormulasAsValue: number
  /** 写成数组公式的动态数组公式（FILTER、UNIQUE 等，按溢出范围） */
  arrayFormulas: number
  /** 导出的行列分组（大纲）个数 */
  outlineGroups: number
  /** 命名区域：导出个数与跳过的名称（不是单元格区域的名称，如常量、公式） */
  definedNames: { exported: number, skipped: string[] }
  fileName: string
}

function colLetters(c: number) {
  let s = ''
  for (let n = c + 1; n > 0; n = Math.floor((n - 1) / 26))
    s = String.fromCharCode(65 + ((n - 1) % 26)) + s
  return s
}

function rangeA1(r: IRange) {
  return `${colLetters(r.startColumn)}${r.startRow + 1}:${colLetters(r.endColumn)}${r.endRow + 1}`
}

interface ArrayFormulaModel {
  getArrayFormulaRange: () => Record<string, Record<string, Record<string, Record<string, IRange>>>> | undefined
}

/**
 * 公式引擎的内部数据（门面接口没有提供溢出范围）。预设里打包的类与单独引入的不是同一个，
 * 不能按类从依赖注入里取，这里按方法名找已创建的实例
 */
function formulaDataModel(api: FUniver): ArrayFormulaModel | null {
  const injector = (api as unknown as { _injector?: { resolvedDependencyCollection?: { resolvedDependencies?: Map<unknown, unknown> } } })._injector
  for (const v of injector?.resolvedDependencyCollection?.resolvedDependencies?.values() ?? []) {
    for (const inst of Array.isArray(v) ? v : [v]) {
      if (inst && typeof (inst as ArrayFormulaModel).getArrayFormulaRange === 'function')
        return inst as ArrayFormulaModel
    }
  }
  return null
}

/** 动态数组公式的溢出范围："行,列" → 区域 */
function spillRanges(api: FUniver, unitId: string, sheetId: string): Map<string, IRange> {
  const out = new Map<string, IRange>()
  try {
    const ranges = formulaDataModel(api)?.getArrayFormulaRange()?.[unitId]?.[sheetId] ?? {}
    for (const [r, cols] of Object.entries(ranges)) {
      for (const [c, range] of Object.entries(cols ?? {})) {
        if (range)
          out.set(`${r},${c}`, range)
      }
    }
  }
  catch {
    // 取不到时按普通公式导出
  }
  return out
}

/** 页签名在引用里的写法 */
function quoteSheet(name: string) {
  return /^[A-Z_\u4E00-\u9FFF][\w.\u4E00-\u9FFF]*$/i.test(name) ? name : `'${name.replace(/'/g, '\'\'')}'`
}

/** 每个数据表最多导出的行数（Excel 能打开百万行，但浏览器生成文件的内存有限） */
export const DATA_TABLE_EXPORT_ROWS = 100_000

interface TableExport {
  loc: FloatLocation<DataTableSpec>
  sheetName: string
  /** Excel 表格名（结构化引用用） */
  excelName: string
  columns: { title: string, kind: string }[]
  rows: (string | number | boolean | null)[][]
  total: number
}

/** Excel 表格名：字母 / 数字 / 下划线 / 点，字母或下划线开头，不能像单元格地址，工作簿内唯一 */
function excelTableName(name: string, used: Set<string>) {
  let n = name.replace(/[^\p{L}\p{N}_.]/gu, '_') || '数据表'
  if (!/^[\p{L}_]/u.test(n) || /^[A-Z]{1,3}\d+$/i.test(n) || /^[RC]\d*$/i.test(n))
    n = `_${n}`
  let out = n
  for (let i = 2; used.has(out.toLowerCase()); i++)
    out = `${n}_${i}`
  used.add(out.toLowerCase())
  return out
}

/** 页签名：去掉 Excel 不允许的字符，最长 31，工作簿内唯一 */
function sheetName(name: string, used: Set<string>) {
  const n = name.replace(/[[\]:*?/\\]/g, '_').slice(0, 28) || '数据表'
  let out = n
  for (let i = 2; used.has(out.toLowerCase()); i++)
    out = `${n}_${i}`
  used.add(out.toLowerCase())
  return out
}

/** 按数据表当前的筛选 / 排序 / 显示的列取数（最多 DATA_TABLE_EXPORT_ROWS 行） */
async function fetchTable(api: FUniver, loc: FloatLocation<DataTableSpec>, sheetNames: Set<string>, tableNames: Set<string>, used: Set<string> | undefined): Promise<TableExport | null> {
  const ds = datasetById(loc.spec.datasetId)
  // 复制一份（未选列时 tableColumns 返回的是数据集目录里的数组）
  const cols = [...tableColumns(loc.spec, ds)]
  if (!ds || !cols.length)
    return null
  // 公式用到、但数据表没显示的列也导出（放在后面），这些公式在 Excel 里才能对应上
  for (const name of used ?? []) {
    const c = ds.columns.find(x => x.title === name || x.key === name)
    if (c && !cols.some(x => x.title === c.title))
      cols.push(c)
  }
  const cur = await openCursorApi(ds.id, tableQuery(api, loc.spec, loc.sheet.getSheetId()))
  const fields = cols.map(c => c.title)
  const limit = Math.min(cur.total, DATA_TABLE_EXPORT_ROWS)
  const rows: (string | number | boolean | null)[][] = []
  for (let off = 0; off < limit; off += 2000) {
    const r = await getRowsApi(ds.id, cur.cursorId, off, Math.min(2000, limit - off), undefined, fields)
    if (r.expired)
      throw new Error(tr('数据表「{0}」的数据在导出期间有更新，请重试', [loc.spec.name]))
    rows.push(...r.rows)
  }
  return {
    loc,
    sheetName: sheetName(loc.spec.name, sheetNames),
    excelName: excelTableName(loc.spec.name, tableNames),
    columns: cols.map(c => ({ title: c.title, kind: c.kind })),
    rows,
    total: cur.total,
  }
}

function resolveStyle(s: ICellData['s'], styles: IWorkbookData['styles']): IStyleData | null {
  if (!s)
    return null
  return typeof s === 'string' ? (styles?.[s] ?? null) as IStyleData | null : s as IStyleData
}

export function downloadBlob(blob: Blob, fileName: string) {
  const a = document.createElement('a')
  a.href = URL.createObjectURL(blob)
  a.download = fileName
  a.click()
  setTimeout(() => URL.revokeObjectURL(a.href), 10_000)
}

/**
 * 当前工作簿生成 .xlsx：多页签、公式（带缓存结果）、样式、合并单元格、行高列宽、冻结、隐藏行列 / 页签
 * exceljs 体积较大，按需加载
 */
export async function buildWorkbookXlsx(api: FUniver, fileName: string): Promise<{ blob: Blob, summary: ExportSummary }> {
  const workbook = api.getActiveWorkbook()
  if (!workbook)
    throw new Error(tr('工作簿尚未就绪'))
  const ExcelJS = (await import('exceljs')).default
  const snapshot = workbook.save() as IWorkbookData
  const out = new ExcelJS.Workbook()
  out.creator = 'Gridlake'
  out.created = new Date()
  // 打开时全部重算：数据表公式转成 SUM(表[列]) 等之后以 Excel 的计算为准（缓存值 0 等也不会丢）
  out.calcProperties = { ...out.calcProperties, fullCalcOnLoad: true }

  // 图表转图片（Excel 打开后是静态图片；在平台里仍是可刷新的图表）
  const { images, failed } = await chartImages(api)
  const summary: ExportSummary = { sheets: 0, cells: 0, formulas: 0, platformFormulas: 0, charts: 0, chartsFailed: failed, features: emptyStats(), dataTables: 0, cellTables: 0, dataTablesTruncated: [], tableFormulas: 0, tableFormulasAsValue: 0, arrayFormulas: 0, outlineGroups: 0, definedNames: { exported: 0, skipped: [] }, fileName }

  // 数据表：先取数（每个数据表导出为单独的页签 + Excel 表格），公式据此转为 Excel 原生写法
  const sheetNames = new Set(snapshot.sheetOrder.map(id => String(snapshot.sheets[id]?.name ?? '').slice(0, 31).toLowerCase()))
  const tableNames = new Set<string>()
  const tables: TableExport[] = []
  const used = new Map<string, Set<string>>()
  for (const sheet of workbook.getSheets()) {
    const lr = sheet.getLastRow()
    const lc = sheet.getLastColumn()
    if (lr >= 0 && lc >= 0)
      sheet.getRange(0, 0, lr + 1, lc + 1).getFormulas().flat().forEach(f => f && tableColumnsUsed(f, used))
  }
  for (const loc of listDataTables(api)) {
    const t = await fetchTable(api, loc, sheetNames, tableNames, used.get(loc.spec.name.trim().toLowerCase()))
    if (t)
      tables.push(t)
  }
  // 单元格模式的数据表：格子里的数据与平台一致时在原位置包成 Excel 表格（见 cell-tables）
  const cellPlan = planCellTables(listCellTables(api).map(({ sheet, spec }) => {
    const cells = snapshot.sheets[sheet.getSheetId()]?.cellData ?? {}
    const width = spec.extent?.cols ?? 0
    return {
      sheetId: sheet.getSheetId(),
      name: spec.name,
      row: spec.row,
      col: spec.col,
      extent: spec.extent,
      total: spec.total,
      edited: Object.keys(spec.edits ?? {}).length > 0,
      columns: tableColumns(spec, datasetById(spec.datasetId)).map(c => c.title),
      header: Array.from({ length: width }, (_, j) => String(cells[spec.row]?.[spec.col + j]?.v ?? '')),
      occupied: (resource<{ tables?: UniverTable[] }>(snapshot, 'SHEET_TABLE_PLUGIN')[sheet.getSheetId()]?.tables ?? []).map(t => t.range),
    }
  }), name => excelTableName(name, tableNames))
  // 表格（超级表）的导出名要避开上面数据表的 Excel 表格名
  const usedTableNames = new Set([...tables.map(t => t.excelName), ...cellPlan.exports.map(t => t.excelName)].map(n => n.toLowerCase()))
  // 完整导出（未截断、至少一行）的数据表才能把公式转成 Excel 写法，否则结果会和平台里不一致
  const formulaTable = (name: string) => {
    const key = name.trim().toLowerCase()
    const t = tables.find(x => x.loc.spec.name.trim().toLowerCase() === key)
    if (t)
      return t.rows.length && t.rows.length === t.total ? { name: t.excelName, columns: new Set(t.columns.map(c => c.title)) } : null
    const c = cellPlan.exports.find(x => x.name.trim().toLowerCase() === key)
    return c ? { name: c.excelName, columns: c.columns } : null
  }
  /** 公式转不成 Excel 写法时批注里的原因：引用了不能包成表格的单元格模式数据表时写明是哪一条 */
  const asValueReason = (formula: string) => {
    const refs = new Map<string, Set<string>>()
    tableColumnsUsed(formula, refs)
    for (const key of refs.keys()) {
      const reason = cellPlan.reasons.get(key)
      if (reason)
        return reason
    }
    return tr('数据表未完整导出或写法无法对应')
  }
  for (const sheetId of snapshot.sheetOrder) {
    const data = snapshot.sheets[sheetId] as IWorksheetData | undefined
    const fsheet = workbook.getSheetBySheetId(sheetId)
    if (!data || !fsheet)
      continue
    summary.sheets++
    const ws = out.addWorksheet(data.name.slice(0, 31), {
      state: data.hidden ? 'hidden' : 'visible',
      views: data.freeze && (data.freeze.xSplit > 0 || data.freeze.ySplit > 0)
        ? [{ state: 'frozen', xSplit: data.freeze.xSplit, ySplit: data.freeze.ySplit, showGridLines: data.showGridlines !== 0 }]
        : [{ showGridLines: data.showGridlines !== 0 }],
    })
    if (data.defaultColumnWidth)
      ws.properties.defaultColWidth = pxToExcelWidth(data.defaultColumnWidth)
    if (data.defaultRowHeight)
      ws.properties.defaultRowHeight = pxToPt(data.defaultRowHeight)
    if (data.tabColor)
      ws.properties.tabColor = { argb: (data.tabColor.replace('#', 'FF')).toUpperCase() }

    // 公式按单元格取（共享公式已展开为各自的相对引用），计算结果用原始值
    const lastRow = fsheet.getLastRow()
    const lastCol = fsheet.getLastColumn()
    const formulas = lastRow >= 0 && lastCol >= 0 ? fsheet.getRange(0, 0, lastRow + 1, lastCol + 1).getFormulas() : []
    const values = lastRow >= 0 && lastCol >= 0 ? fsheet.getRange(0, 0, lastRow + 1, lastCol + 1).getRawValues() : []
    const spills = spillRanges(api, workbook.getId(), sheetId)

    for (const [rKey, row] of Object.entries(data.cellData ?? {})) {
      const r = Number(rKey)
      for (const [cKey, cell] of Object.entries(row ?? {}) as [string, ICellData | undefined][]) {
        const c = Number(cKey)
        if (!cell)
          continue
        const target = ws.getCell(r + 1, c + 1)
        const formula = formulas[r]?.[c] || (typeof cell.f === 'string' ? cell.f : '')
        const raw = values[r]?.[c] ?? cell.v ?? null
        const result = typeof raw === 'number' || typeof raw === 'string' || typeof raw === 'boolean' ? raw : undefined
        if (formula && formula.startsWith('=')) {
          if (PLATFORM_FUNCTION.test(formula)) {
            target.value = result ?? null
            target.note = tr('平台函数（Excel 无法计算，已导出为数值）\n原公式：{0}', [formula])
            summary.platformFormulas++
            // 溢出到相邻格子的结果（如 IMPORTRANGE 取回的整块区域）也按值写出；
            // 溢出部分只在公式引擎里，不在单元格原始值中，按格取计算结果
            const spill = spills.get(`${r},${c}`)
            if (spill) {
              const spilled = fsheet.getRange(spill.startRow, spill.startColumn, spill.endRow - spill.startRow + 1, spill.endColumn - spill.startColumn + 1).getValues()
              for (let sr = spill.startRow; sr <= spill.endRow; sr++) {
                for (let sc = spill.startColumn; sc <= spill.endColumn; sc++) {
                  const v = spilled[sr - spill.startRow]?.[sc - spill.startColumn]
                  if ((sr !== r || sc !== c) && (typeof v === 'number' || typeof v === 'string' || typeof v === 'boolean'))
                    ws.getCell(sr + 1, sc + 1).value = v
                }
              }
            }
          }
          else if (TABLE_FUNCTION.test(formula)) {
            // 数据表函数：数据表已导出为 Excel 表格，转为 SUM(表[列]) 等原生公式
            const excel = toExcelFormula(formula, formulaTable)
            if (excel) {
              // Excel 2007 之后新增的函数在文件里要带 _xlfn. 前缀，否则打开显示 #NAME?
              target.value = { formula: toExcelCompatFormula(excel.slice(1)), result } as ExcelCellValue
              summary.tableFormulas++
            }
            else {
              target.value = result ?? null
              target.note = tr('数据表函数（{0}，已导出为数值）\n原公式：{1}', [asValueReason(formula), formula])
              summary.tableFormulasAsValue++
            }
          }
          else {
            // 新函数加 _xlfn. 等前缀；动态数组按溢出范围写成数组公式
            const excel = toExcelCompatFormula(formula.slice(1))
            const spill = spills.get(`${r},${c}`)
            if (spill && (spill.endRow > spill.startRow || spill.endColumn > spill.startColumn)) {
              target.value = { formula: excel, result, shareType: 'array', ref: rangeA1(spill) } as unknown as ExcelCellValue
              summary.arrayFormulas++
            }
            else {
              target.value = { formula: excel, result } as ExcelCellValue
            }
            summary.formulas++
          }
        }
        else if (cell.v != null && cell.v !== '') {
          target.value = cell.t === 1 ? String(cell.v) : (cell.v as ExcelCellValue)
        }
        else if (cell.p) {
          // 富文本：导出纯文本
          target.value = typeof raw === 'string' ? raw : (cell.p.body?.dataStream ?? '').replace(/[\r\n]+$/, '').replace(/\r/g, '\n')
        }
        const style = univerToExcelStyle(resolveStyle(cell.s, snapshot.styles))
        if (style)
          target.style = style
        summary.cells++
      }
    }

    for (const m of data.mergeData ?? [])
      ws.mergeCells(m.startRow + 1, m.startColumn + 1, m.endRow + 1, m.endColumn + 1)

    for (const [cKey, col] of Object.entries(data.columnData ?? {})) {
      const column = ws.getColumn(Number(cKey) + 1)
      if (col?.w)
        column.width = pxToExcelWidth(col.w)
      if (col?.hd)
        column.hidden = true
    }
    for (const [rKey, row] of Object.entries(data.rowData ?? {})) {
      const target = ws.getRow(Number(rKey) + 1)
      if (row?.h)
        target.height = pxToPt(row.h)
      if (row?.hd)
        target.hidden = true
    }

    // 行列分组：每行 / 列的大纲级别（折叠即隐藏，已随行列隐藏导出）
    const outline = (data.custom as { outline?: SheetOutline } | undefined)?.outline
    if (outline) {
      for (const [i, l] of Object.entries(outline.rows ?? {}))
        ws.getRow(Number(i) + 1).outlineLevel = l
      for (const [i, l] of Object.entries(outline.cols ?? {}))
        ws.getColumn(Number(i) + 1).outlineLevel = l
      ws.properties.outlineLevelRow = maxLevel(outline, 'rows')
      ws.properties.outlineLevelCol = maxLevel(outline, 'cols')
      summary.outlineGroups += groupsOf(outline, 'rows').length + groupsOf(outline, 'cols').length
    }

    exportSheetFeatures(snapshot, sheetId, ws, summary.features, usedTableNames)
    // 单元格模式的数据表：单元格已写好，在原位置包成 Excel 表格（表名在改写公式前已分配）
    const own = cellPlan.exports.filter(t => t.sheetId === sheetId)
    if (own.length) {
      const skipped: string[] = []
      summary.cellTables += exportTables(own.map(t => ({ name: t.excelName, range: t.range, exactName: true })), ws, ws.name, usedTableNames, skipped)
      summary.features.skipped.push(...skipped)
    }

    // 图表：像素位置（画布坐标，含行号列 / 列标行）→ 单元格锚点（可带小数）
    const header = sheetHeaderSize(fsheet)
    const fs = fsheet as unknown as { getColumnWidth: (c: number) => number, getRowHeight: (r: number) => number }
    const toAnchor = (px: number, size: (i: number) => number) => {
      let i = 0
      let rest = Math.max(0, px)
      while (i < 10000) {
        const w = size(i) || 1
        if (rest < w)
          return i + rest / w
        rest -= w
        i++
      }
      return i
    }
    for (const img of images.filter(x => x.sheetId === sheetId)) {
      const id = out.addImage({ base64: img.base64, extension: 'png' })
      ws.addImage(id, {
        tl: { col: toAnchor(img.position.left - header.width, c => fs.getColumnWidth(c)), row: toAnchor(img.position.top - header.height, r => fs.getRowHeight(r)) },
        ext: { width: img.position.width, height: img.position.height },
      } as never)
      summary.charts++
    }

    // 数据表原位置：左上角单元格为空时放一个跳转到数据页签的链接
    for (const t of tables.filter(x => x.loc.sheet.getSheetId() === sheetId)) {
      const r = Math.floor(toAnchor(t.loc.position.top - header.height, i => fs.getRowHeight(i)))
      const c = Math.floor(toAnchor(t.loc.position.left - header.width, i => fs.getColumnWidth(i)))
      const existing = data.cellData?.[r]?.[c]
      if (existing && (existing.v != null || existing.f))
        continue
      const cell = ws.getCell(r + 1, c + 1)
      cell.value = { text: tr('▸ 数据表「{0}」{1} 行，见页签「{2}」', [t.loc.spec.name, t.total.toLocaleString(), t.sheetName]), hyperlink: `#'${t.sheetName.replace(/'/g, '\'\'')}'!A1` } as ExcelCellValue
      cell.font = { color: { argb: 'FF217346' }, underline: true }
    }
  }

  // 数据表页签：Excel 表格（带筛选按钮、隔行底色），金额列千分位两位小数
  for (const t of tables) {
    const ws = out.addWorksheet(t.sheetName)
    if (t.rows.length) {
      ws.addTable({
        name: t.excelName,
        ref: 'A1',
        headerRow: true,
        style: { theme: 'TableStyleLight9', showRowStripes: true },
        columns: t.columns.map(c => ({ name: c.title, filterButton: true })),
        rows: t.rows,
      })
    }
    else {
      ws.addRow(t.columns.map(c => c.title)).font = { bold: true }
    }
    t.columns.forEach((c, i) => {
      const col = ws.getColumn(i + 1)
      col.width = Math.min(40, Math.max(10, c.title.length * 2 + 4))
      if (c.kind === 'money')
        col.numFmt = '#,##0.00'
    })
    ws.views = [{ state: 'frozen', ySplit: 1 }]
    if (t.rows.length < t.total) {
      summary.dataTablesTruncated.push(t.loc.spec.name)
      ws.getCell(1, t.columns.length + 2).value = tr('只导出了前 {0} 行（共 {1} 行）；完整明细请在数据集里导出', [t.rows.length.toLocaleString(), t.total.toLocaleString()])
    }
    summary.dataTables++
  }

  // 命名区域：只导出指向单元格区域的名称（常量、公式类名称 exceljs 写不进去）
  const exported = new Set(out.worksheets.map(w => w.name))
  for (const n of workbook.getDefinedNames()) {
    const ref = n.getFormulaOrRefString().replace(/^=/, '').trim()
    const m = ref.match(/^'?(.+?)'?!(\$?[A-Z]+\$?\d+(?::\$?[A-Z]+\$?\d+)?)$/i)
    const sheet = m?.[1]?.replace(/''/g, '\'').slice(0, 31)
    if (!m || !sheet || !exported.has(sheet)) {
      summary.definedNames.skipped.push(n.getName())
      continue
    }
    // Excel 要求绝对引用
    const abs = m[2]!.replace(/\$?([A-Z]+)\$?(\d+)/gi, '$$$1$$$2')
    out.definedNames.add(`${quoteSheet(sheet)}!${abs}`, n.getName())
    summary.definedNames.exported++
  }

  const buffer = await out.xlsx.writeBuffer()
  return { blob: new Blob([buffer], { type: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet' }), summary }
}

/** 导出并下载 */
export async function exportWorkbookXlsx(api: FUniver, fileName: string): Promise<ExportSummary> {
  const { blob, summary } = await buildWorkbookXlsx(api, fileName)
  downloadBlob(blob, fileName)
  return summary
}
