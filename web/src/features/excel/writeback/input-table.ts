import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import type { FixedColumns } from './fixed-columns'
import type { CellValue, DatasetColumn, DatasetInfo, WritebackCellError, WritebackMode } from '@/api/dataset'
import { getDistinctApi } from '@/api/dataset'
import { isNumericKind } from '@/features/excel/dataset/column-kinds'

/**
 * 填报区：工作表上的一块区域，首行为表头（对应数据集的列），下面逐行填写，提交后写回数据集。
 * 定义保存在工作表元数据 `inputs` 里（随工作簿保存，模板可替换数据集）。
 * 数据行从表头下一行开始，遇到整行空白即结束
 */

export interface InputSpec {
  id: string
  name: string
  datasetId: string
  datasetName: string
  /** 表头左上角 */
  row: number
  col: number
  /** 从左到右各列对应的数据集列名 */
  fields: string[]
  mode: WritebackMode
  /** 按主键更新时的主键列（数据集已登记主键时与之一致） */
  keys: string[]
  /** 固定列：不显示在表格里，提交 / 删除 / 载入时自动带上（收集任务生成的填报区用它放单位和期次） */
  fixed?: FixedColumns
  /** 最近一次成功提交 */
  last?: { batchId: string, at: string, by: string, inserted: number, updated: number }
}

const META_KEY = 'inputs'
/** 单次提交的行数上限（与后端一致） */
export const MAX_INPUT_ROWS = 20_000
/** 表头下方预设格式、下拉的行数 */
const PREPARED_ROWS = 500

const HEADER_BG = '#E2EFDA'
const KEY_BG = '#A9D08E'
const ERROR_BG = '#FDE2E2'

export function getInputs(sheet: FWorksheet | null | undefined): InputSpec[] {
  const custom = sheet?.getCustomMetadata() as Record<string, unknown> | undefined
  const list = custom?.[META_KEY]
  return Array.isArray(list) ? (list as InputSpec[]) : []
}

export function saveInputs(sheet: FWorksheet, list: InputSpec[]) {
  sheet.setCustomMetadata({ ...(sheet.getCustomMetadata() ?? {}), [META_KEY]: list })
}

export function updateInput(sheet: FWorksheet, spec: InputSpec) {
  saveInputs(sheet, getInputs(sheet).map(s => (s.id === spec.id ? spec : s)))
}

/** 移除填报区定义，并去掉放置时加的下拉选项（表格内容保留） */
export function removeInput(sheet: FWorksheet, id: string) {
  const spec = getInputs(sheet).find(s => s.id === id)
  if (spec) {
    const range = sheet.getRange(spec.row + 1, spec.col, PREPARED_ROWS, spec.fields.length) as unknown as { setDataValidation?: (rule: null) => void }
    try {
      range.setDataValidation?.(null)
    }
    catch {
      // 没有下拉时忽略
    }
  }
  saveInputs(sheet, getInputs(sheet).filter(s => s.id !== id))
}

export function colName(c: number) {
  let s = ''
  for (let n = c + 1; n > 0; n = Math.floor((n - 1) / 26))
    s = String.fromCharCode(65 + ((n - 1) % 26)) + s
  return s
}

export function cellName(row: number, col: number) {
  return `${colName(col)}${row + 1}`
}

function blank(v: unknown) {
  return v == null || (typeof v === 'string' && v.trim() === '')
}

/** 数据行数（表头下方连续的非空行） */
export function dataRowCount(sheet: FWorksheet, spec: InputSpec): number {
  const width = spec.fields.length
  const chunk = 500
  let n = 0
  const maxRow = sheet.getMaxRows()
  while (n < MAX_INPUT_ROWS + 1) {
    const start = spec.row + 1 + n
    const size = Math.min(chunk, maxRow - start)
    if (size <= 0)
      break
    const values = sheet.getRange(start, spec.col, size, width).getValues()
    const firstBlank = values.findIndex(r => r.every(blank))
    if (firstBlank >= 0)
      return n + firstBlank
    n += size
  }
  return n
}

