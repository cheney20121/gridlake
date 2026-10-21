import type { ICellData, IStyleData } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import type { PivotStyles, StyleDelta, StyleGrid } from './pivot-styles'
import type { DatasetFilter, PivotAgg, PivotField, PivotRequest, PivotResult, PivotRow } from '@/api/dataset'
import type { DatasetDrill } from '@/components/excel/big-grid/drill'
import { getPivotApi } from '@/api/dataset'
import { t as tr } from '@/i18n'
import { extractStyles, isEmptyStyles, styleDelta, stylesAt } from './pivot-styles'

/**
 * 数据透视表页签：定义保存在工作表元数据里（随工作簿保存到服务端），结果由服务端计算后写成普通单元格。
 *
 * 两种版式：
 * - 表格（默认，与 Excel「表格形式」一致）：每个行字段一列，组内只在首行显示上级标签（可选每行重复），小计行在组末尾
 * - 紧凑：一列标签按层级缩进，分组行在组首（带小计），最左侧一列 ▾ / ▸ 点击折叠展开
 *
 * 行 / 列标签、表头可直接在格子里改名（别名），清空即恢复原名；在页签里改的格式重算后保留（见 pivot-styles）
 */

export interface PivotDisplay {
  layout?: 'tabular' | 'compact'
  /** 表格版式下每行都显示上级标签（便于复制到别处筛选） */
  repeatLabels?: boolean
  /** 紧凑版式下折叠的分组：行键前缀的 JSON */
  collapsed?: string[]
}

export interface PivotSpec {
  version: 1
  datasetId: string
  datasetName: string
  request: PivotRequest
  display?: PivotDisplay
  /** 用户在页签里改的格式（按含义记录，重算后重新套用） */
  styles?: PivotStyles
}

const META_KEY = 'pivot'
const BLANK = '（空白）'
/** 紧凑版式每级缩进 */
const INDENT = '\u3000\u3000'

/** 可改名 / 可点击的格子 */
type CellTarget
  = | { kind: 'key', field: number, key: string | null }
    | { kind: 'colKey', key: string | null }
    | { kind: 'value', index: number }
    | { kind: 'rowField', index: number }
    | { kind: 'toggle', prefix: string }

/** 每个透视表页签最近一次的版式（下钻时把单元格映射回筛选条件） */
interface Layout {
  spec: PivotSpec
  result: PivotResult
  dataStartRow: number
  labelCols: number
  /** 数据列：sheet 列号 - labelCols → 列键下标（null = 总计列）与值下标 */
  colMap: { colKeyIndex: number | null, valueIndex: number }[]
  /** 显示的各行对应的透视结果行（紧凑版式没有小计时，分组行为 null） */
  rows: (PivotRow | null)[]
  /** "行,列" → 格子用途 */
  targets: Map<string, CellTarget>
  /** 格子含义与重画后读回的样式（基准）：与基准不同的部分是用户改的格式 */
  grid: StyleGrid
  base: unknown[][]
}
const layouts = new Map<string, Layout>()

const FONT = { ff: 'Microsoft YaHei', fs: 10 }
const BORDER = { s: 1, cl: { rgb: '#D9D9D9' } }
const CELL_BD = { b: BORDER, r: BORDER }
const STYLE = {
  title: { ...FONT, fs: 9, cl: { rgb: '#6B7280' } },
  header: { ...FONT, bl: 1, bg: { rgb: '#DDEBF7' }, ht: 2, vt: 2, tb: 3, bd: CELL_BD },
  label: { ...FONT, bd: CELL_BD },
  subLabel: { ...FONT, bl: 1, bg: { rgb: '#F2F7FC' }, bd: CELL_BD },
  totalLabel: { ...FONT, bl: 1, bg: { rgb: '#DDEBF7' }, bd: { ...CELL_BD, t: { s: 1, cl: { rgb: '#4472C4' } } } },
  toggle: { ...FONT, cl: { rgb: '#217346' }, ht: 2, vt: 2 },
} satisfies Record<string, IStyleData>

