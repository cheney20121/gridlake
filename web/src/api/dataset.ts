import type { RuleTerm } from '@/packs/ecommerce/shop-ledger/summary-template'
import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/**
 * 数据集引擎（大数据表）：数据留在服务端，前端按可视区域取数
 * 与 ledger.ts 相同，使用绝对地址绕开 request.ts 改写 baseURL，复用其信封解析与错误提示
 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger/datasets${path}`

/** 列类型，见 features/excel/dataset/column-kinds.ts */
export type DatasetColumnKind = 'text' | 'id' | 'integer' | 'money' | 'number' | 'datetime' | 'boolean'

export interface DatasetColumn {
  key: string
  title: string
  kind: DatasetColumnKind
  width: number
  /** 计算列（如资金净额、用户计算列） */
  computed?: boolean
  /** 用户计算列的公式 */
  formula?: string
  /** 关联视图继承自左侧数据集的计算列：定义它的数据集名称（只能在那里修改 / 删除） */
  origin?: string
}

export interface DatasetInfo {
  id: string
  name: string
  columns: DatasetColumn[]
  searchable: boolean
  supportsRule: boolean
  /** 内置数据集（如资金流水）由专门的导入流程维护，不能改名 / 删除 */
  builtin: boolean
  primaryKey: string[]
  /** builtin / table / view（关联视图）/ recon（对账结果）/ summary（透视表转成的汇总数据集）/ external（直接查询 Parquet 文件） */
  kind: 'builtin' | 'table' | 'view' | 'recon' | 'summary' | 'external'
  view?: ViewDef
  summary?: SummaryDef
  external?: ExternalInfo
  rowCount: number
  /** 所有者（内置与登录校验启用前创建的没有所有者，只有管理员能改名 / 删除 / 改结构） */
  ownerId?: string | null
  ownerName?: string | null
}

/** 筛选条件；In 的 "" 表示空值 */
export type DatasetFilter
  = | { op: 'in', column: string, values: string[] }
    | { op: 'between', column: string, min?: number | string | null, max?: number | string | null }
    | { op: 'contains', column: string, value: string }
    | { op: 'prefix', column: string, value: string }
    | { op: 'prefixes', column: string, values: string[] }

export interface DatasetSort {
  column: string
  desc: boolean
}

export interface DatasetQuery {
  filters?: DatasetFilter[]
  keyword?: string
  sort?: DatasetSort[]
  /** 汇总项规则：只保留有贡献的行，并提供贡献列 */
  rule?: RuleTerm[]
}

/** 规则贡献列在排序 / 统计中的键 */
export const CONTRIBUTION_KEY = '__contribution'

export interface DatasetCursor {
  cursorId: string
  total: number
  contribution: number | null
  hasContribution: boolean
  cached: boolean
  elapsedMs: number
}

export type CellValue = string | number | boolean | null

export interface DatasetRows {
  /** 游标失效（数据变更或被淘汰）：需重新建游标 */
  expired: boolean
  offset: number
  rows: CellValue[][]
}

export interface SelectionStats {
  expired: boolean
  count: number
  numericCount: number
  sum: number | null
  avg: number | null
  min: number | null
  max: number | null
}

export interface DistinctResult {
  values: { value: string, count: number }[]
  totalDistinct: number
  min: CellValue
  max: CellValue
}

export function getDatasetsApi() {
  return request<DatasetInfo[]>({ url: url(''), method: 'get' })
}

/** silent：调用方在界面里自己显示错误（见 request.ts 的 silent），不再弹全局提示 */
export function openCursorApi(datasetId: string, query: DatasetQuery, signal?: AbortSignal, silent = false) {
  return request<DatasetCursor>({ url: url(`/${datasetId}/cursor`), method: 'post', data: query, signal, timeout: 5 * 60 * 1000, silent })
}

/** columns：只取这些列（按此顺序）；省略为全部列 */
export function getRowsApi(datasetId: string, cursorId: string, offset: number, limit: number, signal?: AbortSignal, columns?: string[]) {
  return request<DatasetRows>({ url: url(`/${datasetId}/rows`), method: 'get', params: { cursor: cursorId, offset, limit, columns: columns?.join(',') || undefined }, signal })
}

