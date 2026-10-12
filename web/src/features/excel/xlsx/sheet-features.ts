import type { IWorkbookData } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import type { Worksheet } from 'exceljs'
import type { UniverIconConfig } from './icon-set'
import type { UniverTable } from './tables'
import { excelToIconSet, iconSetToExcel } from './icon-set'
import { exportTables, readExcelTables } from './tables'

/**
 * xlsx 互通：条件格式、数据验证（下拉列表等）、批注、超链接
 *
 * - 导出：读取工作簿快照里各插件的资源（与 Univer 内部存储一致），转成 exceljs 的结构
 * - 导入：把 exceljs 读到的规则转成插件的内部结构，通过插件命令添加（与在界面上创建的完全一致）
 * - 无法对应的规则（自定义的图标集、复选框、部分时间段条件）计数后提示，不静默丢弃
 */

interface IRange {
  startRow: number
  startColumn: number
  endRow: number
  endColumn: number
}

export interface FeatureStats {
  conditionalFormats: number
  dataValidations: number
  notes: number
  hyperlinks: number
  /** 表格（超级表） */
  tables: number
  /** 无法转换的规则说明 */
  skipped: string[]
}

export function emptyStats(): FeatureStats {
  return { conditionalFormats: 0, dataValidations: 0, notes: 0, hyperlinks: 0, tables: 0, skipped: [] }
}

// ── 地址与颜色 ──

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

function a1(r: IRange) {
  const tl = `${colName(r.startColumn)}${r.startRow + 1}`
  const br = `${colName(r.endColumn)}${r.endRow + 1}`
  return tl === br ? tl : `${tl}:${br}`
}

function parseA1(ref: string): IRange | null {
  const m = ref.trim().replace(/\$/g, '').match(/^([A-Z]+)(\d+)(?::([A-Z]+)(\d+))?$/i)
  if (!m)
    return null
  const col = (s: string) => s.toUpperCase().split('').reduce((a, ch) => a * 26 + ch.charCodeAt(0) - 64, 0) - 1
  const r1 = Number(m[2]) - 1
  const c1 = col(m[1]!)
  const r2 = m[4] ? Number(m[4]) - 1 : r1
  const c2 = m[3] ? col(m[3]) : c1
  return { startRow: Math.min(r1, r2), startColumn: Math.min(c1, c2), endRow: Math.max(r1, r2), endColumn: Math.max(c1, c2) }
}