const NUM_FMT = { money: '#,##0.00;-#,##0.00;"-"', number: '#,##0;-#,##0;"-"', decimal: '#,##0.00;-#,##0.00;"-"', percent: '0.0%;-0.0%;"-"' }
/** 结果可能带小数的汇总方式 */
const DECIMAL_AGGS = new Set<PivotAgg>(['calc', 'avg', 'first', 'last', 'median', 'approx_median', 'approx_p90', 'approx_p95', 'approx_p99', 'p10', 'p25', 'p75', 'p90', 'p95', 'p99', 'stddev', 'var'])

type RowKind = PivotRow['kind']

function valueStyle(kind: RowKind, fmt: string): IStyleData {
  const base = kind === 'total' ? STYLE.totalLabel : kind === 'subtotal' ? STYLE.subLabel : STYLE.label
  return { ...base, ht: 3, n: { pattern: fmt } }
}

export function getPivotSpec(sheet: FWorksheet | null | undefined): PivotSpec | null {
  const custom = sheet?.getCustomMetadata() as Record<string, unknown> | undefined
  const spec = custom?.[META_KEY] as PivotSpec | undefined
  return spec?.version === 1 ? spec : null
}

export function isPivotSheet(sheet: FWorksheet | null | undefined) {
  return !!getPivotSpec(sheet)
}

function keyText(v: string | null) {
  return v == null || v === '' ? BLANK : v
}

/** 标签显示文字：有别名用别名 */
function labelOf(req: PivotRequest, field: PivotField | undefined, key: string | null) {
  return (field && req.aliases?.[field.column]?.[key ?? '']) || keyText(key)
}

const GRANULARITY_TEXT: Record<string, string> = { year: tr('（年）'), quarter: tr('（季度）'), month: tr('（月）'), week: tr('（周）'), day: tr('（日）') }

function fieldTitle(f: PivotField) {
  return f.label?.trim() || f.column + (GRANULARITY_TEXT[f.granularity ?? ''] ?? '')
}

function isCompact(spec: PivotSpec) {
  return spec.display?.layout === 'compact' && spec.request.rows.length >= 2
}

interface DisplayRow {
  row: PivotRow | null
  /** 行键（紧凑版式的分组行没有对应的结果行时为分组前缀） */
  keys: (string | null)[]
  kind: RowKind
  /** 标签列号 → 文字与用途 */
  labels: Map<number, { text: string, target?: CellTarget }>
}

/** 表格版式：每个行字段一列 */
function tabularRows(spec: PivotSpec, rows: PivotRow[]): DisplayRow[] {
  const req = spec.request
  const repeat = !!spec.display?.repeatLabels
  const out: DisplayRow[] = []
  let prev: (string | null)[] = []
  for (const row of rows) {
    const labels: DisplayRow['labels'] = new Map()
    if (row.kind === 'total') {
      labels.set(0, { text: tr('总计') })
      prev = []
    }
    else if (row.kind === 'subtotal') {
      const level = row.keys.length - 1
      if (repeat)
        row.keys.slice(0, level).forEach((key, c) => labels.set(c, { text: labelOf(req, req.rows[c], key), target: { kind: 'key', field: c, key } }))
      labels.set(level, { text: tr('{0} 汇总', [labelOf(req, req.rows[level], row.keys[level]!)]), target: { kind: 'key', field: level, key: row.keys[level]! } })
      prev = []
    }
    else {
      // 组内只在首行显示上级标签
      row.keys.forEach((key, c) => {
        const sameParent = prev.length > c && row.keys.slice(0, c + 1).every((x, i) => x === prev[i])
        if (repeat || !sameParent || c === row.keys.length - 1)
          labels.set(c, { text: labelOf(req, req.rows[c], key), target: { kind: 'key', field: c, key } })
      })
      prev = row.keys
    }
    out.push({ row, keys: row.keys, kind: row.kind, labels })
  }
  return out
}