export function getStatsApi(datasetId: string, data: { cursorId: string, from: number, to: number, columns: string[] }, signal?: AbortSignal) {
  return request<SelectionStats>({ url: url(`/${datasetId}/stats`), method: 'post', data, signal })
}

export function getDistinctApi(datasetId: string, data: DatasetQuery & { column: string, search?: string }, signal?: AbortSignal) {
  return request<DistinctResult>({ url: url(`/${datasetId}/distinct`), method: 'post', data, signal })
}

/** 单次导出上限（与后端一致） */
export const MAX_EXPORT_ROWS = 1_000_000

/** Excel 单表数据行上限（与后端一致） */
export const MAX_XLSX_ROWS = 1_048_575

/** 导出地址：用浏览器原生下载（流式写盘），百万行也不占页面内存 */
export function exportUrl(datasetId: string, cursorId: string, columns: string[], name: string, format: 'csv' | 'xlsx' = 'csv', desc = '') {
  const params = new URLSearchParams({ cursor: cursorId, columns: columns.join(','), name, format, desc })
  return url(`/${datasetId}/export?${params}`)
}

// ── 通用数据集导入 ──

export interface ImportPreviewColumn {
  source: string
  title: string
  kind: DatasetColumnKind
  fillRate: number
  samples: string[]
}

export interface ImportPreview {
  token: string
  fileName: string
  fileSize: number
  /** 表头所在行（1 起） */
  headerRow: number
  columns: ImportPreviewColumn[]
  sampleRows: string[][]
  estimatedRows: number
  /** 列名能对上的已有数据集（可追加） */
  matches: { datasetId: string, name: string, matched: number, total: number, missing: string[] }[]
  /** 多个文件合并时：各文件的行数与缺少的列（合并结果追加了「来源文件」列） */
  sourceFiles?: { name: string, rows: number, missing: string[] }[]
}

export type ImportMode = 'create' | 'append' | 'replace'

export interface ImportOutcome {
  datasetId: string
  name: string
  rowsRead: number
  inserted: number
  skipped: number
  replaced: number
  invalid: { column: string, count: number }[]
  /** 新建时按样本推断的整数 / 是否列，全量数据里有放不下的值，已改成数值 / 文本列 */
  widened?: string[]
  elapsedMs: number
}

/** 预览导入：一个文件，或多个同结构的文件（按列名对齐合并） */
export function previewDatasetImportApi(files: File | File[], opts?: { signal?: AbortSignal, onProgress?: (loaded: number, total: number) => void }) {
  const data = new FormData()
  for (const f of Array.isArray(files) ? files : [files])
    data.append('file', f)
  return request<ImportPreview>({
    url: url('/import/preview'),
    method: 'post',
    data,
    timeout: 10 * 60 * 1000,
    signal: opts?.signal,
    onUploadProgress: opts?.onProgress ? e => opts.onProgress!(e.loaded, e.total ?? 0) : undefined,
  })
}

/** 从网址导入：服务端下载后预览（http / https） */
export function previewDatasetImportUrlApi(link: string) {
  return request<ImportPreview>({ url: url('/import/preview-url'), method: 'post', data: { url: link }, timeout: 10 * 60 * 1000 })
}

export function commitDatasetImportApi(data: {
  token: string
  mode: ImportMode
  datasetId?: string
  name?: string
  columns?: { source: string, title: string, kind: DatasetColumnKind, include: boolean }[]
  primaryKey?: string[]
  fileName: string
}) {
  return request<ImportOutcome>({ url: url('/import/commit'), method: 'post', data, timeout: 30 * 60 * 1000 })
}

/** 修改列类型的预览 / 结果 */
export interface ColumnKindPreview {
  column: string
  from: DatasetColumnKind
  to: DatasetColumnKind
  /** 非空值个数 */
  values: number
  /** 转不过来、会变成空值的个数与例子 */
  lost: number
  lostSamples: string[]
  /** 认不出、按原文保留的个数（改成日期时间时） */
  kept: number
  /** [转换前, 转换后] */
  samples: [string, CellValue][]
  warnings: string[]
  applied: boolean
}

