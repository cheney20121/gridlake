/**
 * 透视表的手工格式：用户在透视表页签里改的格式（数字格式、颜色、加粗、边框…）在重算后保留。
 *
 * 透视表每次重算都整页重画，格子的位置会变（筛选、排序、新增行），所以格式不能按位置记，而是按「含义」记：
 * - 值：某个值字段的全部明细格子（如「毛利率」整列设成百分比，新出现的月份 / 店铺也是百分比）
 * - 列：某个值字段在某个列键下的明细格子
 * - 行：某一行（按行键，如「A 店」那一行、「总计」行）的全部值格子
 * - 格：某一行 × 某一列的单个格子（标签、表头、小计 / 总计格子也按格记）
 *
 * 记录方式：重画后立刻读回每个格子的样式作为基准；之后与基准不同的部分就是用户的改动（差异，值为 null 表示用户去掉了这一项）。
 * 差异逐级提取：同一个值的明细格子共有的部分记到「值」，剩下的同一列共有的记到「列」，同一行共有的记到「行」，其余记到「格」。
 * 只有一个格子时无法判断是整列还是单格：沿用上次记录的级别（如上次是整列格式、这次只剩一行，仍按整列记）。
 */

/** 样式差异：样式键 → 新值（null = 去掉这一项） */
export type StyleDelta = Record<string, unknown>

export interface PivotStyles {
  value?: Record<string, StyleDelta>
  col?: Record<string, StyleDelta>
  row?: Record<string, StyleDelta>
  cell?: Record<string, StyleDelta>
}

/** 一次重画的格子含义 */
export interface StyleGrid {
  /** 每行的含义：标题 'T'、表头 'H:…'、数据行 `${kind}:${行键 JSON}` */
  rowIds: string[]
  /** 是否明细行（只有明细行参与「值」「列」级格式；小计、总计按行 / 格记） */
  dataRow: boolean[]
  /** 每列的含义：标签列 `L:序号`，值列 `${值标识}|${列键 JSON 或 Σ}` */
  colIds: string[]
  /** 值列所属的值标识；标签列为 null */
  valueOf: (string | null)[]
}

const SEP = '\u0001'
export const cellKey = (rowId: string, colId: string) => `${rowId}${SEP}${colId}`

const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null)

/** 当前样式相对基准的差异；没有差异返回 null */
export function styleDelta(base: Record<string, unknown> | null | undefined, cur: Record<string, unknown> | null | undefined): StyleDelta | null {
  const out: StyleDelta = {}
  for (const k of new Set([...Object.keys(base ?? {}), ...Object.keys(cur ?? {})])) {
    const a = base?.[k]
    const b = cur?.[k]
    if (!same(a, b))
      out[k] = b ?? null
  }
  return Object.keys(out).length ? out : null
}

/** 多个格子共有的差异（每一项在所有格子里都有且相同） */
function common(list: (StyleDelta | null)[]): StyleDelta | null {
  if (!list.length || list.some(d => !d))
    return null
  const [first, ...rest] = list as StyleDelta[]
  const out: StyleDelta = {}
  for (const [k, v] of Object.entries(first!)) {
    if (rest.every(d => k in d && same(d[k], v)))
      out[k] = v
  }
  return Object.keys(out).length ? out : null
}

/** 每个格子都包含 prev 的全部项时返回 prev（单个格子时沿用上次的级别） */
function stillHolds(list: (StyleDelta | null)[], prev: StyleDelta | undefined): StyleDelta | null {
  if (!prev || !list.length)
    return null
  return list.every(d => d && Object.entries(prev).every(([k, v]) => k in d && same(d[k], v))) ? prev : null
}

function subtract(d: StyleDelta | null, part: StyleDelta): StyleDelta | null {
  if (!d)
    return null
  const out = { ...d }
  for (const k of Object.keys(part)) delete out[k]
  return Object.keys(out).length ? out : null
}

/**
 * 从本次重画的格子差异提取格式，并与上次的记录合并：
 * 本次画出来的值 / 列 / 行 / 格以本次为准，没画出来的（被筛选掉的行、去掉的值字段）保留上次的记录
 */