/** 紧凑版式：第 0 列折叠按钮，第 1 列标签按层级缩进；分组行在组首，显示该组小计 */
function compactRows(spec: PivotSpec, rows: PivotRow[]): DisplayRow[] {
  const req = spec.request
  const k = req.rows.length
  const collapsed = new Set(spec.display?.collapsed ?? [])
  const subtotals = new Map(rows.filter(r => r.kind === 'subtotal').map(r => [JSON.stringify(r.keys), r]))
  /** keys 的前 level 级里是否有折叠的分组 */
  const hidden = (keys: (string | null)[], level: number) => keys.slice(0, level).some((_, l) => collapsed.has(JSON.stringify(keys.slice(0, l + 1))))
  const out: DisplayRow[] = []
  const open: string[] = []
  for (const row of rows) {
    if (row.kind === 'total') {
      out.push({ row, keys: [], kind: 'total', labels: new Map([[1, { text: tr('总计') }]]) })
      continue
    }
    if (row.kind === 'subtotal')
      continue
    for (let l = 0; l < k - 1; l++) {
      const prefix = row.keys.slice(0, l + 1)
      const pj = JSON.stringify(prefix)
      if (open[l] === pj)
        continue
      open[l] = pj
      open.length = l + 1
      if (hidden(row.keys, l))
        continue
      const key = row.keys[l]!
      out.push({
        row: subtotals.get(pj) ?? null,
        keys: prefix,
        kind: 'subtotal',
        labels: new Map([
          [0, { text: collapsed.has(pj) ? '▸' : '▾', target: { kind: 'toggle', prefix: pj } }],
          [1, { text: INDENT.repeat(l) + labelOf(req, req.rows[l], key), target: { kind: 'key', field: l, key } }],
        ]),
      })
    }
    if (hidden(row.keys, k - 1))
      continue
    const key = row.keys[k - 1]!
    out.push({ row, keys: row.keys, kind: 'data', labels: new Map([[1, { text: INDENT.repeat(k - 1) + labelOf(req, req.rows[k - 1], key), target: { kind: 'key', field: k - 1, key } }]]) })
  }
  return out
}

/** 正在重画（忽略重画自己产生的样式变化） */
let rendering = false
/** 超过这么多格子不记录 / 套用手工格式（读回样式要逐格比较） */
const MAX_STYLE_CELLS = 400_000

/** 值字段的标识（改表头不影响；计算值按名称） */
function valueId(v: PivotRequest['values'][number]) {
  return `${v.agg}:${v.column ?? ''}:${v.show ?? 'raw'}${v.agg === 'calc' ? `:${v.label ?? ''}` : ''}`
}

/** 读回一块区域每个格子的样式（样式 ID 或样式对象） */
function readStyles(sheet: FWorksheet, rows: number, cols: number): unknown[][] {
  if (!rows || !cols)
    return []
  return sheet.getRange(0, 0, rows, cols).getCellDatas().map(r => r.map(c => c?.s ?? null))
}

/** 把页签上与基准不同的样式记下来（与上次的记录合并）；没有版式或太大时返回 null */
function captureStyles(sheet: FWorksheet): PivotStyles | null {
  const layout = layouts.get(sheet.getSheetId())
  if (!layout || !layout.base.length)
    return null
  const rows = layout.grid.rowIds.length
  const cols = layout.grid.colIds.length
  const styles = sheet.getWorkbook().getStyles()
  const resolve = (s: unknown) => (s == null ? null : (styles.get(s as string) ?? null)) as Record<string, unknown> | null
  const cur = readStyles(sheet, rows, cols)
  const deltas: (StyleDelta | null)[][] = cur.map((r, ri) => r.map((s, ci) => {
    const b = layout.base[ri]?.[ci] ?? null
    return s === b ? null : styleDelta(resolve(b), resolve(s))
  }))
  return extractStyles(layout.grid, deltas, layout.spec.styles)
}

/** 用户改了透视表页签的格式：立即记进定义（随工作簿保存） */
function syncStyles(sheet: FWorksheet) {
  const layout = layouts.get(sheet.getSheetId())
  const styles = captureStyles(sheet)
  if (!layout || !styles || JSON.stringify(styles) === JSON.stringify(layout.spec.styles ?? {}))
    return
  const spec = { ...layout.spec, styles: isEmptyStyles(styles) ? undefined : styles }
  layout.spec = spec
  const meta = getPivotSpec(sheet)
  if (meta)
    saveSpec(sheet, { ...meta, styles: spec.styles })
}

/** 把计算结果写进页签（先清掉旧内容与合并）；用户改过的格式按含义重新套上 */
function render(sheet: FWorksheet, specIn: PivotSpec, result: PivotResult) {
  rendering = true
  try {
    renderInner(sheet, specIn, result)
  }
  finally {
    rendering = false
  }
}

