import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import type { EditValue } from './edits'
import type { CellTableSpec, DataTableSpec } from './types'
import type { CellValue, DatasetFilter, DatasetSort } from '@/api/dataset'
import type { FloatLocation } from '@/features/excel/chart/chart-manager'
import { getRowsApi, openCursorApi } from '@/api/dataset'
import { excelPattern } from '@/components/excel/big-grid/column-style'
import { chartBus, insertFloat, removeFloat, sheetHeaderSize } from '@/features/excel/chart/chart-manager'
import { datasetById } from '@/features/excel/dashboard/slicer'
import { cellName } from '@/features/excel/writeback/input-table'
import { applyEdits, editCount, recordEdit, rowKey } from './edits'
import { cellPixel, cellTableArea, DATA_TABLE_KEY, DEFAULT_TABLE_SIZE, findCellTable, getCellTables, listCellTables, MAX_CELL_ROWS, newDataTableId, saveCellTables, tableColumns, tableQuery, uniqueTableName } from './types'

/**
 * 单元格模式数据表的写入、刷新与模式切换。
 *
 * - 写入：表头 + 数据行写成普通单元格；行数变少时清掉多出来的旧内容
 * - 不覆盖别人的内容：新占用的格子里已有内容时停止写入并提示（与 Excel 的 #SPILL! 一致）
 * - 刷新时机：打开工作簿、本页切片器变化、数据集更新（导入 / 同步 / 改计算列）、修改定义
 */

const HEADER_BG = '#E2EFDA'
const PAGE = 2000

export class CellTableBlockedError extends Error {}

/** 正在由表格自己写入单元格：这些写入不是用户的手工调整，不记录 */
let writing = 0

/** 同一个表最多记录的手工调整个数（再多会让工作簿的元数据膨胀） */
const MAX_EDITS = 5000

function blank(v: unknown) {
  return v == null || v === ''
}

async function fetchRows(api: FUniver, spec: DataTableSpec, sheetId: string) {
  const ds = datasetById(spec.datasetId)
  if (!ds)
    throw new Error(`数据表「${spec.name}」的数据集已删除`)
  const cols = tableColumns(spec, ds)
  if (!cols.length)
    throw new Error(`数据表「${spec.name}」没有可显示的列`)
  const cur = await openCursorApi(ds.id, tableQuery(api, spec, sheetId))
  const limit = Math.min(cur.total, MAX_CELL_ROWS)
  const fields = cols.map(c => c.title)
  const rows: CellValue[][] = []
  for (let off = 0; off < limit; off += PAGE) {
    const r = await getRowsApi(ds.id, cur.cursorId, off, Math.min(PAGE, limit - off), undefined, fields)
    if (r.expired)
      throw new Error(`数据表「${spec.name}」取数期间数据有更新，请重试`)
    rows.push(...r.rows)
  }
  return { cols, rows, total: cur.total }
}

/** 新区域里、旧区域外的格子是否已有内容（有则不能写） */
function findBlocker(sheet: FWorksheet, spec: CellTableSpec, rows: number, cols: number) {
  const old = spec.extent
  const values = sheet.getRange(spec.row, spec.col, rows, cols).getValues()
  for (let r = 0; r < rows; r++) {
    for (let c = 0; c < cols; c++) {
      const inOld = old && r < old.rows && c < old.cols
      if (!inOld && !blank(values[r]?.[c]))
        return cellName(spec.row + r, spec.col + c)
    }
  }
  return null
}

function ensureSize(sheet: FWorksheet, rows: number, cols: number) {
  if (sheet.getMaxRows() < rows)
    sheet.insertRowsAfter(sheet.getMaxRows() - 1, rows - sheet.getMaxRows())
  if (sheet.getMaxColumns() < cols)
    sheet.insertColumnsAfter(sheet.getMaxColumns() - 1, cols - sheet.getMaxColumns())
}

/** 清掉旧区域里不再属于表格的部分（行数、列数变少） */
function clearShrunk(sheet: FWorksheet, spec: CellTableSpec, rows: number, cols: number) {
  const old = spec.extent
  if (!old)
    return
  if (old.rows > rows)
    sheet.getRange(spec.row + rows, spec.col, old.rows - rows, old.cols).clear()
  if (old.cols > cols)
    sheet.getRange(spec.row, spec.col + cols, Math.min(old.rows, rows), old.cols - cols).clear()
}