/** 预览把某一列改成另一种类型（不改数据） */
export function previewColumnKindApi(id: string, column: string, kind: DatasetColumnKind) {
  return request<ColumnKindPreview>({ url: url(`/${id}/columns/kind/preview`), method: 'post', data: { column, kind }, timeout: 5 * 60 * 1000 })
}

/** 修改列类型（就地转换）；有值转不过来时要 acceptLoss；会让计算列、关联视图失效时服务端整体取消并说明原因 */
export function changeColumnKindApi(id: string, column: string, kind: DatasetColumnKind, acceptLoss: boolean) {
  return request<ColumnKindPreview>({ url: url(`/${id}/columns/kind`), method: 'put', data: { column, kind, acceptLoss }, timeout: 30 * 60 * 1000 })
}

export function renameDatasetApi(id: string, name: string) {
  return request<boolean>({ url: url(`/${id}`), method: 'put', data: { name } })
}

/** 删除数据集：移到回收站（数据保留，可恢复）；返回行数 */
export function deleteDatasetApi(id: string, operator: string) {
  return request<number>({ url: url(`/${id}`), method: 'delete', params: { operator } })
}

export interface TrashedDataset {
  id: string
  name: string
  kind: 'table' | 'view' | 'recon' | 'summary' | 'external'
  /** 行数（关联视图、对账为空） */
  rowCount: number | null
  deletedAt: string
  deletedBy: string | null
  /** 到这个时间自动彻底删除（不自动清理时为空） */
  purgeAt: string | null
  /** 现在不能恢复的原因（如关联视图引用的数据集也在回收站） */
  blocked: string | null
}

/** 数据集回收站：days 为保留天数（0 表示不自动清理） */
export function listDatasetTrashApi() {
  return request<{ days: number, items: TrashedDataset[] }>({ url: url('/trash'), method: 'get' })
}

/** 恢复；原名已被占用时自动改名（renamed） */
export function restoreDatasetApi(id: string) {
  return request<{ id: string, name: string, renamed: boolean }>({ url: url(`/${id}/untrash`), method: 'post' })
}

/** 彻底删除（连同回收站里基于它的关联视图）；返回删除的名称 */
export function purgeDatasetApi(id: string) {
  return request<string[]>({ url: url(`/${id}/purge`), method: 'delete' })
}

export function emptyDatasetTrashApi() {
  return request<number>({ url: url('/trash'), method: 'delete' })
}

// ── 数据透视表 ──

/** 日期分组：年 / 季度（2026-Q4）/ 月 / ISO 周（2026-W41）/ 日 */
export type PivotGranularity = 'year' | 'quarter' | 'month' | 'week' | 'day'

export interface PivotField {
  column: string
  granularity?: PivotGranularity | null
  /** 表头别名 */
  label?: string
}

export type PivotAgg = 'sum' | 'count' | 'avg' | 'min' | 'max' | 'first' | 'last' | 'count_distinct' | 'approx_distinct'
  | 'median' | 'approx_median' | 'approx_p90' | 'approx_p95' | 'approx_p99' | 'p10' | 'p25' | 'p75' | 'p90' | 'p95' | 'p99' | 'stddev' | 'var'
  /** 计算值：用公式组合同一张透视表里已有的值（见 PivotValue.formula） */
  | 'calc'

/** 值显示方式：原值 / 占总计 / 占行合计 / 占列合计 / 占上级 / 同比 / 同比增减 / 环比 / 环比增减 */
export type PivotShow = 'raw' | 'pct_total' | 'pct_row' | 'pct_col' | 'pct_parent' | 'yoy' | 'yoy_diff' | 'mom' | 'mom_diff'

export interface PivotValue {
  /** count 可省略（按行计数） */
  column?: string | null
  agg: PivotAgg
  show?: PivotShow
  /** 表头别名；计算值的名称 */
  label?: string
  /** agg 为 calc 时的公式，如 ([金额（求和）]-[成本（求和）])/[金额（求和）]；按汇总后的值逐格计算，总计行也是对的 */
  formula?: string
  /** 计算值的数字格式 */
  format?: 'money' | 'number' | 'percent'
}