function renderInner(sheet: FWorksheet, specIn: PivotSpec, result: PivotResult) {
  // 先把上一版页签上的手工格式收进来（面板传来的定义里没有 styles，以页签上的为准）
  const captured = captureStyles(sheet)
  const styles = captured ?? specIn.styles ?? getPivotSpec(sheet)?.styles
  const spec: PivotSpec = { ...specIn, styles: isEmptyStyles(styles) ? undefined : styles }
  const { columnKeys, columnTotal, values } = result
  const req = spec.request
  const k = req.rows.length
  const m = req.columns.length
  const compact = isCompact(spec)
  const labelCols = compact ? 2 : Math.max(1, k)
  const groups: (number | null)[] = [...columnKeys.map((_, i) => i), ...(m === 0 || columnTotal ? [null] : [])]
  const colMap = groups.flatMap(g => values.map((_, vi) => ({ colKeyIndex: g, valueIndex: vi })))
  const headerRows = m > 0 ? 2 : 1
  const titleRows = 1
  const dataStartRow = titleRows + headerRows
  const shown = compact ? compactRows(spec, result.rows) : tabularRows(spec, result.rows)
  const totalRows = dataStartRow + shown.length
  const totalCols = labelCols + colMap.length
  const targets = new Map<string, CellTarget>()

  // 清理旧内容（保留页签本身，其他页签引用本页的公式不会失效）
  sheet.getMergedRanges().forEach(r => r.breakApart())
  const lastRow = sheet.getLastRow()
  const lastCol = sheet.getLastColumn()
  if (lastRow >= 0 && lastCol >= 0)
    sheet.getRange(0, 0, lastRow + 1, lastCol + 1).clear()
  if (sheet.getMaxRows() < totalRows + 5)
    sheet.insertRowsAfter(sheet.getMaxRows() - 1, totalRows + 5 - sheet.getMaxRows())
  if (sheet.getMaxColumns() < totalCols + 2)
    sheet.insertColumnsAfter(sheet.getMaxColumns() - 1, totalCols + 2 - sheet.getMaxColumns())

  const matrix: ICellData[][] = Array.from({ length: totalRows }, () => Array.from({ length: totalCols }, () => ({})))
  const filterDesc = filterDescOf(req)
  matrix[0]![0] = { v: `${tr('数据透视表')} · ${spec.datasetName}${filterDesc ? ` · ${filterDesc}` : ''}`, s: STYLE.title }

  // 表头
  const fieldRow = dataStartRow - 1
  for (let c = 0; c < labelCols; c++)
    matrix[fieldRow]![c] = { v: '', s: STYLE.header }
  if (compact) {
    matrix[fieldRow]![1] = { v: req.rows.map(fieldTitle).join(' / '), s: STYLE.header }
  }
  else if (k) {
    req.rows.forEach((f, c) => {
      matrix[fieldRow]![c] = { v: fieldTitle(f), s: STYLE.header }
      targets.set(`${fieldRow},${c}`, { kind: 'rowField', index: c })
    })
  }
  colMap.forEach((cm, i) => {
    const c = labelCols + i
    matrix[fieldRow]![c] = { v: values[cm.valueIndex]!.label, s: STYLE.header }
    targets.set(`${fieldRow},${c}`, { kind: 'value', index: cm.valueIndex })
    if (m > 0) {
      const ck = cm.colKeyIndex == null ? null : columnKeys[cm.colKeyIndex]!
      const text = ck ? ck.map((key, j) => labelOf(req, req.columns[j], key)).join(' / ') : tr('总计')
      matrix[fieldRow - 1]![c] = { v: cm.valueIndex === 0 ? text : '', s: STYLE.header }
      // 只有一个列字段时列标签可改名（多个时一格里是组合，无法对应到单个值）
      if (ck && m === 1 && cm.valueIndex === 0)
        targets.set(`${fieldRow - 1},${c}`, { kind: 'colKey', key: ck[0]! })
    }
  })
  if (m > 0) {
    for (let c = 0; c < labelCols; c++)
      matrix[fieldRow - 1]![c] = { v: c === (compact ? 1 : 0) ? req.columns.map(fieldTitle).join(' / ') : '', s: STYLE.header }
  }

  // 数据行
  shown.forEach((d, ri) => {
    const r = dataStartRow + ri
    const labelStyle = d.kind === 'total' ? STYLE.totalLabel : d.kind === 'subtotal' ? STYLE.subLabel : STYLE.label
    for (let c = 0; c < labelCols; c++)
      matrix[r]![c] = { s: labelStyle }
    d.labels.forEach(({ text, target }, c) => {
      const toggle = target?.kind === 'toggle'
      matrix[r]![c] = { v: text, s: toggle ? { ...labelStyle, ...STYLE.toggle } : labelStyle }
      if (target)
        targets.set(`${r},${c}`, target)
    })
    colMap.forEach((cm, i) => {
      const info = values[cm.valueIndex]!
      const cells = d.row?.cells
      const v = cells?.[cm.colKeyIndex == null ? cells.length - 1 : cm.colKeyIndex]?.[cm.valueIndex]
      const fmt = info.kind === 'percent' ? NUM_FMT.percent : info.kind === 'money' ? NUM_FMT.money : DECIMAL_AGGS.has(info.agg) ? NUM_FMT.decimal : NUM_FMT.number
      matrix[r]![labelCols + i] = { ...(v == null ? {} : { v }), s: valueStyle(d.kind, fmt) }
    })
  })

  // 格子含义：手工格式按它记录与套用
  const fieldRowId = 'H:fields'
  const grid: StyleGrid = {
    rowIds: Array.from({ length: totalRows }, (_, r) => r === 0 ? 'T' : r === fieldRow ? fieldRowId : r < dataStartRow ? 'H:cols' : `${shown[r - dataStartRow]!.kind}:${JSON.stringify(shown[r - dataStartRow]!.keys)}`),
    dataRow: Array.from({ length: totalRows }, (_, r) => r >= dataStartRow && shown[r - dataStartRow]!.kind === 'data'),
    colIds: Array.from({ length: totalCols }, (_, c) => {
      if (c < labelCols)
        return `L:${c}`
      const cm = colMap[c - labelCols]!
      return `${valueId(req.values[cm.valueIndex]!)}|${cm.colKeyIndex == null ? 'Σ' : JSON.stringify(columnKeys[cm.colKeyIndex])}`
    }),
    valueOf: Array.from({ length: totalCols }, (_, c) => c < labelCols ? null : valueId(req.values[colMap[c - labelCols]!.valueIndex]!)),
  }

  sheet.getRange(0, 0, totalRows, totalCols).setValues(matrix)
  // 读回基准，再套上手工格式（只写有格式的格子）
  const tracked = totalRows * totalCols <= MAX_STYLE_CELLS
  const base = tracked ? readStyles(sheet, totalRows, totalCols) : []
  if (tracked && spec.styles) {
    const patch: Record<number, Record<number, ICellData>> = {}
    for (let r = 0; r < totalRows; r++) {
      for (let c = 0; c < totalCols; c++) {
        const d = stylesAt(spec.styles, grid, r, c)
        if (d)
          (patch[r] ??= {})[c] = { ...matrix[r]![c], s: d as IStyleData }
      }
    }
    if (Object.keys(patch).length)
      sheet.getRange(0, 0, totalRows, totalCols).setValues(patch)
  }
  // 列键标题跨值字段合并
  if (m > 0 && values.length > 1) {
    groups.forEach((_, gi) => sheet.getRange(fieldRow - 1, labelCols + gi * values.length, 1, values.length).merge())
  }
  if (compact) {
    sheet.setColumnWidth(0, 28)
    sheet.setColumnWidth(1, 120 + 40 * k)
  }
  else {
    for (let c = 0; c < labelCols; c++)
      sheet.setColumnWidth(c, 160)
  }
  for (let c = labelCols; c < totalCols; c++)
    sheet.setColumnWidth(c, 130)
  sheet.setFreeze({ startRow: dataStartRow, startColumn: labelCols, ySplit: dataStartRow, xSplit: labelCols })

  layouts.set(sheet.getSheetId(), { spec, result, dataStartRow, labelCols, colMap, rows: shown.map(d => d.row), targets, grid, base })
  if (JSON.stringify(spec.styles ?? null) !== JSON.stringify(getPivotSpec(sheet)?.styles ?? null))
    saveSpec(sheet, { ...(getPivotSpec(sheet) ?? spec), styles: spec.styles })
}

