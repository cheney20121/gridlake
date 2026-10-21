import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import type { TableEdits } from './edits'
import type { DatasetFilter, DatasetInfo, DatasetQuery, DatasetSort } from '@/api/dataset'
import type { ColumnStyle } from '@/components/excel/big-grid/column-style'
import type { FloatLocation } from '@/features/excel/chart/chart-manager'
import { insertFloat, listFloats, sheetHeaderSize } from '@/features/excel/chart/chart-manager'
import { datasetById, slicerFiltersFor } from '@/features/excel/dashboard/slicer'
import { t as tr } from '@/i18n'

/**
 * 数据表：把数据集（或关联视图）的明细放进工作表，占位固定、内部滚动，千万行也只取可视区域
 *
 * 与图表一样是浮动对象，定义随工作簿快照保存。公式按表名引用它（下推到服务端，按数据表当前的筛选计算）：
 *   =TSUM("订单表[订单金额]")  =TCOUNT("订单表")  =TVALUE("订单表[订单号]", 1)
 */

export const DATA_TABLE_KEY = 'webexcel-datatable'

export interface DataTableSpec {
  v: 1
  id: string
  kind: 'datatable'
  /** 表名：公式按它引用，工作簿内唯一 */
  name: string
  datasetId: string
  /** 显示的列（列名，按此顺序）；空为全部列 */
  fields: string[]
  /** 列宽（列名 → px） */
  widths?: Record<string, number>
  /** 列样式：数字格式与条件格式（列名 → 样式）；只改显示，不改数据。单元格模式只支持数字格式 */
  columnStyles?: Record<string, ColumnStyle>
  filters: DatasetFilter[]
  sort: DatasetSort[]
  keyword?: string
  /** 叠加同一工作表上切片器的筛选（默认是） */
  followSlicers?: boolean
}

export const DEFAULT_TABLE_SIZE = { width: 720, height: 360 }

/**
 * 单元格模式的数据表（对标 Row Zero 的「标准模式」）：结果直接写成普通单元格（最多 MAX_CELL_ROWS 行），
 * 随数据集、切片器刷新；可用 A:A、区域引用，也能和浮动模式一样按表名用 TSUM 等函数。
 * 定义存在工作表元数据 `celltables` 里（随工作簿保存），与浮动模式共用同一套字段，可互相切换
 */
export interface CellTableSpec extends DataTableSpec {
  /** 表头左上角 */
  row: number
  col: number
  /** 上次写入的区域（含表头）：刷新时清掉多出来的部分 */
  extent?: { rows: number, cols: number }
  /** 上次刷新时符合条件的总行数（超过上限时只写前 MAX_CELL_ROWS 行） */
  total?: number
  /** 保留手工调整：在表里改过的格子，刷新后仍然保留（按数据集的主键认行，见 edits.ts） */
  keepEdits?: boolean
  /** 记下的手工调整 */
  edits?: TableEdits
}

export const MAX_CELL_ROWS = 10_000
const CELL_META_KEY = 'celltables'

export function getCellTables(sheet: FWorksheet | null | undefined): CellTableSpec[] {
  const list = (sheet?.getCustomMetadata() as Record<string, unknown> | undefined)?.[CELL_META_KEY]
  return Array.isArray(list) ? (list as CellTableSpec[]) : []
}

export function saveCellTables(sheet: FWorksheet, list: CellTableSpec[]) {
  sheet.setCustomMetadata({ ...(sheet.getCustomMetadata() ?? {}), [CELL_META_KEY]: list })
}

export interface CellTableLocation {
  sheet: FWorksheet
  spec: CellTableSpec
}

export function listCellTables(api: FUniver): CellTableLocation[] {
  return (api.getActiveWorkbook()?.getSheets() ?? []).flatMap(sheet => getCellTables(sheet).map(spec => ({ sheet, spec })))
}

export function findCellTable(api: FUniver, id: string) {
  return listCellTables(api).find(t => t.spec.id === id) ?? null
}

/** 单元格模式数据表占用的区域（含表头；还没写过时只算表头一行） */
export function cellTableArea(spec: CellTableSpec) {
  return { row: spec.row, col: spec.col, rows: spec.extent?.rows ?? 1, cols: spec.extent?.cols ?? Math.max(spec.fields.length, 1) }
}

/** 格子落在哪个单元格模式数据表里 */
export function cellTableAt(sheet: FWorksheet, row: number, col: number) {
  return getCellTables(sheet).find((t) => {
    const a = cellTableArea(t)
    return row >= a.row && row < a.row + a.rows && col >= a.col && col < a.col + a.cols
  }) ?? null
}

