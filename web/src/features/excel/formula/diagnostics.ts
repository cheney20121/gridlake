import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import type { Cycle, FormulaCell } from './circular'
import { cellName } from '@/features/excel/writeback/input-table'
import { t } from '@/i18n'
import { findCycles } from './circular'
import { colIndex, parseRefs, stripStrings } from './formula-refs'

/**
 * 公式诊断：定位可能拖慢计算的公式与出错的公式。
 *
 * Univer 不提供单个公式的耗时，这里按「引用的格子数」估算开销：每个公式引用的区域格子数之和，
 * 整列引用按工作表实际用到的行数计。同一列向下复制的同形公式（相对位置一致）合并成一组，组的开销 = 单个 × 个数。
 * 易失函数（NOW、OFFSET、INDIRECT 等）每次任何修改都会重算，单独标出；服务端计算的公式（数据集、数据表、跨工作簿）另计。
 * 循环引用按公式之间的引用关系检测（见 circular.ts）。
 */

export interface FormulaGroup {
  sheetId: string
  sheetName: string
  /** 组内第一个格子 */
  row: number
  col: number
  /** 组内格子的地址范围说明（如 C2:C5001） */
  where: string
  count: number
  formula: string
  /** 单个公式引用的格子数 */
  cellsEach: number
  /** 估算开销 = 单个 × 个数 */
  cost: number
  wholeColumn: boolean
  volatile: string[]
  server: boolean
  advice: string[]
}

export interface FormulaError {
  sheetId: string
  sheetName: string
  row: number
  col: number
  address: string
  formula: string
  value: string
}

export interface DiagnosticsReport {
  formulas: number
  sheets: { sheetId: string, name: string, formulas: number }[]
  groups: FormulaGroup[]
  errors: FormulaError[]
  /** 循环引用（互相牵连的一组一条，给出一条环路）；cyclesPartial：公式之间的引用太多，只检查了一部分 */
  cycles: Cycle[]
  cyclesPartial: boolean
  volatileCount: number
  serverCount: number
  elapsedMs: number
}