/** '#FDE2E1' / 'rgb(253,226,225)' → 'FFFDE2E1' */
export function toArgb(color: string | undefined | null): string | undefined {
  if (!color)
    return undefined
  const c = color.trim()
  const rgb = c.match(/^rgba?\((\d+)\s*,\s*(\d+)\s*,\s*(\d+)/i)
  if (rgb)
    return `FF${[rgb[1], rgb[2], rgb[3]].map(x => Number(x).toString(16).padStart(2, '0')).join('')}`.toUpperCase()
  const hex = c.replace('#', '')
  if (/^[0-9a-f]{6}$/i.test(hex))
    return `FF${hex}`.toUpperCase()
  if (/^[0-9a-f]{8}$/i.test(hex))
    return hex.toUpperCase()
  return undefined
}

/** 'FFFDE2E1' → '#FDE2E1'（Excel 主题色没有 argb 时返回 undefined） */
export function fromArgb(argb: string | undefined | null): string | undefined {
  if (!argb || !/^[0-9a-f]{6,8}$/i.test(argb))
    return undefined
  return `#${argb.slice(-6).toUpperCase()}`
}

export function resource<T>(snapshot: IWorkbookData, name: string): Record<string, T> {
  const r = snapshot.resources?.find(x => x.name === name)
  if (!r?.data)
    return {}
  try {
    return JSON.parse(r.data) as Record<string, T>
  }
  catch {
    return {}
  }
}

// ── 条件格式 ──

interface CfStyle {
  bg?: { rgb?: string }
  cl?: { rgb?: string }
  bl?: number
  it?: number
  ul?: { s?: number }
  st?: { s?: number }
}

interface CfValue { type: string, value?: number | string }

interface CfRule {
  cfId?: string
  ranges: IRange[]
  stopIfTrue?: boolean
  rule: {
    type: 'highlightCell' | 'dataBar' | 'colorScale' | 'iconSet'
    subType?: string
    operator?: string
    value?: unknown
    style?: CfStyle
    config?: unknown
    isShowValue?: boolean
  }
}

function cfStyleToExcel(style: CfStyle | undefined) {
  const out: Record<string, unknown> = {}
  const bg = toArgb(style?.bg?.rgb)
  if (bg)
    out.fill = { type: 'pattern', pattern: 'solid', bgColor: { argb: bg } }
  const font: Record<string, unknown> = {}
  const cl = toArgb(style?.cl?.rgb)
  if (cl)
    font.color = { argb: cl }
  if (style?.bl)
    font.bold = true
  if (style?.it)
    font.italic = true
  if (style?.ul?.s)
    font.underline = true
  if (style?.st?.s)
    font.strike = true
  if (Object.keys(font).length)
    out.font = font
  return out
}

function excelStyleToCf(style: { fill?: { bgColor?: { argb?: string }, fgColor?: { argb?: string } }, font?: { color?: { argb?: string }, bold?: boolean, italic?: boolean, underline?: unknown, strike?: boolean } } | undefined): CfStyle {
  const out: CfStyle = {}
  const bg = fromArgb(style?.fill?.bgColor?.argb ?? style?.fill?.fgColor?.argb)
  if (bg)
    out.bg = { rgb: bg }
  const cl = fromArgb(style?.font?.color?.argb)
  if (cl)
    out.cl = { rgb: cl }
  if (style?.font?.bold)
    out.bl = 1
  if (style?.font?.italic)
    out.it = 1
  if (style?.font?.underline)
    out.ul = { s: 1 }
  if (style?.font?.strike)
    out.st = { s: 1 }
  return out
}

function cfvoToExcel(v: CfValue) {
  const type = v.type === 'num' ? 'num' : v.type
  if (type === 'formula')
    return { type: 'formula', value: String(v.value ?? '').replace(/^=/, '') }
  return v.value == null || type === 'min' || type === 'max' ? { type } : { type, value: Number(v.value) }
}

function excelCfvoToCf(v: { type?: string, value?: number | string } | undefined): CfValue {
  const t = v?.type ?? 'min'
  if (t === 'min' || t === 'max')
    return { type: t }
  if (t === 'formula')
    return { type: 'formula', value: `=${String(v?.value ?? '')}` }
  return { type: t === 'number' ? 'num' : t, value: Number(v?.value ?? 0) }
}

const TIME_PERIOD_TO_EXCEL: Record<string, string> = {
  today: 'today',
  yesterday: 'yesterday',
  tomorrow: 'tomorrow',
  last7Days: 'last7Days',
  thisMonth: 'thisMonth',
  lastMonth: 'lastMonth',
  nextMonth: 'nextMonth',
  thisWeek: 'thisWeek',
  lastWeek: 'lastWeek',
  nextWeek: 'nextWeek',
}

/** 单条条件格式 → exceljs 规则；无法对应时返回原因 */
function cfToExcel(cf: CfRule, priority: number): Record<string, unknown> | string {
  const r = cf.rule
  const tl = cf.ranges[0] ? `${colName(cf.ranges[0].startColumn)}${cf.ranges[0].startRow + 1}` : 'A1'
  const all = cf.ranges.map(x => `$${colName(x.startColumn)}$${x.startRow + 1}:$${colName(x.endColumn)}$${x.endRow + 1}`).join(',')
  const base = { priority, stopIfTrue: !!cf.stopIfTrue }
  if (r.type === 'colorScale') {
    const cfg = [...(r.config as { index: number, color: string, value: CfValue }[])].sort((a, b) => a.index - b.index)
    return { ...base, type: 'colorScale', cfvo: cfg.map(c => cfvoToExcel(c.value)), color: cfg.map(c => ({ argb: toArgb(c.color) ?? 'FF000000' })) }
  }
  if (r.type === 'dataBar') {
    const cfg = r.config as { min: CfValue, max: CfValue, isGradient: boolean, positiveColor: string }
    return { ...base, type: 'dataBar', cfvo: [cfvoToExcel(cfg.min), cfvoToExcel(cfg.max)], color: { argb: toArgb(cfg.positiveColor) ?? 'FF638EC6' }, gradient: cfg.isGradient }
  }
  if (r.type === 'iconSet') {
    const ic = iconSetToExcel(r.config as UniverIconConfig[], r.isShowValue)
    return typeof ic === 'string' ? ic : { ...base, ...ic }
  }
  const style = cfStyleToExcel(r.style)
  const op = r.operator ?? ''
  switch (r.subType) {
    case 'number': {
      const vals = Array.isArray(r.value) ? r.value : [r.value]
      return { ...base, type: 'cellIs', operator: op, formulae: vals.map(v => Number(v)), style }
    }
    case 'text': {
      const text = String(r.value ?? '')
      const esc = text.replace(/"/g, '""')
      if (op === 'containsText')
        return { ...base, type: 'expression', formulae: [`NOT(ISERROR(SEARCH("${esc}",${tl})))`], style }
      if (op === 'notContainsText')
        return { ...base, type: 'expression', formulae: [`ISERROR(SEARCH("${esc}",${tl}))`], style }
      if (op === 'beginsWith')
        return { ...base, type: 'expression', formulae: [`LEFT(${tl},${text.length})="${esc}"`], style }
      if (op === 'endsWith')
        return { ...base, type: 'expression', formulae: [`RIGHT(${tl},${text.length})="${esc}"`], style }
      if (op === 'equal' || op === 'notEqual')
        return { ...base, type: 'cellIs', operator: op, formulae: [`"${esc}"`], style }
      if (op === 'containsBlanks')
        return { ...base, type: 'expression', formulae: [`LEN(TRIM(${tl}))=0`], style }
      if (op === 'notContainsBlanks')
        return { ...base, type: 'expression', formulae: [`LEN(TRIM(${tl}))>0`], style }
      if (op === 'containsErrors')
        return { ...base, type: 'expression', formulae: [`ISERROR(${tl})`], style }
      if (op === 'notContainsErrors')
        return { ...base, type: 'expression', formulae: [`NOT(ISERROR(${tl}))`], style }
      return `文本条件「${op}」`
    }
    case 'formula':
      return { ...base, type: 'expression', formulae: [String(r.value ?? '').replace(/^=/, '')], style }
    case 'duplicateValues':
    case 'uniqueValues':
      return { ...base, type: 'expression', formulae: [`COUNTIF(${all.split(',')[0]},${tl})${r.subType === 'duplicateValues' ? '>1' : '=1'}`], style }
    case 'average':
      return { ...base, type: 'aboveAverage', aboveAverage: op.startsWith('greater'), equalAverage: op.endsWith('OrEqual'), style }
    case 'rank': {
      const rr = r as unknown as { isBottom?: boolean, isPercent?: boolean, value?: number }
      return { ...base, type: 'top10', rank: Number(rr.value ?? 10), percent: !!rr.isPercent, bottom: !!rr.isBottom, style }
    }
    case 'timePeriod': {
      const tp = TIME_PERIOD_TO_EXCEL[op]
      return tp ? { ...base, type: 'timePeriod', timePeriod: tp, style } : `时间段条件「${op}」`
    }
    default:
      return `条件格式「${r.subType ?? r.type}」`
  }
}

/** exceljs 规则 → 条件格式；无法对应时返回原因 */
function excelToCf(rule: Record<string, any>, ranges: IRange[]): CfRule | string {
  const style = excelStyleToCf(rule.style)
  const stopIfTrue = !!rule.stopIfTrue
  switch (rule.type) {
    case 'colorScale': {
      const cfvo = (rule.cfvo ?? []) as { type?: string, value?: number }[]
      const colors = (rule.color ?? []) as { argb?: string }[]
      return { ranges, stopIfTrue, rule: { type: 'colorScale', config: cfvo.map((v, i) => ({ index: i, color: fromArgb(colors[i]?.argb) ?? '#ffffff', value: excelCfvoToCf(v) })) } }
    }
    case 'dataBar': {
      const cfvo = (rule.cfvo ?? []) as { type?: string, value?: number }[]
      const color = fromArgb(rule.color?.argb) ?? '#638EC6'
      return { ranges, stopIfTrue, rule: { type: 'dataBar', isShowValue: true, config: { min: excelCfvoToCf(cfvo[0]), max: excelCfvoToCf(cfvo[1] ?? { type: 'max' }), isGradient: rule.gradient !== false, positiveColor: color, nativeColor: '#FF0000' } } }
    }
    case 'cellIs': {
      const f = (rule.formulae ?? []) as (string | number)[]
      const nums = f.map(x => Number(x))
      if (nums.every(n => Number.isFinite(n))) {
        const value = rule.operator === 'between' || rule.operator === 'notBetween' ? [nums[0], nums[1]] : nums[0]
        return { ranges, stopIfTrue, rule: { type: 'highlightCell', subType: 'number', operator: rule.operator, value, style } }
      }
      const text = String(f[0] ?? '')
      if (/^".*"$/.test(text) && (rule.operator === 'equal' || rule.operator === 'notEqual'))
        return { ranges, stopIfTrue, rule: { type: 'highlightCell', subType: 'text', operator: rule.operator, value: text.slice(1, -1).replace(/""/g, '"'), style } }
      return { ranges, stopIfTrue, rule: { type: 'highlightCell', subType: 'formula', value: `=${cellIsAsFormula(rule.operator, f, ranges)}`, style } }
    }
    case 'containsText':
    case 'notContainsText':
    case 'beginsWith':
    case 'endsWith': {
      // exceljs 读取时不保留 text 属性，只有等价公式（如 NOT(ISERROR(SEARCH("店1",A2)))）：从公式里取出文本
      const formula = String(rule.formulae?.[0] ?? '')
      const text = rule.text ?? formula.match(/"((?:[^"]|"")*)"/)?.[1]?.replace(/""/g, '"') ?? ''
      const op = rule.type !== 'containsText'
        ? rule.type
        : rule.operator === 'notContains' || rule.operator === 'notContainsText' || /^ISERROR\(SEARCH/i.test(formula)
          ? 'notContainsText'
          : rule.operator === 'containsBlanks' || rule.operator === 'notContainsBlanks' || rule.operator === 'containsErrors' || rule.operator === 'notContainsErrors'
            ? rule.operator
            : 'containsText'
      return { ranges, stopIfTrue, rule: { type: 'highlightCell', subType: 'text', operator: op, value: text, style } }
    }
    case 'expression':
      return { ranges, stopIfTrue, rule: { type: 'highlightCell', subType: 'formula', value: `=${String(rule.formulae?.[0] ?? '')}`, style } }
    case 'aboveAverage':
      return { ranges, stopIfTrue, rule: { type: 'highlightCell', subType: 'average', operator: `${rule.aboveAverage === false ? 'lessThan' : 'greaterThan'}${rule.equalAverage ? 'OrEqual' : ''}`, style } }
    case 'top10':
      return { ranges, stopIfTrue, rule: { type: 'highlightCell', subType: 'rank', value: Number(rule.rank ?? 10), isPercent: !!rule.percent, isBottom: !!rule.bottom, style } as CfRule['rule'] }
    case 'timePeriod':
      return { ranges, stopIfTrue, rule: { type: 'highlightCell', subType: 'timePeriod', operator: rule.timePeriod, style } }
    case 'iconSet': {
      const ic = excelToIconSet(rule)
      return typeof ic === 'string' ? ic : { ranges, stopIfTrue, rule: { type: 'iconSet', isShowValue: ic.isShowValue, config: ic.config } }
    }
    default:
      return `条件格式「${rule.type}」`
  }
}

/** cellIs 的非数字条件（引用其他单元格）转为公式条件 */
function cellIsAsFormula(op: string, f: (string | number)[], ranges: IRange[]) {
  const tl = ranges[0] ? `${colName(ranges[0].startColumn)}${ranges[0].startRow + 1}` : 'A1'
  const sym: Record<string, string> = { greaterThan: '>', greaterThanOrEqual: '>=', lessThan: '<', lessThanOrEqual: '<=', equal: '=', notEqual: '<>' }
  if (op === 'between')
    return `AND(${tl}>=${f[0]},${tl}<=${f[1]})`
  if (op === 'notBetween')
    return `OR(${tl}<${f[0]},${tl}>${f[1]})`
  return `${tl}${sym[op] ?? '='}${f[0]}`
}

// ── 数据验证 ──

interface DvRule {
  uid?: string
  ranges: IRange[]
  type: string
  operator?: string
  formula1?: string
  formula2?: string
  allowBlank?: boolean
  errorStyle?: number
  showDropDown?: boolean
  error?: string
  errorTitle?: string
  prompt?: string
  promptTitle?: string
  showErrorMessage?: boolean
  showInputMessage?: boolean
}

const DV_TYPES_TO_EXCEL: Record<string, string> = { whole: 'whole', decimal: 'decimal', date: 'date', textLength: 'textLength', custom: 'custom', time: 'time' }

/** 日期条件：Univer 存 2026-01-01，Excel 需要 DATE() */
function dateFormula(v: string | undefined) {
  const m = v?.match(/^(\d{4})-(\d{1,2})-(\d{1,2})/)
  return m ? `DATE(${m[1]},${Number(m[2])},${Number(m[3])})` : (v ?? '').replace(/^=/, '')
}

function dvToExcel(dv: DvRule): Record<string, unknown> | string {
  const errorStyle = dv.errorStyle === 2 ? 'warning' : dv.errorStyle === 0 ? 'information' : 'stop'
  const common = {
    allowBlank: dv.allowBlank !== false,
    showErrorMessage: dv.errorStyle === 1 || dv.showErrorMessage === true,
    errorStyle,
    ...(dv.error ? { error: dv.error } : {}),
    ...(dv.errorTitle ? { errorTitle: dv.errorTitle } : {}),
    ...(dv.prompt ? { prompt: dv.prompt, showInputMessage: true } : {}),
    ...(dv.promptTitle ? { promptTitle: dv.promptTitle } : {}),
  }
  if (dv.type === 'list' || dv.type === 'listMultiple') {
    const f = dv.formula1 ?? ''
    if (f.startsWith('='))
      return { ...common, type: 'list', formulae: [f.slice(1)] }
    let values: string[]
    try {
      const parsed = JSON.parse(f)
      values = Array.isArray(parsed) ? parsed.map(String) : f.split(',')
    }
    catch {
      values = f.split(',')
    }
    const joined = values.map(v => v.trim()).filter(Boolean).join(',')
    if (joined.length > 255)
      return '下拉选项超过 Excel 255 字符上限'
    return { ...common, type: 'list', formulae: [`"${joined.replace(/"/g, '""')}"`] }
  }
  const type = DV_TYPES_TO_EXCEL[dv.type]
  if (!type)
    return dv.type === 'checkbox' ? '复选框' : `数据验证「${dv.type}」`
  if (type === 'custom')
    return { ...common, type, formulae: [(dv.formula1 ?? '').replace(/^=/, '')] }
  const conv = (v: string | undefined) => (type === 'date' ? dateFormula(v) : (v ?? '').replace(/^=/, ''))
  const formulae = [conv(dv.formula1), ...(dv.operator === 'between' || dv.operator === 'notBetween' ? [conv(dv.formula2)] : [])]
  return { ...common, type, operator: dv.operator ?? 'between', formulae }
}

function excelToDv(v: Record<string, any>, ranges: IRange[]): DvRule | string {
  const errorStyle = v.errorStyle === 'warning' ? 2 : v.errorStyle === 'information' ? 0 : v.showErrorMessage ? 1 : 2
  const common = {
    ranges,
    allowBlank: v.allowBlank !== false,
    errorStyle,
    ...(v.error ? { error: v.error } : {}),
    ...(v.prompt ? { prompt: v.prompt, showInputMessage: true } : {}),
  }
  const f = (v.formulae ?? []) as (string | number | Date)[]
  // 日期按最近的整天取（exceljs 读出的日期可能带时区偏移，如 12-31 读成 12-30 16:00）
  const text = (x: unknown) => (x instanceof Date ? new Date(Math.round(x.getTime() / 86400000) * 86400000).toISOString().slice(0, 10) : String(x ?? ''))
  if (v.type === 'list') {
    const first = text(f[0])
    if (/^".*"$/s.test(first)) {
      const values = first.slice(1, -1).replace(/""/g, '"').split(',').map(s => s.trim()).filter(Boolean)
      return { ...common, type: 'list', formula1: JSON.stringify(values), showDropDown: true }
    }
    return { ...common, type: 'list', formula1: `=${first.replace(/^=/, '')}`, showDropDown: true }
  }
  if (!DV_TYPES_TO_EXCEL[v.type])
    return `数据验证「${v.type}」`
  if (v.type === 'custom')
    return { ...common, type: 'custom', formula1: `=${text(f[0]).replace(/^=/, '')}` }
  const conv = (x: unknown) => {
    const s = text(x)
    const m = s.match(/^DATE\((\d+),(\d+),(\d+)\)$/i)
    return m ? `${m[1]}-${m[2]!.padStart(2, '0')}-${m[3]!.padStart(2, '0')}` : s
  }
  return { ...common, type: v.type, operator: v.operator ?? 'between', formula1: conv(f[0]), ...(f.length > 1 ? { formula2: conv(f[1]) } : {}) }
}

// ── 导出 ──

/** 把一个工作表的条件格式 / 数据验证 / 批注 / 超链接 / 表格写入 exceljs 工作表（表格名在整个工作簿里不能重名，用 usedTableNames 去重） */
export function exportSheetFeatures(snapshot: IWorkbookData, sheetId: string, ws: Worksheet, stats: FeatureStats, usedTableNames: Set<string> = new Set()) {
  const sheetName = snapshot.sheets[sheetId]?.name ?? ''
  const cfs = resource<CfRule[]>(snapshot, 'SHEET_CONDITIONAL_FORMATTING_PLUGIN')[sheetId] ?? []
  // Univer 规则列表靠前的优先级高
  cfs.forEach((cf, i) => {
    const rule = cfToExcel(cf, i + 1)
    if (typeof rule === 'string') {
      stats.skipped.push(`「${sheetName}」${rule}（${cf.ranges.map(a1).join(',')}）`)
      return
    }
    ws.addConditionalFormatting({ ref: cf.ranges.map(a1).join(' '), rules: [rule as never] })
    stats.conditionalFormats++
  })

  const dvs = resource<DvRule[]>(snapshot, 'SHEET_DATA_VALIDATION_PLUGIN')[sheetId] ?? []
  const dvApi = (ws as unknown as { dataValidations: { add: (address: string, v: unknown) => void } }).dataValidations
  for (const dv of dvs) {
    const v = dvToExcel(dv)
    if (typeof v === 'string') {
      stats.skipped.push(`「${sheetName}」${v}（${dv.ranges.map(a1).join(',')}）`)
      continue
    }
    for (const r of dv.ranges)
      dvApi.add(a1(r), v)
    stats.dataValidations++
  }

  const notes = resource<Record<string, Record<string, { note?: string }>>>(snapshot, 'SHEET_NOTE_PLUGIN')[sheetId] ?? {}
  for (const [r, cols] of Object.entries(notes)) {
    for (const [c, n] of Object.entries(cols ?? {})) {
      if (!n?.note)
        continue
      ws.getCell(Number(r) + 1, Number(c) + 1).note = n.note
      stats.notes++
    }
  }

  // 表格：放在最后，此时单元格的值与公式都已写好
  const tables = resource<{ tables?: UniverTable[] }>(snapshot, 'SHEET_TABLE_PLUGIN')[sheetId]?.tables ?? []
  stats.tables += exportTables(tables, ws, sheetName, usedTableNames, stats.skipped)

  // 超链接：单元格富文本里的链接区段（整格只取第一个链接，Excel 一个单元格只能有一个）
  const cellData = snapshot.sheets[sheetId]?.cellData ?? {}
  for (const [r, row] of Object.entries(cellData)) {
    for (const [c, cell] of Object.entries(row ?? {})) {
      const body = (cell as { p?: { body?: { dataStream?: string, customRanges?: { rangeType: number, properties?: { url?: string } }[] } } }).p?.body
      const link = body?.customRanges?.find(x => x.rangeType === 0 && x.properties?.url)
      if (!link)
        continue
      const text = (body?.dataStream ?? '').replace(/\r\n$/, '').replace(/\r?\n$/, '')
      const url = link.properties!.url!
      ws.getCell(Number(r) + 1, Number(c) + 1).value = url.startsWith('#')
        ? { text, hyperlink: url } as never
        : { text, hyperlink: url, tooltip: url } as never
      stats.hyperlinks++
    }
  }
}

// ── 导入 ──

/** 合并相邻的区域（exceljs 读取时按单元格逐个列出数据验证）：先按列纵向合并，再把等高的相邻列横向合并 */
export function mergeRanges(ranges: IRange[]): IRange[] {
  const byCol = [...ranges].sort((a, b) => a.startColumn - b.startColumn || a.endColumn - b.endColumn || a.startRow - b.startRow)
  const vertical: IRange[] = []
  for (const r of byCol) {
    const last = vertical[vertical.length - 1]
    if (last && last.startColumn === r.startColumn && last.endColumn === r.endColumn && r.startRow <= last.endRow + 1)
      last.endRow = Math.max(last.endRow, r.endRow)
    else
      vertical.push({ ...r })
  }
  const byRow = vertical.sort((a, b) => a.startRow - b.startRow || a.endRow - b.endRow || a.startColumn - b.startColumn)
  const out: IRange[] = []
  for (const r of byRow) {
    const last = out[out.length - 1]
    if (last && last.startRow === r.startRow && last.endRow === r.endRow && r.startColumn <= last.endColumn + 1)
      last.endColumn = Math.max(last.endColumn, r.endColumn)
    else
      out.push({ ...r })
  }
  return out
}

/** 把 exceljs 工作表里的条件格式 / 数据验证 / 批注 / 超链接 / 表格加到刚导入的 Univer 工作表 */
export async function importSheetFeatures(api: FUniver, fsheet: FWorksheet, ws: Worksheet, stats: FeatureStats) {
  const unitId = api.getActiveWorkbook()!.getId()
  const subUnitId = fsheet.getSheetId()
  const name = fsheet.getSheetName()

  const cfList = ((ws as unknown as { conditionalFormattings?: { ref: string, rules: Record<string, any>[] }[] }).conditionalFormattings ?? [])
  // Excel 优先级数字越小越优先；Univer 规则列表靠前的优先，且添加命令把新规则放在列表最前 —— 按优先级从低到高依次添加
  const all = cfList
    .flatMap(cf => cf.rules.map(rule => ({ rule, ref: cf.ref, ranges: cf.ref.split(/\s+/).map(parseA1).filter((x): x is IRange => !!x) })))
    .filter(x => x.ranges.length)
    .sort((a, b) => (b.rule.priority ?? 0) - (a.rule.priority ?? 0))
  for (const { rule, ref, ranges } of all) {
    const r = excelToCf(rule, ranges)
    if (typeof r === 'string') {
      stats.skipped.push(`「${name}」${r}（${ref}）`)
      continue
    }
    const ok = await api.executeCommand('sheet.command.add-conditional-rule', { unitId, subUnitId, rule: r }).catch(() => false)
    if (ok)
      stats.conditionalFormats++
    else
      stats.skipped.push(`「${name}」条件格式（${ref}）`)
  }

  // 表格：Univer 的表格插件按区域创建，标题行文字成为列名
  for (const t of readExcelTables(ws, name, stats.skipped)) {
    const ok = await Promise.resolve(fsheet.addTable(t.name, t.range)).catch(() => false)
    if (ok) {
      stats.tables++
      if (t.note)
        stats.skipped.push(`「${name}」表格「${t.name}」（${t.note}）`)
    }
    else {
      stats.skipped.push(`「${name}」表格「${t.name}」`)
    }
  }

  const dvModel = (ws as unknown as { dataValidations?: { model?: Record<string, Record<string, any>> } }).dataValidations?.model ?? {}
  // exceljs 按单元格（或区域）逐个列出；相同规则的单元格合并为一条
  const groups = new Map<string, { v: Record<string, any>, ranges: IRange[] }>()
  for (const [addr, v] of Object.entries(dvModel)) {
    const ranges = addr.split(/\s+/).map(a => parseA1(a.replace(/^range:/, ''))).filter((x): x is IRange => !!x)
    const key = JSON.stringify(v)
    const g = groups.get(key) ?? { v, ranges: [] }
    g.ranges.push(...ranges)
    groups.set(key, g)
  }
  let n = 0
  for (const { v, ranges } of groups.values()) {
    const rule = excelToDv(v, mergeRanges(ranges))
    if (typeof rule === 'string') {
      stats.skipped.push(`「${name}」${rule}`)
      continue
    }
    const ok = await api.executeCommand('sheet.command.addDataValidation', { unitId, subUnitId, rule: { ...rule, uid: `dv_${Date.now().toString(36)}_${n++}` } }).catch(() => false)
    if (ok)
      stats.dataValidations++
    else
      stats.skipped.push(`「${name}」数据验证（${ranges.map(a1).join(',')}）`)
  }

  const links: { r: number, c: number, text: string, url: string }[] = []
  ws.eachRow({ includeEmpty: false }, (row, rn) => {
    row.eachCell({ includeEmpty: false }, (cell, cn) => {
      const note = cell.note
      const text = typeof note === 'string' ? note : (note as { texts?: { text: string }[] } | undefined)?.texts?.map(t => t.text).join('') ?? ''
      if (text.trim()) {
        const range = fsheet.getRange(rn - 1, cn - 1) as unknown as { createOrUpdateNote?: (n: { note: string, width?: number, height?: number }) => void }
        range.createOrUpdateNote?.({ note: text.trim(), width: 160, height: 72 })
        stats.notes++
      }
      const url = cell.hyperlink || (cell.value as { hyperlink?: string } | null)?.hyperlink
      if (url)
        links.push({ r: rn - 1, c: cn - 1, text: cell.text || url, url })
    })
  })
  for (const l of links) {
    const range = fsheet.getRange(l.r, l.c) as unknown as { setHyperLink?: (url: string, label?: string) => Promise<boolean> }
    if (await range.setHyperLink?.(l.url, l.text).catch(() => false))
      stats.hyperlinks++
  }
}