export interface PivotRequest {
  rows: PivotField[]
  columns: PivotField[]
  values: PivotValue[]
  filters?: DatasetFilter[]
  keyword?: string
  sort?: { by: 'key' | 'value', valueIndex?: number, desc?: boolean }
  /** 第一个行字段按第一个值排名的前 N 项 */
  top?: number | null
  subtotals?: boolean
  grandTotals?: boolean
  /** 同比 / 环比的时间字段；为空时自动取第一个日期类行 / 列字段 */
  baseField?: string | null
  /** 行 / 列标签别名：字段列名 → 原值（空值为 ""）→ 别名；后端按名称排序时用别名 */
  aliases?: Record<string, Record<string, string>>
  /** 计算项：在某个行 / 列字段的取值之间做运算，新增一个取值（如列字段「单据类型」里新增「期末 = [期初] + [入库] - [出库]」） */
  calcItems?: { field: string, name: string, formula: string }[]
}

export interface PivotValueInfo {
  label: string
  agg: PivotAgg
  column: string | null
  /** percent：占比、增长率（小数，如 0.25 = 25%） */
  kind: 'money' | 'number' | 'percent'
  show: PivotShow
}

export interface PivotRow {
  /** 行键；null 表示该字段为空值 */
  keys: (string | null)[]
  kind: 'data' | 'subtotal' | 'total'
  /** cells[列下标][值下标]；有列字段且显示总计时，最后一列为总计。按显示方式换算后的值 */
  cells: (number | null)[][]
  /** 原始汇总值（使用了占比 / 同比 / 环比时提供，下钻核对用） */
  raw?: (number | null)[][]
}

export interface PivotResult {
  rowFields: string[]
  columnFields: string[]
  values: PivotValueInfo[]
  columnKeys: (string | null)[][]
  columnTotal: boolean
  rows: PivotRow[]
  leafRows: number
  firstLevelCount: number | null
  /** 同比 / 环比所用的时间字段 */
  baseField?: string
  elapsedMs: number
}

export function getPivotApi(datasetId: string, req: PivotRequest, signal?: AbortSignal) {
  return request<PivotResult>({ url: url(`/${datasetId}/pivot`), method: 'post', data: req, signal, timeout: 5 * 60 * 1000 })
}

// ── 关联视图与计算列 ──

export interface ViewDef {
  left: string
  right: string
  on: { left: string, right: string }[]
  columns: { source: string, title: string }[]
}

export interface JoinPreview {
  leftRows: number
  rightRows: number
  matchedRows: number
  unmatchedRows: number
  /** [键, 行数]：关联不上的键（最多 20 个） */
  unmatchedKeys: [string, number][]
  /** 右侧同一键有多行的键数（取最先入库的一行） */
  rightDuplicateKeys: number
}

export function previewJoinApi(def: ViewDef) {
  return request<JoinPreview>({ url: url('/views/preview'), method: 'post', data: def, timeout: 5 * 60 * 1000 })
}

/** 汇总数据集：透视表「转为数据表」，行 / 列字段成为分组列，汇总值与计算值成为数值列，随源数据实时变化 */
export interface SummaryDef {
  source: string
  rows: PivotField[]
  values: PivotValue[]
  filters?: DatasetFilter[]
}

export function createSummaryApi(name: string, def: SummaryDef) {
  return request<string>({ url: url('/summaries'), method: 'post', data: { name, def } })
}

/** 外部数据集：直接查询服务器收件目录（LEDGER_INBOX_ROOTS）里的 Parquet 文件，不导入；只有管理员能建 / 刷新 */
export interface ExternalInput {
  /** 目录：绝对路径，或相对于第一个收件目录 */
  path: string
  /** 文件名通配符，多个用 ; 分隔 */
  pattern: string
  /** 包含子目录 */
  recursive: boolean
  /** 把 key=value 形式的目录名读成列（Hive 分区） */
  hive: boolean
}