function filterDescOf(req: PivotRequest) {
  const parts = (req.filters ?? []).map((f) => {
    if (f.op === 'in')
      return `${f.column}：${f.values.slice(0, 2).map(v => v || BLANK).join('、')}${f.values.length > 2 ? tr(' 等 {0} 项', [f.values.length]) : ''}`
    if (f.op === 'between')
      return `${f.column}：${f.min ?? '…'} ~ ${f.max ?? '…'}`
    if (f.op === 'contains')
      return tr('{0} 包含「{1}」', [f.column, f.value])
    if (f.op === 'prefixes')
      return tr('{0} 以「{1}」之一开头', [f.column, f.values.map(v => v || BLANK).join('」「')])
    return tr('{0} 以「{1}」开头', [f.column, f.value])
  })
  if (req.keyword)
    parts.push(tr('关键字「{0}」', [req.keyword]))
  if (req.top)
    parts.push(tr('前 {0} 项', [req.top]))
  return parts.join('；')
}

// ── 透视表定义被页签内操作修改（折叠、改名）时通知面板同步 ──
type Listener = (sheetId: string, spec: PivotSpec, result: PivotResult) => void
const listeners = new Set<Listener>()
export function onPivotChanged(fn: Listener) {
  listeners.add(fn)
  return () => listeners.delete(fn)
}
function notify(sheet: FWorksheet, spec: PivotSpec, result: PivotResult) {
  listeners.forEach(fn => fn(sheet.getSheetId(), spec, result))
}