export function extractStyles(grid: StyleGrid, deltas: (StyleDelta | null)[][], prev: PivotStyles | undefined): PivotStyles {
  const d = deltas.map(r => r.slice())
  const rows = grid.rowIds.length
  const cols = grid.colIds.length
  const dataRows = [...Array.from({ length: rows }).keys()].filter(r => grid.dataRow[r])
  const valueCols = [...Array.from({ length: cols }).keys()].filter(c => grid.valueOf[c] != null)
  const out: Required<PivotStyles> = { value: {}, col: {}, row: {}, cell: {} }

  const take = (level: keyof PivotStyles, id: string, cells: [number, number][]) => {
    const list = cells.map(([r, c]) => d[r]?.[c] ?? null)
    const part = (list.length >= 2 ? common(list) : null) ?? stillHolds(list, prev?.[level]?.[id])
    if (!part)
      return
    out[level][id] = part
    for (const [r, c] of cells) d[r]![c] = subtract(d[r]![c] ?? null, part)
  }

  const byValue = new Map<string, number[]>()
  for (const c of valueCols) byValue.set(grid.valueOf[c]!, [...(byValue.get(grid.valueOf[c]!) ?? []), c])
  byValue.forEach((cs, vid) => take('value', vid, cs.flatMap(c => dataRows.map(r => [r, c] as [number, number]))))
  for (const c of valueCols) take('col', grid.colIds[c]!, dataRows.map(r => [r, c]))
  for (let r = 0; r < rows; r++) {
    if (grid.rowIds[r]!.startsWith('H:') || grid.rowIds[r] === 'T')
      continue
    take('row', grid.rowIds[r]!, valueCols.map(c => [r, c]))
  }
  for (let r = 0; r < rows; r++) {
    for (let c = 0; c < cols; c++) {
      const x = d[r]?.[c]
      if (x)
        out.cell[cellKey(grid.rowIds[r]!, grid.colIds[c]!)] = x
    }
  }

  // 合并上次的记录：本次没有画出来的保留
  const shownRows = new Set(grid.rowIds)
  const shownCols = new Set(grid.colIds)
  const shownValues = new Set(byValue.keys())
  const keep = (level: keyof PivotStyles, shown: (id: string) => boolean) => {
    for (const [id, v] of Object.entries(prev?.[level] ?? {})) {
      if (!shown(id) && !(id in out[level]))
        out[level][id] = v
    }
  }
  keep('value', id => shownValues.has(id))
  keep('col', id => shownCols.has(id))
  keep('row', id => shownRows.has(id))
  keep('cell', (id) => {
    const [r, c] = id.split(SEP)
    return shownRows.has(r!) && shownCols.has(c!)
  })

  const result: PivotStyles = {}
  for (const level of ['value', 'col', 'row', 'cell'] as const) {
    if (Object.keys(out[level]).length)
      result[level] = out[level]
  }
  return result
}

/** 某个格子要叠加的格式（值 → 列 → 行 → 格，后者覆盖前者）；没有返回 null */
export function stylesAt(styles: PivotStyles | undefined, grid: StyleGrid, r: number, c: number): StyleDelta | null {
  if (!styles)
    return null
  const rowId = grid.rowIds[r]!
  const colId = grid.colIds[c]!
  const vid = grid.valueOf[c]
  const parts = [
    vid != null && grid.dataRow[r] ? styles.value?.[vid] : undefined,
    vid != null && grid.dataRow[r] ? styles.col?.[colId] : undefined,
    vid != null ? styles.row?.[rowId] : undefined,
    styles.cell?.[cellKey(rowId, colId)],
  ].filter(Boolean) as StyleDelta[]
  return parts.length ? Object.assign({}, ...parts) : null
}

export function isEmptyStyles(s: PivotStyles | undefined) {
  return !s || !(['value', 'col', 'row', 'cell'] as const).some(l => Object.keys(s[l] ?? {}).length)
}