const VOLATILE = ['NOW', 'TODAY', 'RAND', 'RANDBETWEEN', 'RANDARRAY', 'OFFSET', 'INDIRECT', 'CELL', 'INFO']
const SERVER = /\b(?:DSUMIFS|DCOUNTIFS|DAVERAGEIFS|DMAXIFS|DMINIFS|DLOOKUP|TSUM|TCOUNT|TAVERAGE|TMIN|TMAX|TVALUE|IMPORTRANGE)\s*\(/i
const LOOKUP = /\b(?:VLOOKUP|HLOOKUP|XLOOKUP|MATCH|XMATCH|SUMIFS?|COUNTIFS?|AVERAGEIFS?|MAXIFS|MINIFS|SUMPRODUCT|FILTER|UNIQUE|SORT)\s*\(/i
/** Excel 错误值与平台函数返回的「#说明」 */
const ERROR_VALUE = /^#(?:REF!|NAME\?|VALUE!|DIV\/0!|N\/A|NUM!|NULL!|SPILL!|CALC!|GETTING_DATA)|^#\S/

interface SheetSize { rows: number, cols: number }

/** 一个公式引用的格子数，以及有没有整列 / 整行引用 */
function referencedCells(formula: string, size: (sheet: string | null) => SheetSize) {
  let cells = 0
  let whole = false
  for (const r of parseRefs(formula)) {
    const s = size(r.sheet)
    if (r.r1 == null) {
      whole = true
      cells += (r.c2! - r.c1! + 1) * s.rows
    }
    else if (r.c1 == null) {
      whole = true
      cells += (r.r2! - r.r1! + 1) * s.cols
    }
    else {
      cells += (r.c2! - r.c1 + 1) * (r.r2! - r.r1 + 1)
    }
  }
  return { cells, whole }
}

/** 公式的「形状」：相对引用换成相对当前格子的偏移，向下 / 向右复制的同形公式得到相同的形状 */
function shapeOf(formula: string, row: number, col: number) {
  return stripStrings(formula).replace(/(\$?)([A-Z]{1,3})(\$?)(\d+)(?![\w(])/gi, (_, dc: string, c: string, dr: string, r: string) => {
    const cPart = dc ? `C${colIndex(c)}` : `C[${colIndex(c) - col}]`
    const rPart = dr ? `R${Number(r) - 1}` : `R[${Number(r) - 1 - row}]`
    return rPart + cPart
  })
}

function adviceFor(g: Omit<FormulaGroup, 'advice'>) {
  const out: string[] = []
  if (g.wholeColumn)
    out.push(t('整列引用（如 D:D）会扫描整列，改成实际范围（如 D2:D5000）或把数据放进数据表'))
  if (g.volatile.length)
    out.push(`${g.volatile.join('、')} 是易失函数，任何修改都会让它重算${g.volatile.includes('OFFSET') || g.volatile.includes('INDIRECT') ? '；OFFSET / INDIRECT 可改用 INDEX' : ''}`)
  if (LOOKUP.test(g.formula) && g.count >= 200 && g.cellsEach >= 1000)
    out.push(t('大量按条件汇总 / 查找：可改用透视表，或数据集公式 DSUMIFS / DLOOKUP（服务端一次算完）'))
  if (!out.length && g.cost >= 1_000_000)
    out.push(t('引用的格子很多，考虑缩小范围或先汇总再引用'))
  return out
}

function scanSheet(sheet: FWorksheet, size: (name: string | null) => SheetSize) {
  const lastRow = sheet.getLastRow()
  const lastCol = sheet.getLastColumn()
  const groups = new Map<string, FormulaGroup & { cells: { r: number, c: number }[] }>()
  const errors: FormulaError[] = []
  const formulaCells: FormulaCell[] = []
  let formulas = 0
  if (lastRow < 0 || lastCol < 0)
    return { formulas, groups: [], errors, formulaCells }
  const range = sheet.getRange(0, 0, lastRow + 1, lastCol + 1)
  const fs = range.getFormulas()
  const vs = range.getValues()
  const sheetId = sheet.getSheetId()
  const sheetName = sheet.getSheetName()
  fs.forEach((row, r) => row.forEach((f, c) => {
    if (!f)
      return
    formulas++
    formulaCells.push({ sheet: sheetName, row: r, col: c, formula: f })
    const v = vs[r]?.[c]
    if (typeof v === 'string' && ERROR_VALUE.test(v))
      errors.push({ sheetId, sheetName, row: r, col: c, address: cellName(r, c), formula: f, value: v })
    const key = `${c}|${shapeOf(f, r, c)}`
    const g = groups.get(key)
    if (g) {
      g.count++
      g.cells.push({ r, c })
      return
    }
    const { cells, whole } = referencedCells(f, name => (name == null ? size(sheetName) : size(name)))
    const upper = f.toUpperCase()
    groups.set(key, {
      sheetId,
      sheetName,
      row: r,
      col: c,
      where: '',
      count: 1,
      formula: f,
      cellsEach: cells,
      cost: 0,
      wholeColumn: whole,
      volatile: VOLATILE.filter(fn => new RegExp(String.raw`\b${fn}\s*\(`).test(upper)),
      server: SERVER.test(f),
      advice: [],
      cells: [{ r, c }],
    })
  }))
  const out: FormulaGroup[] = [...groups.values()].map(({ cells, ...g }) => {
    const last = cells[cells.length - 1]!
    const where = cells.length === 1 ? cellName(g.row, g.col) : `${cellName(g.row, g.col)}:${cellName(last.r, last.c)}`
    const base = { ...g, where, cost: g.cellsEach * g.count }
    return { ...base, advice: adviceFor(base) }
  })
  return { formulas, groups: out, errors, formulaCells }
}

/** 扫描整个工作簿 */
export function analyzeFormulas(api: FUniver): DiagnosticsReport {
  const t0 = performance.now()
  const sheets = api.getActiveWorkbook()?.getSheets() ?? []
  const sizes = new Map(sheets.map(s => [s.getSheetName(), { rows: Math.max(s.getLastRow() + 1, 1), cols: Math.max(s.getLastColumn() + 1, 1) }]))
  const size = (name: string | null) => sizes.get(name ?? '') ?? { rows: 1, cols: 1 }
  const report: DiagnosticsReport = { formulas: 0, sheets: [], groups: [], errors: [], cycles: [], cyclesPartial: false, volatileCount: 0, serverCount: 0, elapsedMs: 0 }
  const allCells: FormulaCell[] = []
  for (const sheet of sheets) {
    const r = scanSheet(sheet, size)
    report.formulas += r.formulas
    report.sheets.push({ sheetId: sheet.getSheetId(), name: sheet.getSheetName(), formulas: r.formulas })
    report.groups.push(...r.groups)
    report.errors.push(...r.errors)
    for (const c of r.formulaCells)
      allCells.push(c)
  }
  const found = findCycles(allCells, size)
  report.cycles = found.cycles
  report.cyclesPartial = found.partial
  report.volatileCount = report.groups.filter(g => g.volatile.length).reduce((n, g) => n + g.count, 0)
  report.serverCount = report.groups.filter(g => g.server).reduce((n, g) => n + g.count, 0)
  report.groups.sort((a, b) => b.cost - a.cost || b.count - a.count)
  report.elapsedMs = Math.round(performance.now() - t0)
  return report
}

/** 实测一次全量重算的耗时（毫秒）；超时返回 null */
export async function measureRecalc(api: FUniver, timeoutMs = 60_000): Promise<number | null> {
  const formula = api.getFormula()
  const t0 = performance.now()
  formula.executeCalculation()
  try {
    await formula.onCalculationResultApplied(timeoutMs)
    return Math.round(performance.now() - t0)
  }
  catch {
    return null
  }
}