function saveSpec(sheet: FWorksheet, spec: PivotSpec) {
  sheet.setCustomMetadata({ ...(sheet.getCustomMetadata() ?? {}), [META_KEY]: spec })
}

/** 计算并写入；返回计算结果（供面板显示耗时、行数） */
export async function refreshPivot(sheet: FWorksheet, spec: PivotSpec, signal?: AbortSignal): Promise<PivotResult> {
  const result = await getPivotApi(spec.datasetId, spec.request, signal)
  // 面板传来的定义没有手工格式：沿用页签上的（render 里会再按页签现状合并）
  saveSpec(sheet, { ...spec, styles: spec.styles ?? getPivotSpec(sheet)?.styles })
  render(sheet, spec, result)
  return result
}

/** 只改版式（表格 / 紧凑、重复标签、折叠）：用上次的结果重画，不重新计算 */
export async function setPivotDisplay(sheet: FWorksheet, display: PivotDisplay) {
  const spec = getPivotSpec(sheet)
  if (!spec)
    return
  const next: PivotSpec = { ...spec, display: { ...display } }
  const layout = layouts.get(sheet.getSheetId())
  if (!layout) {
    if (next.request.values.length)
      await refreshPivot(sheet, next)
    else
      saveSpec(sheet, next)
    return
  }
  saveSpec(sheet, next)
  render(sheet, next, layout.result)
}

/** 紧凑版式下第 level 级（0 起）的全部分组前缀，用于「全部折叠」 */
export function pivotGroupPrefixes(sheetId: string, level = 0): string[] {
  const layout = layouts.get(sheetId)
  if (!layout)
    return []
  const set = new Set<string>()
  for (const r of layout.result.rows) {
    if (r.kind === 'data' && r.keys.length > level + 1)
      set.add(JSON.stringify(r.keys.slice(0, level + 1)))
  }
  return [...set]
}

/** 新建透视表页签并计算 */
export async function createPivotSheet(api: FUniver, spec: PivotSpec): Promise<FWorksheet> {
  const workbook = api.getActiveWorkbook()
  if (!workbook)
    throw new Error(tr('工作簿尚未就绪'))
  const taken = new Set(workbook.getSheets().map(s => s.getSheetName()))
  let n = 1
  while (taken.has(`透视表${n}`))
    n++
  const sheet = workbook.insertSheet(`透视表${n}`)
  sheet.setCustomMetadata({ [META_KEY]: spec })
  workbook.setActiveSheet(sheet)
  return sheet
}

function ymd(t: number) {
  return new Date(t).toISOString().slice(0, 10)
}