/**
 * 按定义取数并写入单元格，返回更新后的定义（extent、total）；不保存定义，由调用方保存。
 * 新区域被已有内容挡住时抛 CellTableBlockedError，工作表不做任何改动
 */
export async function renderCellTable(api: FUniver, sheet: FWorksheet, spec: CellTableSpec): Promise<CellTableSpec> {
  const fetched = await fetchRows(api, spec, sheet.getSheetId())
  const { cols, total } = fetched
  // 保留手工调整：按主键把记下的手改覆盖到取到的数据上（与数据集一致的记录顺手清掉）
  const kept = spec.keepEdits
    ? applyEdits(fetched.rows, cols.map(c => c.title), datasetById(spec.datasetId)?.primaryKey ?? [], spec.edits)
    : { rows: fetched.rows, edits: spec.edits }
  const rows = kept.rows
  const height = rows.length + 1
  const width = cols.length
  ensureSize(sheet, spec.row + height + 1, spec.col + width + 1)
  const blocker = findBlocker(sheet, spec, height, width)
  if (blocker)
    throw new CellTableBlockedError(`数据表「${spec.name}」需要 ${cellName(spec.row, spec.col)}:${cellName(spec.row + height - 1, spec.col + width - 1)}，但 ${blocker} 已有内容。请清空该区域或换个位置`)
  writing++
  try {
    clearShrunk(sheet, spec, height, width)
    sheet.getRange(spec.row, spec.col, height, width).setValues([cols.map(c => c.title), ...rows.map(r => r.map(v => v ?? ''))])
    const header = sheet.getRange(spec.row, spec.col, 1, width)
    header.setFontWeight('bold')
    header.setBackgroundColor(HEADER_BG)
    if (rows.length) {
      cols.forEach((c, i) => {
        // 设置过列样式的列（哪怕只剩空记录）每次刷新都按样式写数字格式，撤销后才能回到默认；其余只给金额列加千分位
        const style = spec.columnStyles?.[c.title]
        const custom = style?.numFmt ? excelPattern(style.numFmt) : null
        const pattern = custom ?? (c.kind === 'money' ? '#,##0.00' : style ? 'General' : null)
        if (pattern)
          sheet.getRange(spec.row + 1, spec.col + i, rows.length, 1).setNumberFormat(pattern)
      })
    }
  }
  finally {
    writing--
  }
  const edits = kept.edits && editCount(kept.edits) ? kept.edits : undefined
  return { ...spec, extent: { rows: height, cols: width }, total, edits }
}

/** 保存一个单元格模式数据表的定义（新增或替换） */
function upsertSpec(sheet: FWorksheet, spec: CellTableSpec) {
  const list = getCellTables(sheet)
  saveCellTables(sheet, list.some(s => s.id === spec.id) ? list.map(s => (s.id === spec.id ? spec : s)) : [...list, spec])
}

/** 在当前工作表的选中单元格处新建单元格模式数据表并写入；挡住时抛 CellTableBlockedError，不做任何改动 */
export async function insertCellTable(api: FUniver, init: { datasetId: string, datasetName: string, name?: string, fields?: string[], filters?: DatasetFilter[], sort?: DatasetSort[], keyword?: string }) {
  const sheet = api.getActiveWorkbook()?.getActiveSheet()
  if (!sheet)
    throw new Error('没有打开的工作表')
  const cell = sheet.getSelection()?.getActiveRange()
  const spec: CellTableSpec = {
    v: 1,
    id: newDataTableId(),
    kind: 'datatable',
    name: init.name ?? uniqueTableName(api, init.datasetName),
    datasetId: init.datasetId,
    fields: init.fields ?? [],
    filters: init.filters ?? [],
    sort: init.sort ?? [],
    keyword: init.keyword || undefined,
    followSlicers: true,
    row: cell?.getRow() ?? 0,
    col: cell?.getColumn() ?? 0,
  }
  const next = await renderCellTable(api, sheet, spec)
  upsertSpec(sheet, next)
  chartBus.emit('cell-table', next.id)
  return next
}