export interface ExternalInfo extends ExternalInput {
  fileCount: number
  rows: number
  size: number
  refreshedAt: string
}

export interface ExternalPreview {
  fileCount: number
  /** 前几个文件（相对于目录） */
  files: string[]
  rows: number
  size: number
  columns: { title: string, source: string, kind: DatasetColumnKind, sqlType: string }[]
  sampleRows: CellValue[][]
}

export interface ExternalRefreshOutcome {
  fileCount: number
  rows: number
  addedFiles: number
  removedFiles: number
  addedColumns: string[]
  removedColumns: string[]
}

export function previewExternalApi(input: ExternalInput) {
  return request<ExternalPreview>({ url: url('/external/preview'), method: 'post', data: input, timeout: 5 * 60 * 1000 })
}

export function createExternalApi(name: string, input: ExternalInput) {
  return request<string>({ url: url('/external'), method: 'post', data: { name, ...input }, timeout: 5 * 60 * 1000 })
}

/** 文件有增减或结构变化后重新读取；会让计算列、关联视图失效时服务端整体取消并说明原因 */
export function refreshExternalApi(id: string) {
  return request<ExternalRefreshOutcome>({ url: url(`/${id}/external/refresh`), method: 'post', timeout: 5 * 60 * 1000 })
}

export function createViewApi(name: string, def: ViewDef) {
  return request<string>({ url: url('/views'), method: 'post', data: { name, ...def } })
}

/** 修改关联视图：会让下游视图或计算列失效时服务端整体取消并说明原因 */
export function updateViewApi(id: string, name: string, def: ViewDef) {
  return request<boolean>({ url: url(`/views/${id}`), method: 'put', data: { name, ...def } })
}

export interface CalcPreview {
  kind: DatasetColumnKind
  samples: Record<string, CellValue>[]
  nullRows: number
  totalRows: number
}

export function previewCalcApi(datasetId: string, formula: string, signal?: AbortSignal) {
  return request<CalcPreview>({ url: url(`/${datasetId}/calc/preview`), method: 'post', data: { formula }, signal, timeout: 5 * 60 * 1000 })
}

export function addCalcApi(datasetId: string, name: string, formula: string, kind?: DatasetColumnKind | null) {
  return request<boolean>({ url: url(`/${datasetId}/calc`), method: 'post', data: { name, formula, kind: kind || null } })
}

/** 修改计算列公式：引用它的计算列、关联视图随之更新 */
export function updateCalcApi(datasetId: string, name: string, formula: string, kind?: DatasetColumnKind | null) {
  return request<boolean>({ url: url(`/${datasetId}/calc/${encodeURIComponent(name)}`), method: 'put', data: { formula, kind: kind || null } })
}

export function removeCalcApi(datasetId: string, name: string) {
  return request<boolean>({ url: url(`/${datasetId}/calc/${encodeURIComponent(name)}`), method: 'delete' })
}

// ── 填报回写 ──

export type WritebackMode = 'upsert' | 'append'

/** 提交方式：delete 按主键删除行（填报区里只看主键列） */
export type WritebackSubmitMode = WritebackMode | 'delete'

export interface WritebackRequest {
  mode: WritebackSubmitMode
  /** 每行各值对应的数据集列名 */
  columns: string[]
  rows: CellValue[][]
  /** 数据集未登记主键时按这些列更新 */
  keys?: string[]
  operator?: string
  /** 来源（工作簿、页签、填报区），记入提交记录 */
  source?: { workbookId?: string | null, workbookName?: string | null, sheetName?: string, inputName?: string }
}

export interface WritebackCellError {
  /** 请求 rows 中的下标 */
  row: number
  column: string
  message: string
}

export interface WritebackOutcome {
  /** 写入成功时的提交记录 id；有错误时为空（整批未写入） */
  batchId: string | null
  datasetName: string
  inserted: number
  updated: number
  unchanged: number
  /** 删除的行（mode = delete） */
  deleted: number
  /** 要删除的主键在数据集里找不到的个数 */
  notFound: number
  errors: WritebackCellError[]
  errorCount: number
  elapsedMs: number
}