/** 选中的格子属于哪个填报区（表头、数据行或紧接其后的空行） */
export function inputAt(sheet: FWorksheet, row: number, col: number): InputSpec | null {
  return getInputs(sheet).find(s => col >= s.col && col < s.col + s.fields.length && row >= s.row && row <= s.row + dataRowCount(sheet, s) + 1) ?? null
}

/** 与已有填报区是否重叠（只看列范围与起始行附近，避免两个填报区共用表头行） */
export function overlaps(sheet: FWorksheet, row: number, col: number, width: number, exceptId?: string) {
  return getInputs(sheet).some((s) => {
    if (s.id === exceptId)
      return false
    const colsOverlap = col < s.col + s.fields.length && s.col < col + width
    const rowsOverlap = row >= s.row && row <= s.row + Math.max(dataRowCount(sheet, s), 1) + 1
    return colsOverlap && (rowsOverlap || (s.row >= row && s.row <= row + 1))
  })
}

function columnOf(ds: DatasetInfo, field: string): DatasetColumn | undefined {
  return ds.columns.find(c => c.title === field)
}

/** 写表头、数据区格式与下拉选项 */
export async function renderInput(api: FUniver, sheet: FWorksheet, spec: InputSpec, ds: DatasetInfo) {
  const width = spec.fields.length
  if (sheet.getMaxRows() < spec.row + PREPARED_ROWS + 2)
    sheet.insertRowsAfter(sheet.getMaxRows() - 1, spec.row + PREPARED_ROWS + 2 - sheet.getMaxRows())
  if (sheet.getMaxColumns() < spec.col + width + 1)
    sheet.insertColumnsAfter(sheet.getMaxColumns() - 1, spec.col + width + 1 - sheet.getMaxColumns())
  sheet.getRange(spec.row, spec.col, 1, width).setValues([spec.fields])
  const header = sheet.getRange(spec.row, spec.col, 1, width)
  header.setFontWeight('bold')
  header.setBackgroundColor(HEADER_BG)
  spec.fields.forEach((f, i) => {
    if (spec.keys.includes(f))
      sheet.getRange(spec.row, spec.col + i).setBackgroundColor(KEY_BG)
    const c = columnOf(ds, f)
    const body = sheet.getRange(spec.row + 1, spec.col + i, PREPARED_ROWS, 1)
    if (c?.kind === 'money')
      body.setNumberFormat('#,##0.00')
    else if (c?.kind === 'text' || c?.kind === 'id')
      body.setNumberFormat('@')
    if (sheet.getColumnWidth(spec.col + i) < 96)
      sheet.setColumnWidth(spec.col + i, 110)
  })
  await addDropdowns(api, sheet, spec, ds)
}

interface ValidationBuilder {
  requireValueInList: (values: string[], multiple?: boolean, showDropdown?: boolean) => ValidationBuilder
  setAllowInvalid: (allow: boolean) => ValidationBuilder
  setOptions: (options: { renderMode?: number }) => ValidationBuilder
  build: () => unknown
}

/** 下拉选项覆盖的行数 */
const DROPDOWN_ROWS = 200
/** 文本列取值不超过这么多种时才加下拉 */
const DROPDOWN_MAX_VALUES = 20

/**
 * 文本列在数据集里取值不多时加下拉选项：允许输入新值（不在列表里的会有提示，便于发现笔误）；
 * 按纯文字显示，避免每一行都出现下拉标签
 */