/** 修改定义并重新写入（表名、列、筛选、排序等） */
export async function updateCellTable(api: FUniver, spec: CellTableSpec) {
  const loc = findCellTable(api, spec.id)
  if (!loc)
    throw new Error('数据表已被删除')
  // 手工调整以工作表里记下的为准（表单里是打开时的旧副本）；关掉「保留手工调整」就清掉记录
  const next = await renderCellTable(api, loc.sheet, { ...spec, row: loc.spec.row, col: loc.spec.col, extent: loc.spec.extent, edits: spec.keepEdits ? loc.spec.edits : undefined })
  upsertSpec(loc.sheet, next)
  chartBus.emit('cell-table', next.id)
  return next
}

/** 删除单元格模式数据表；keepValues 为真时只解除关联，单元格内容保留为普通数据 */
export function removeCellTable(api: FUniver, id: string, keepValues = false) {
  const loc = findCellTable(api, id)
  if (!loc)
    return
  if (!keepValues) {
    const a = cellTableArea(loc.spec)
    loc.sheet.getRange(a.row, a.col, a.rows, a.cols).clear()
  }
  saveCellTables(loc.sheet, getCellTables(loc.sheet).filter(s => s.id !== id))
  chartBus.emit('cell-table', id)
}

/** 清掉记下的手工调整，按数据集重新写入 */
export async function resetCellTableEdits(api: FUniver, id: string) {
  const loc = findCellTable(api, id)
  if (!loc)
    throw new Error('数据表已被删除')
  const next = await renderCellTable(api, loc.sheet, { ...loc.spec, edits: undefined })
  upsertSpec(loc.sheet, next)
  chartBus.emit('cell-table', id)
  return next
}

interface MutationParams {
  unitId?: string
  subUnitId?: string
  cellValue?: Record<string, Record<string, { v?: unknown, f?: unknown, s?: unknown } | null>>
}

/**
 * 用户在单元格模式数据表里改了格子：记到定义里（按主键认行），刷新时再覆盖回去。
 * 只看用户操作产生的写入（公式计算结果、表格自己的写入不算）；改主键列、表头、超出表格的格子不记
 */
function recordCellEdits(api: FUniver, p: MutationParams) {
  if (writing > 0 || !p.cellValue || !p.subUnitId)
    return
  const sheet = api.getActiveWorkbook()?.getSheetBySheetId(p.subUnitId)
  if (!sheet)
    return
  const tables = getCellTables(sheet).filter(t => t.keepEdits && t.extent)
  if (!tables.length)
    return
  let list = getCellTables(sheet)
  let changed = false
  for (const [rs, cols] of Object.entries(p.cellValue)) {
    const r = Number(rs)
    for (const [cs, cell] of Object.entries(cols ?? {})) {
      const c = Number(cs)
      // 只有改了内容的（样式等不算）
      if (!cell || (!('v' in cell) && !('f' in cell)))
        continue
      const t = list.find(x => x.keepEdits && x.extent && r > x.row && r < x.row + x.extent.rows && c >= x.col && c < x.col + x.extent.cols)
      if (!t)
        continue
      const ds = datasetById(t.datasetId)
      const shown = tableColumns(t, ds)
      const keyIdx = (ds?.primaryKey ?? []).map(k => shown.findIndex(x => x.title === k))
      const title = shown[c - t.col]?.title
      // 没有主键 / 主键列没显示 / 改的就是主键列：无法认行
      if (!title || !keyIdx.length || keyIdx.includes(-1) || keyIdx.includes(c - t.col) || keyIdx.some(i => p.cellValue![rs]?.[String(t.col + i)]))
        continue
      if (editCount(t.edits) >= MAX_EDITS)
        continue
      const key = rowKey(keyIdx.map(i => sheet.getRange(r, t.col + i, 1, 1).getValue() as string | number | boolean | null))
      const value: EditValue = typeof cell.f === 'string' && cell.f ? cell.f : cell.v == null ? '' : (cell.v as EditValue)
      list = list.map(x => (x.id === t.id ? { ...x, edits: recordEdit(x.edits, key, title, value) } : x))
      changed = true
    }
  }
  if (changed)
    saveCellTables(sheet, list)
}