/** 日期分组的取值 → 明细筛选（季度、ISO 周换算成日期区间） */
function periodFilter(f: PivotField, key: string | null): DatasetFilter {
  const end = '\uFFFF'
  const q = key?.match(/^(\d{4})-Q([1-4])$/)
  if (f.granularity === 'quarter' && q) {
    const first = (Number(q[2]) - 1) * 3 + 1
    return { op: 'between', column: f.column, min: `${q[1]}-${String(first).padStart(2, '0')}`, max: `${q[1]}-${String(first + 2).padStart(2, '0')}${end}` }
  }
  const w = key?.match(/^(\d{4})-W(\d{2})$/)
  if (f.granularity === 'week' && w) {
    const jan4 = Date.UTC(Number(w[1]), 0, 4)
    const monday = jan4 - ((new Date(jan4).getUTCDay() + 6) % 7) * 86400000 + (Number(w[2]) - 1) * 7 * 86400000
    return { op: 'between', column: f.column, min: ymd(monday), max: `${ymd(monday + 6 * 86400000)}${end}` }
  }
  return { op: 'prefix', column: f.column, value: key ?? '' }
}

/** 透视表单元格 → 明细下钻（固定筛选 + 核对基准） */
export function pivotDrillAt(sheetId: string, row: number, col: number): DatasetDrill & { datasetId: string } | null {
  const layout = layouts.get(sheetId)
  if (!layout)
    return null
  const ri = row - layout.dataStartRow
  const ci = col - layout.labelCols
  const prow = ri >= 0 ? layout.rows[ri] : null
  if (!prow)
    return null
  const { spec, result } = layout
  const req = spec.request
  const filters: DatasetFilter[] = [...(req.filters ?? [])]
  const toFilter = (f: PivotField, key: string | null): DatasetFilter => f.granularity
    ? periodFilter(f, key)
    : { op: 'in', column: f.column, values: [key ?? ''] }
  // 计算项是公式算出来的虚拟取值，没有对应的明细行
  const isCalcItem = (f: PivotField | undefined, key: string | null) => key != null && !!f && !!req.calcItems?.some(c => c.field === f.column && c.name === key)
  if (prow.keys.some((key, i) => isCalcItem(req.rows[i], key)))
    return null
  prow.keys.forEach((key, i) => filters.push(toFilter(req.rows[i]!, key)))
  const labels = prow.keys.map((key, i) => labelOf(req, req.rows[i], key))

  // 选中的是标签列：按整行（全部列、第一个值）下钻
  const cm = layout.colMap[ci] ?? { colKeyIndex: null, valueIndex: 0 }
  if (cm.colKeyIndex != null) {
    const ck = result.columnKeys[cm.colKeyIndex]!
    if (ck.some((key, i) => isCalcItem(req.columns[i], key)))
      return null
    ck.forEach((key, i) => filters.push(toFilter(req.columns[i]!, key)))
    labels.push(...ck.map((key, i) => labelOf(req, req.columns[i], key)))
  }
  const info = result.values[cm.valueIndex]!
  const cellIndex = cm.colKeyIndex == null ? prow.cells.length - 1 : cm.colKeyIndex
  // 占比 / 同比 / 环比的格子：明细是本期（该格子范围）的行，用原始汇总值核对
  const value = (prow.raw ?? prow.cells)[cellIndex]?.[cm.valueIndex]
  const baseLabel = info.show && info.show !== 'raw' ? info.label.replace(/ · .*$/, '') : info.label
  const scope = labels.length ? labels.join(' / ') : '全部'
  let expected: DatasetDrill['expected']
  if (typeof value === 'number') {
    if (info.agg === 'sum' && info.column)
      expected = { label: tr('透视表「{0}」', [baseLabel]), value, kind: 'columnSum', column: info.column }
    else if (info.agg === 'count' && !info.column)
      expected = { label: tr('透视表「计数」'), value, kind: 'count' }
  }
  return {
    datasetId: spec.datasetId,
    title: `${info.label} · ${scope}`,
    filters,
    expected,
  }
}

/** 页签是否已在本次会话中刷新过（打开工作簿后首次切到透视表时按最新数据重算） */
export function hasLayout(sheetId: string) {
  return layouts.has(sheetId)
}

// ── 页签内操作：点 ▾ / ▸ 折叠展开，改标签 / 表头文字即设别名 ──

