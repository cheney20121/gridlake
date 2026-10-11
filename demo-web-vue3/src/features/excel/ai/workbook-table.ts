import type { FUniver } from '@univerjs/core/facade'
import type { Applied } from './workbook'
import type { AiTableContext } from '@/api/ai'
import type { DatasetFilter, DatasetSort } from '@/api/dataset'
import type { CellRule, ColumnStyle, NumFmt } from '@/components/excel/big-grid/column-style'
import type { CellTableSpec, DataTableSpec } from '@/features/excel/datatable/types'
import { NUM_FMT_LABEL } from '@/components/excel/big-grid/column-style'
import { removeFloat, updateFloat } from '@/features/excel/chart/chart-manager'
import { datasetById } from '@/features/excel/dashboard/slicer'
import { insertCellTable, removeCellTable, updateCellTable } from '@/features/excel/datatable/cell-table'
import { editCount } from '@/features/excel/datatable/edits'
import { checkTableName, findCellTable, findDataTable, insertDataTable, listCellTables, listDataTables, uniqueTableName } from '@/features/excel/datatable/types'

/**
 * AI 对工作簿里数据表的调整：只改数据表定义里的"视图"（筛选、排序、显示列、关键字）与"列样式"（数字格式、条件格式），
 * 由服务端执行查询、表格按可视区域取数、只对屏幕上的格子求值，所以千万行也一样快。撤销信息只记被改的那几项，不影响列宽等其它设置
 */

/** 数据表定义里 AI 能调整的部分 */
export interface TableView {
  filters: DatasetFilter[]
  sort: DatasetSort[]
  fields: string[]
  keyword?: string
}

/** set_table_view 建议的内容（见服务端 tools.rs）：没给的项为 undefined，表示保持不变 */
export interface TableViewPayload {
  table: string
  datasetId: string
  filters?: DatasetFilter[]
  sort?: DatasetSort[]
  fields?: string[]
  keyword?: string
  preview?: { note?: string }
}

export interface TableViewUndo {
  kind: 'table_view'
  tableId: string
  name: string
  before: TableView
  /** 应用后的视图指纹：撤销时对不上说明用户又改过 */
  afterKey: string
}

/** set_column_format 建议的内容：没给的项为 undefined，表示保持不变；rules 给了就整体替换该列的规则 */
export interface ColumnFormatPayload {
  table: string
  datasetId: string
  column: string
  numFmt?: NumFmt
  rules?: CellRule[]
  preview?: { note?: string }
}

export interface ColumnFormatUndo {
  kind: 'column_format'
  tableId: string
  name: string
  column: string
  /** 应用前这一列的样式；null 为没有设置过 */
  before: ColumnStyle | null
  afterKey: string
}

/** 上下文里最多带的数据表数 */
const MAX_CTX_TABLES = 12

export function collectTableContext(api: FUniver | null | undefined): AiTableContext[] {
  if (!api)
    return []
  const make = (spec: DataTableSpec, mode: 'float' | 'cells'): AiTableContext => ({
    name: spec.name,
    datasetName: datasetById(spec.datasetId)?.name ?? '',
    mode,
    fields: spec.fields,
    filters: spec.filters,
    sort: spec.sort,
    keyword: spec.keyword || undefined,
    columnStyles: spec.columnStyles && Object.keys(spec.columnStyles).length ? spec.columnStyles : undefined,
  })
  return [
    ...listDataTables(api).map(t => make(t.spec, 'float')),
    ...listCellTables(api).map(t => make(t.spec, 'cells')),
  ].slice(0, MAX_CTX_TABLES)
}

function viewOf(spec: DataTableSpec): TableView {
  return { filters: spec.filters, sort: spec.sort, fields: spec.fields, keyword: spec.keyword }
}

function viewKey(spec: DataTableSpec) {
  const v = viewOf(spec)
  return JSON.stringify([v.filters, v.sort, v.fields, v.keyword ?? ''])
}