/** 浮动对象左上角所在的单元格 */
function cellAt(sheet: FWorksheet, x: number, y: number) {
  const s = sheet as unknown as { getColumnWidth: (c: number) => number, getRowHeight: (r: number) => number }
  const header = sheetHeaderSize(sheet)
  let col = 0
  for (let left = header.width; col < sheet.getMaxColumns() - 1 && left + s.getColumnWidth(col) <= x; col++)
    left += s.getColumnWidth(col)
  let row = 0
  for (let top = header.height; row < sheet.getMaxRows() - 1 && top + s.getRowHeight(row) <= y; row++)
    top += s.getRowHeight(row)
  return { row, col }
}

/** 浮动模式 → 单元格模式：从数据表左上角所在的格子开始写，成功后移除浮动对象 */
export async function floatToCells(api: FUniver, loc: FloatLocation<DataTableSpec>) {
  const { row, col } = cellAt(loc.sheet, loc.position.left, loc.position.top)
  const spec = await renderCellTable(api, loc.sheet, { ...loc.spec, row, col })
  upsertSpec(loc.sheet, spec)
  removeFloat(api, loc.spec.id)
  chartBus.emit('cell-table', spec.id)
  return spec
}

/** 单元格模式 → 浮动模式：清掉写入的单元格，在原位置放回浮动数据表 */
export function cellsToFloat(api: FUniver, id: string) {
  const loc = findCellTable(api, id)
  if (!loc)
    throw new Error('数据表已被删除')
  const { row, col, extent: _extent, total: _total, ...base } = loc.spec
  removeCellTable(api, id)
  const at = cellPixel(loc.sheet, row, col)
  insertFloat(api, DATA_TABLE_KEY, base, DEFAULT_TABLE_SIZE, loc.sheet, { x: at.x + 1, y: at.y + 1 })
  chartBus.emit('cell-table', id)
  return base as DataTableSpec
}

type Wrap = <T>(fn: () => Promise<T>) => Promise<T>

/**
 * 刷新符合条件的单元格模式数据表（同一个表的刷新串行，过时的结果丢弃）。
 * wrap：包一层（如打开工作簿时不记为用户修改）
 */
const running = new Map<string, Promise<unknown>>()
export async function refreshCellTables(api: FUniver, match: (spec: CellTableSpec, sheetId: string) => boolean, opts: { wrap?: Wrap, onError?: (e: unknown, spec: CellTableSpec) => void } = {}) {
  const wrap: Wrap = opts.wrap ?? (fn => fn())
  const targets = listCellTables(api).filter(t => match(t.spec, t.sheet.getSheetId()))
  await Promise.all(targets.map(async ({ spec }) => {
    const prev = running.get(spec.id) ?? Promise.resolve()
    const job = prev.catch(() => {}).then(() => wrap(async () => {
      // 排队期间定义可能被改过：按最新的定义刷新
      const loc = findCellTable(api, spec.id)
      if (!loc)
        return
      try {
        upsertSpec(loc.sheet, await renderCellTable(api, loc.sheet, loc.spec))
      }
      catch (e) {
        opts.onError?.(e, loc.spec)
      }
    }))
    running.set(spec.id, job)
    await job
    if (running.get(spec.id) === job)
      running.delete(spec.id)
  }))
  if (targets.length)
    chartBus.emit('cell-table')
}

/** 切片器、数据集变化时自动刷新；返回取消监听的函数 */
export function watchCellTables(api: FUniver, onError: (e: unknown, spec: CellTableSpec) => void) {
  const offSlicer = chartBus.on('slicer', (sheetId: string) => {
    refreshCellTables(api, (s, sid) => sid === sheetId && s.followSlicers !== false, { onError })
  })
  const offDataset = chartBus.on('refresh-dataset', (datasetId?: string) => {
    refreshCellTables(api, s => !datasetId || s.datasetId === datasetId, { onError })
  })
  const edit = api.addEvent(api.Event.CommandExecuted, (e) => {
    // 用户操作产生的写入：编辑、粘贴、删除内容；公式计算结果带 fromFormula 标记
    const opts = e.options as { fromFormula?: boolean, onlyLocal?: boolean } | undefined
    if (e.id === 'sheet.mutation.set-range-values' && !opts?.fromFormula && !opts?.onlyLocal)
      recordCellEdits(api, e.params as MutationParams)
  })
  return () => {
    offSlicer()
    offDataset()
    edit.dispose()
  }
}