/** 设别名后的定义；返回 null 表示没有变化 */
function withAlias(spec: PivotSpec, target: CellTarget, text: string): PivotSpec | null {
  const req: PivotRequest = structuredClone(spec.request)
  const setAlias = (field: PivotField | undefined, key: string | null) => {
    if (!field)
      return false
    const raw = key ?? ''
    const map = { ...req.aliases?.[field.column] }
    const reset = !text || text === keyText(key) || text === raw
    if (reset ? !(raw in map) : map[raw] === text)
      return false
    if (reset)
      delete map[raw]
    else
      map[raw] = text
    const aliases = { ...req.aliases, [field.column]: map }
    if (!Object.keys(map).length)
      delete aliases[field.column]
    req.aliases = aliases
    return true
  }
  const setLabel = (item: { label?: string } | undefined, auto: string) => {
    if (!item)
      return false
    const next = !text || text === auto ? undefined : text
    if ((item.label || undefined) === next)
      return false
    if (next)
      item.label = next
    else
      delete item.label
    return true
  }
  let changed = false
  switch (target.kind) {
    case 'key':
      changed = setAlias(req.rows[target.field], target.key)
      break
    case 'colKey':
      changed = setAlias(req.columns[0], target.key)
      break
    case 'rowField': {
      const f = req.rows[target.index]
      changed = !!f && setLabel(f, fieldTitle({ ...f, label: undefined }))
      break
    }
    case 'value':
      changed = setLabel(req.values[target.index], '')
      break
    default:
      return null
  }
  return changed ? { ...spec, request: req } : null
}

let registered: FUniver | null = null

/** 注册页签内的折叠与改名（工作簿打开后调用一次） */
export function registerPivotInteractions(api: FUniver, onError: (e: unknown) => void) {
  if (registered === api)
    return
  registered = api
  // 用户在透视表页签里改格式（样式、数字格式、边框…）：记进定义，重算后重新套上
  const pending = new Set<string>()
  api.addEvent(api.Event.CommandExecuted, (e) => {
    const params = e.params as { subUnitId?: string } | undefined
    const sheetId = params?.subUnitId
    if (rendering || e.type !== 2 || !sheetId || !layouts.has(sheetId) || pending.has(sheetId))
      return
    if (e.id !== 'sheet.mutation.set-range-values' && !e.id.includes('numfmt'))
      return
    pending.add(sheetId)
    setTimeout(() => {
      pending.delete(sheetId)
      const ws = api.getActiveWorkbook()?.getSheetBySheetId(sheetId)
      if (ws)
        syncStyles(ws)
    }, 0)
  })
  api.addEvent(api.Event.CellClicked, ({ worksheet, row, column }) => {
    const layout = layouts.get(worksheet.getSheetId())
    const target = layout?.targets.get(`${row},${column}`)
    if (!layout || target?.kind !== 'toggle')
      return
    const collapsed = new Set(layout.spec.display?.collapsed ?? [])
    if (!collapsed.delete(target.prefix))
      collapsed.add(target.prefix)
    // 在点击处理完之后再重画
    setTimeout(() => {
      const spec = getPivotSpec(worksheet)
      if (!spec)
        return
      setPivotDisplay(worksheet, { ...spec.display, collapsed: [...collapsed] })
        .then(() => {
          const now = layouts.get(worksheet.getSheetId())
          if (now)
            notify(worksheet, now.spec, now.result)
        })
        .catch(onError)
    }, 0)
  })
  api.addEvent(api.Event.SheetEditEnded, ({ worksheet, row, column, isConfirm }) => {
    if (!isConfirm)
      return
    const layout = layouts.get(worksheet.getSheetId())
    const target = layout?.targets.get(`${row},${column}`)
    if (!layout || !target || target.kind === 'toggle')
      return
    setTimeout(() => {
      const raw = worksheet.getRange(row, column).getValue()
      // 去掉紧凑版式的缩进、小计行的「汇总」后缀
      let text = String(raw ?? '').replace(/^\s+/, '').trim()
      if (target.kind === 'key' && layout.rows[row - layout.dataStartRow]?.kind === 'subtotal' && !isCompact(layout.spec))
        text = text.replace(/\s*汇总$/, '')
      const next = withAlias(layout.spec, target, text)
      if (!next) {
        // 没有变化（或改的是不能改名的格子）：恢复原样
        render(worksheet, layout.spec, layout.result)
        return
      }
      refreshPivot(worksheet, next)
        .then(result => notify(worksheet, next, result))
        .catch((e) => {
          render(worksheet, layout.spec, layout.result)
          onError(e)
        })
    }, 0)
  })
}