export interface WritebackRecord {
  id: string
  datasetId: string
  datasetName: string | null
  mode: WritebackSubmitMode
  operator: string
  createdAt: string
  inserted: number
  updated: number
  unchanged: number
  deleted: number
  source: WritebackRequest['source'] | null
  undoneAt: string | null
  undoneBy: string | null
  canUndo: boolean
  undoBlocked: string | null
}

const ledgerUrl = (path: string) => `${LEDGER_BASE}/api/ledger${path}`

/** 提交填报数据（整批校验，有错误时不写入） */
export function writebackApi(datasetId: string, data: WritebackRequest) {
  return request<WritebackOutcome>({ url: url(`/${datasetId}/writeback`), method: 'post', data, timeout: 5 * 60 * 1000 })
}

export function listWritebacksApi(datasetId?: string, limit = 50) {
  return request<WritebackRecord[]>({ url: ledgerUrl('/writebacks'), method: 'get', params: { datasetId, limit } })
}

export function undoWritebackApi(id: string, operator: string) {
  return request<{ datasetId: string, datasetName: string, removed: number, restored: number, reinserted: number }>({ url: ledgerUrl(`/writebacks/${id}/undo`), method: 'post', params: { operator } })
}

/** 新建空的填报数据集，返回 id */
export function createInputDatasetApi(data: { name: string, columns: { title: string, kind: DatasetColumnKind }[], primaryKey: string[] }) {
  return request<string>({ url: url('/input'), method: 'post', data })
}

// ── 使用情况 ──

export interface DatasetWorkbookRef {
  id: string
  name: string
  updatedAt: string
  updatedBy: string | null
  /** 部件类型 → 个数（pivot / webexcel-chart / webexcel-slicer / webexcel-kpi / webexcel-table / webexcel-datatable / input / formula） */
  parts: Record<string, number>
  columns: string[]
}

export interface DatasetNamedRef {
  id: string
  name: string
  /** 视图：左表 / 右表；SQL：读取 / 输出；连接：同步方式；推送：所在工作簿 */
  role: string
  enabled?: boolean
}

export interface DatasetUsage {
  datasetId: string
  name: string
  workbooks: DatasetWorkbookRef[]
  templates: DatasetNamedRef[]
  views: DatasetNamedRef[]
  sqlQueries: DatasetNamedRef[]
  connectors: DatasetNamedRef[]
  reports: DatasetNamedRef[]
  /** 填报提交次数（未撤销） */
  writebacks: number
  access: {
    /** 最近 30 天（含今天） */
    days: { day: string, hits: number }[]
    /** 来源（browse / pivot / formula / export / ai / mcp / report / sql）→ 30 天合计 */
    byKind: Record<string, number>
    total: number
    lastDay: string | null
  }
}

export function getDatasetUsageApi(id: string) {
  return request<DatasetUsage>({ url: url(`/${id}/usage`), method: 'get', timeout: 60 * 1000 })
}

// ── 异常发现 ──

export interface AnomalyFinding {
  id: string
  /** volume 行数 / metric 金额数值日合计 / segment 分组变化 */
  kind: 'volume' | 'metric' | 'segment'
  severity: 'high' | 'medium' | 'low'
  title: string
  detail: string
  datasetId: string
  datasetName: string
  actual: number
  expected: number
  /** 相对预期的变化（-0.4 表示低了 40%） */
  change: number
  drill: { title: string, filters: DatasetFilter[] }
}

export interface AnomalyScan {
  datasetId: string
  datasetName: string
  timeColumn: string | null
  latestDate: string | null
  findings: AnomalyFinding[]
  /** 没能扫描 / 数据不足的原因 */
  note: string | null
}

/** 扫描数据集：最近 7 天相对此前 28 天明显偏离的走势与分组 */
export function getAnomaliesApi(datasetId: string) {
  return request<AnomalyScan>({ url: url(`/${datasetId}/anomalies`), method: 'get', timeout: 120 * 1000 })
}