async function addDropdowns(api: FUniver, sheet: FWorksheet, spec: InputSpec, ds: DatasetInfo) {
  const factory = (api as unknown as { newDataValidation?: () => ValidationBuilder }).newDataValidation
  if (!factory || !ds.rowCount)
    return
  for (const [i, f] of spec.fields.entries()) {
    const c = columnOf(ds, f)
    if (c?.kind !== 'text')
      continue
    try {
      const r = await getDistinctApi(ds.id, { column: c.key })
      const values = r.values.map(v => v.value).filter(Boolean)
      if (!values.length || r.totalDistinct > DROPDOWN_MAX_VALUES)
        continue
      // renderMode 0 = 纯文字（DataValidationRenderMode.TEXT）
      const rule = factory.call(api).requireValueInList(values, false, true).setAllowInvalid(true).setOptions({ renderMode: 0 }).build()
      const range = sheet.getRange(spec.row + 1, spec.col + i, DROPDOWN_ROWS, 1) as unknown as { setDataValidation?: (rule: unknown) => void }
      range.setDataValidation?.(rule)
    }
    catch {
      // 下拉只是辅助，失败不影响填报
    }
  }
}

/**
 * 读出填报数据：数值列取单元格的值（公式取结果），其他列取显示文字（日期按显示格式）。
 * 返回各行在工作表中的行号，用于标出错误
 */
export function readInputRows(sheet: FWorksheet, spec: InputSpec, ds: DatasetInfo): { rows: CellValue[][], sheetRows: number[] } {
  const n = dataRowCount(sheet, spec)
  if (!n)
    return { rows: [], sheetRows: [] }
  const range = sheet.getRange(spec.row + 1, spec.col, n, spec.fields.length)
  const raw = range.getValues()
  const shown = range.getDisplayValues()
  const numeric = spec.fields.map(f => isNumericKind(columnOf(ds, f)?.kind ?? ''))
  const rows = raw.map((r, ri) => r.map((v, ci) => {
    if (blank(v))
      return null
    if (numeric[ci])
      return typeof v === 'number' ? v : String(v)
    return shown[ri]?.[ci] ?? String(v)
  }))
  return { rows, sheetRows: rows.map((_, i) => spec.row + 1 + i) }
}

/** 清空数据行（保留表头） */
export function clearInputRows(sheet: FWorksheet, spec: InputSpec) {
  const n = dataRowCount(sheet, spec)
  if (n)
    sheet.getRange(spec.row + 1, spec.col, n, spec.fields.length).clearContent()
}

/** 用数据集里的行替换数据区 */
export function writeInputRows(sheet: FWorksheet, spec: InputSpec, rows: CellValue[][]) {
  clearInputRows(sheet, spec)
  if (!rows.length)
    return
  if (sheet.getMaxRows() < spec.row + rows.length + 2)
    sheet.insertRowsAfter(sheet.getMaxRows() - 1, spec.row + rows.length + 2 - sheet.getMaxRows())
  sheet.getRange(spec.row + 1, spec.col, rows.length, spec.fields.length).setValues(rows.map(r => r.map(v => v ?? '')))
}

// ── 错误标记：记下原来的背景色，下次提交前恢复 ──
const marks = new Map<string, { sheetId: string, cells: { row: number, col: number, bg: string | null }[] }>()

export function clearErrorMarks(api: FUniver, specId: string) {
  const m = marks.get(specId)
  marks.delete(specId)
  const sheet = m && api.getActiveWorkbook()?.getSheetBySheetId(m.sheetId)
  if (!sheet || !m)
    return
  for (const c of m.cells)
    sheet.getRange(c.row, c.col).setBackgroundColor(c.bg as string)
}

export function markErrors(api: FUniver, sheet: FWorksheet, spec: InputSpec, sheetRows: number[], errors: WritebackCellError[]) {
  clearErrorMarks(api, spec.id)
  const cells: { row: number, col: number, bg: string | null }[] = []
  const seen = new Set<string>()
  for (const e of errors) {
    const row = sheetRows[e.row]
    const ci = spec.fields.indexOf(e.column)
    if (row == null || ci < 0 || seen.has(`${row},${ci}`))
      continue
    seen.add(`${row},${ci}`)
    const range = sheet.getRange(row, spec.col + ci)
    cells.push({ row, col: spec.col + ci, bg: range.getCellStyleData()?.bg?.rgb ?? null })
    range.setBackgroundColor(ERROR_BG)
  }
  marks.set(spec.id, { sheetId: sheet.getSheetId(), cells })
}