function findById(api: FUniver, id: string) {
  const float = listDataTables(api).find(t => t.spec.id === id)
  if (float)
    return { spec: float.spec as DataTableSpec, cells: false }
  const cell = findCellTable(api, id)
  return cell ? { spec: cell.spec as DataTableSpec, cells: true } : null
}

async function save(api: FUniver, spec: DataTableSpec, cells: boolean) {
  if (cells)
    await updateCellTable(api, spec as CellTableSpec)
  else
    updateFloat(api, spec)
}

/** 应用 set_table_view：给了的项整体替换，没给的保持不变 */
export async function applyTableView(api: FUniver, p: TableViewPayload): Promise<Applied> {
  const found = findDataTable(api, p.table)
  if (!found)
    throw new Error(`工作簿里没有名为「${p.table}」的数据表`)
  if (found.spec.datasetId !== p.datasetId)
    throw new Error(`数据表「${p.table}」用的数据集已经变了，请重新提问`)
  const before = viewOf(found.spec)
  const next: DataTableSpec = {
    ...found.spec,
    filters: p.filters ?? found.spec.filters,
    sort: p.sort ?? found.spec.sort,
    fields: p.fields ?? found.spec.fields,
    keyword: p.keyword === undefined ? found.spec.keyword : (p.keyword || undefined),
  }
  await save(api, next, found.cells)
  const note = p.preview?.note
  return {
    message: `已调整数据表「${p.table}」${note ? `（${note}）` : ''}`,
    undo: { kind: 'table_view', tableId: next.id, name: p.table, before, afterKey: viewKey(next) },
  }
}

/** 撤销 set_table_view；表被删除、或视图在应用后又被改过时保留现状并说明 */
export async function undoTableView(api: FUniver, undo: TableViewUndo): Promise<string> {
  const found = findById(api, undo.tableId)
  if (!found)
    return `数据表「${undo.name}」已不存在`
  if (viewKey(found.spec) !== undo.afterKey)
    return `数据表「${undo.name}」的视图已被修改，保留现有设置`
  await save(api, { ...found.spec, ...undo.before, keyword: undo.before.keyword }, found.cells)
  return `已恢复数据表「${undo.name}」原来的视图`
}

/** 一列样式的指纹：撤销时对不上说明应用后又被改过 */
function styleKey(spec: DataTableSpec, column: string) {
  return JSON.stringify(spec.columnStyles?.[column] ?? null)
}

/**
 * 写入一列的样式。单元格模式的数据表要保留一个空记录：刷新时靠它把数字格式写回默认（见 renderCellTable）；
 * 浮动表格没有样式时直接删掉这一项
 */
function withColumnStyle(spec: DataTableSpec, column: string, style: ColumnStyle | null, cells: boolean): DataTableSpec {
  const styles = { ...spec.columnStyles }
  if (style || cells)
    styles[column] = style ?? {}
  else
    delete styles[column]
  return { ...spec, columnStyles: Object.keys(styles).length ? styles : undefined }
}

/** 应用 set_column_format：数字格式与条件格式。单元格模式的数据表不支持条件格式与万元显示（写进工作表的是普通单元格） */
export async function applyColumnFormat(api: FUniver, p: ColumnFormatPayload): Promise<Applied> {
  const found = findDataTable(api, p.table)
  if (!found)
    throw new Error(`工作簿里没有名为「${p.table}」的数据表`)
  if (found.spec.datasetId !== p.datasetId)
    throw new Error(`数据表「${p.table}」用的数据集已经变了，请重新提问`)
  if (found.cells && p.rules?.length)
    throw new Error(`「${p.table}」是单元格模式的数据表，不支持条件格式；请先把它切换为浮动表格`)
  if (found.cells && p.numFmt === 'wan')
    throw new Error(`「${p.table}」是单元格模式的数据表，不支持万元显示；请换成其它数字格式，或先切换为浮动表格`)
  const prev = found.spec.columnStyles?.[p.column] ?? null
  const style: ColumnStyle = { ...prev }
  if (p.numFmt !== undefined)
    style.numFmt = p.numFmt === 'default' ? undefined : p.numFmt
  if (p.rules !== undefined)
    style.rules = p.rules.length ? p.rules : undefined
  const empty = !style.numFmt && !style.rules?.length
  const next = withColumnStyle(found.spec, p.column, empty ? null : { numFmt: style.numFmt, rules: style.rules }, found.cells)
  await save(api, next, found.cells)
  const bits: string[] = []
  if (p.numFmt !== undefined)
    bits.push(p.numFmt === 'default' ? '恢复默认数字格式' : `数字格式「${NUM_FMT_LABEL[p.numFmt]}」`)
  if (p.rules !== undefined)
    bits.push(p.rules.length ? `${p.rules.length} 条条件格式` : '清除条件格式')
  return {
    message: `已设置数据表「${p.table}」${p.column}列：${bits.join('、')}`,
    undo: { kind: 'column_format', tableId: next.id, name: p.table, column: p.column, before: prev, afterKey: styleKey(next, p.column) },
  }
}