export function newDataTableId() {
  return `dt_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`
}

export function listDataTables(api: FUniver): FloatLocation<DataTableSpec>[] {
  return listFloats<DataTableSpec>(api, DATA_TABLE_KEY)
}

/** 按表名找数据表（浮动、单元格两种模式；不区分大小写，与 Excel 表名一致） */
export function findDataTable(api: FUniver, name: string): { spec: DataTableSpec, sheet: FWorksheet, cells: boolean } | null {
  const n = name.trim().toLowerCase()
  const match = (spec: DataTableSpec) => spec.name.trim().toLowerCase() === n
  const float = listDataTables(api).find(t => match(t.spec))
  if (float)
    return { spec: float.spec, sheet: float.sheet, cells: false }
  const cell = listCellTables(api).find(t => match(t.spec))
  return cell ? { ...cell, cells: true } : null
}

/** 工作簿里有没有数据表（任一模式） */
export function hasDataTables(api: FUniver) {
  return listDataTables(api).length > 0 || listCellTables(api).length > 0
}

/** 表名规则：不能为空、不能含 [ ] " 等公式里有特殊含义的字符 */
export function checkTableName(api: FUniver, name: string, selfId?: string): string | null {
  const n = name.trim()
  if (!n)
    return tr('表名不能为空')
  if (/[[\]"'!]/.test(n))
    return tr('表名不能包含 [ ] " \' ! 等字符')
  const dup = findDataTable(api, n)
  if (dup && dup.spec.id !== selfId)
    return tr('工作簿里已有名为「{0}」的数据表', [n])
  return null
}

/** 不重名的默认表名：数据集名 + 表，重名时加序号 */
export function uniqueTableName(api: FUniver, base: string) {
  const root = base.replace(/[[\]"'!]/g, '') || '数据表'
  let name = `${root}表`
  for (let i = 2; findDataTable(api, name); i++)
    name = `${root}表${i}`
  return name
}

/** 实际显示的列（定义里的列已不存在时跳过） */
export function tableColumns(spec: DataTableSpec, ds: DatasetInfo | undefined) {
  if (!ds)
    return []
  if (!spec.fields.length)
    return ds.columns
  return spec.fields
    .map(f => ds.columns.find(c => c.title === f || c.key === f))
    .filter((c): c is DatasetInfo['columns'][number] => !!c)
}

/** 数据表当前的查询条件：自身筛选 + 切片器（同一工作表、数据集有该列）+ 关键字 + 排序 */
export function tableQuery(api: FUniver, spec: DataTableSpec, sheetId: string | null): DatasetQuery {
  const slicers = spec.followSlicers === false ? [] : slicerFiltersFor(api, sheetId, spec.datasetId)
  return {
    filters: [...spec.filters, ...slicers],
    keyword: spec.keyword?.trim() || undefined,
    sort: spec.sort,
  }
}

/** 数据表所在工作表 */
export function tableSheetId(api: FUniver, id: string) {
  return listDataTables(api).find(t => t.spec.id === id)?.sheet.getSheetId() ?? null
}

/** 单元格左上角在浮动层中的像素位置（原点为画布左上角，含行号列、列标行） */
export function cellPixel(sheet: FWorksheet, row: number, col: number) {
  const s = sheet as unknown as { getColumnWidth: (c: number) => number, getRowHeight: (r: number) => number }
  const header = sheetHeaderSize(sheet)
  let x = header.width
  for (let c = 0; c < col; c++)
    x += s.getColumnWidth(c)
  let y = header.height
  for (let r = 0; r < row; r++)
    y += s.getRowHeight(r)
  return { x, y }
}

/** 数据集是否有搜索（关键字）能力 */
export function searchable(datasetId: string) {
  return !!datasetById(datasetId)?.searchable
}

/** 在当前工作表的选中单元格处插入数据表 */
export function insertDataTable(api: FUniver, init: { datasetId: string, datasetName: string, name?: string, fields?: string[], filters?: DatasetFilter[], sort?: DatasetSort[], keyword?: string }) {
  const sheet = api.getActiveWorkbook()?.getActiveSheet()
  if (!sheet)
    throw new Error(tr('没有打开的工作表'))
  const cell = sheet.getSelection()?.getActiveRange()
  const at = cellPixel(sheet, cell?.getRow() ?? 0, cell?.getColumn() ?? 0)
  const spec: DataTableSpec = {
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
  }
  insertFloat(api, DATA_TABLE_KEY, spec, DEFAULT_TABLE_SIZE, sheet, { x: at.x + 1, y: at.y + 1 })
  return spec
}