/** 撤销 set_column_format */
export async function undoColumnFormat(api: FUniver, undo: ColumnFormatUndo): Promise<string> {
  const found = findById(api, undo.tableId)
  if (!found)
    return `数据表「${undo.name}」已不存在`
  if (styleKey(found.spec, undo.column) !== undo.afterKey)
    return `数据表「${undo.name}」${undo.column}列的样式已被修改，保留现有设置`
  await save(api, withColumnStyle(found.spec, undo.column, undo.before, found.cells), found.cells)
  return `已恢复数据表「${undo.name}」${undo.column}列原来的样式`
}

/** create_data_table 建议的内容：没给的项用默认（全部列、不筛选、不排序） */
export interface TableCreatePayload {
  datasetId: string
  datasetName: string
  name?: string
  mode: 'float' | 'cells'
  fields?: string[]
  filters?: DatasetFilter[]
  sort?: DatasetSort[]
  keyword?: string
  preview?: { note?: string }
}

export interface TableCreateUndo {
  kind: 'table_create'
  tableId: string
  name: string
  cells: boolean
}

/** 表名：AI 给的名字可用就用，重名自动加序号；没给就用「数据集名+表」 */
function pickTableName(api: FUniver, wanted: string | undefined, datasetName: string) {
  const root = wanted?.trim()
  if (!root)
    return uniqueTableName(api, datasetName)
  if (!checkTableName(api, root))
    return root
  for (let i = 2; i < 100; i++) {
    const n = `${root}${i}`
    if (!checkTableName(api, n))
      return n
  }
  return uniqueTableName(api, datasetName)
}

/** 应用 create_data_table：在当前选中的单元格处插入数据表（浮动表格，或铺到单元格） */
export async function applyTableCreate(api: FUniver, p: TableCreatePayload): Promise<Applied> {
  const name = pickTableName(api, p.name, p.datasetName)
  const init = { datasetId: p.datasetId, datasetName: p.datasetName, name, fields: p.fields, filters: p.filters, sort: p.sort, keyword: p.keyword }
  const cells = p.mode === 'cells'
  const spec = cells ? await insertCellTable(api, init) : insertDataTable(api, init)
  const note = p.preview?.note
  return {
    message: `已在当前工作表插入数据表「${name}」${note ? `（${note}）` : ''}`,
    undo: { kind: 'table_create', tableId: spec.id, name, cells },
  }
}

/** 撤销 create_data_table：删掉这张数据表；单元格模式里有手工调整时保留，免得丢用户改的内容 */
export async function undoTableCreate(api: FUniver, undo: TableCreateUndo): Promise<string> {
  const found = findById(api, undo.tableId)
  if (!found)
    return `数据表「${undo.name}」已不存在`
  if (found.cells) {
    if (editCount((found.spec as CellTableSpec).edits) > 0)
      return `数据表「${undo.name}」里有手工调整，保留现有内容`
    removeCellTable(api, undo.tableId)
  }
  else {
    removeFloat(api, undo.tableId)
  }
  return `已删除数据表「${undo.name}」`
}
