import { parseRefs } from './formula-refs'

/**
 * 循环引用检测：A1 的公式用到 B1，B1 又用到 A1（或公式引用了包含自己的区域，如 A5 里写 =SUM(A:A)）。
 * 把每个有公式的格子当作图的点，公式引用到的有公式的格子当作边，找强连通分量（Tarjan，用循环实现，公式很多时也不会爆栈）。
 * 没有公式的格子不会让循环成立，所以只需要看有公式的格子。
 *
 * 只认 A1 写法的引用；INDIRECT / OFFSET 拼出来的引用、命名区域看不到。引用区域里有大量公式时边会很多，
 * 超过 maxEdges 就停止并标记为「只检查了一部分」，不会卡住页面
 */

export interface FormulaCell {
  sheet: string
  row: number
  col: number
  formula: string
}

export interface CellRef {
  sheet: string
  row: number
  col: number
}

export interface Cycle {
  /** 沿着引用走一圈回到起点的路径（首尾是同一个格子）；很长时只给前面一部分 */
  path: CellRef[]
  /** 这一组互相牵连的格子总数（可能比 path 多：几个环连在一起） */
  size: number
  /** 路径被截短了 */
  truncated: boolean
}

export interface CycleResult {
  cycles: Cycle[]
  /** 边太多，只检查了一部分公式 */
  partial: boolean
}

const MAX_EDGES = 3_000_000
const MAX_PATH = 12
const MAX_CYCLES = 50

export function findCycles(cells: FormulaCell[], size: (sheet: string) => { rows: number, cols: number }, maxEdges = MAX_EDGES): CycleResult {
  const n = cells.length
  // 每个工作表：列 → 有公式的行（升序）及其点编号
  const index = new Map<string, Map<number, { rows: number[], ids: number[] }>>()
  const order = cells.map((_, i) => i).sort((a, b) => cells[a]!.row - cells[b]!.row)
  for (const i of order) {
    const c = cells[i]!
    let cols = index.get(c.sheet)
    if (!cols)
      index.set(c.sheet, cols = new Map())
    let col = cols.get(c.col)
    if (!col)
      cols.set(c.col, col = { rows: [], ids: [] })
    col.rows.push(c.row)
    col.ids.push(i)
  }

  const adj: number[][] = Array.from({ length: n }, () => [])
  let edges = 0
  let partial = false
  const lowerBound = (rows: number[], x: number) => {
    let lo = 0
    let hi = rows.length
    while (lo < hi) {
      const mid = (lo + hi) >> 1
      if (rows[mid]! < x)
        lo = mid + 1
      else
        hi = mid
    }
    return lo
  }
  /** 加入第 i 个公式的所有边；总边数超过上限时返回 false */
  const addEdges = (i: number) => {
    const cell = cells[i]!
    const seen = new Set<number>()
    for (const r of parseRefs(cell.formula)) {
      const sheet = r.sheet ?? cell.sheet
      const cols = index.get(sheet)
      if (!cols)
        continue
      const sz = size(sheet)
      const c1 = r.c1 ?? 0
      const c2 = Math.min(r.c2 ?? sz.cols - 1, sz.cols - 1)
      const r1 = r.r1 ?? 0
      const r2 = r.r2 ?? sz.rows - 1
      for (let c = c1; c <= c2; c++) {
        const col = cols.get(c)
        if (!col)
          continue
        for (let k = lowerBound(col.rows, r1); k < col.rows.length && col.rows[k]! <= r2; k++) {
          const j = col.ids[k]!
          if (seen.has(j))
            continue
          seen.add(j)
          adj[i]!.push(j)
          if (++edges > maxEdges)
            return false
        }
      }
    }
    return true
  }
  for (let i = 0; i < n; i++) {
    if (!addEdges(i)) {
      partial = true
      break
    }
  }

  // Tarjan（循环实现）
  const idx = Array.from({ length: n }, () => -1)
  const low = Array.from({ length: n }, () => 0)
  const onStack = Array.from({ length: n }, () => false)
  const stack: number[] = []
  const sccs: number[][] = []
  let counter = 0
  for (let s = 0; s < n; s++) {
    if (idx[s] !== -1)
      continue
    const work: { v: number, next: number }[] = [{ v: s, next: 0 }]
    idx[s] = low[s] = counter++
    stack.push(s)
    onStack[s] = true
    while (work.length) {
      const top = work[work.length - 1]!
      const edgesOf = adj[top.v]!
      if (top.next < edgesOf.length) {
        const w = edgesOf[top.next++]!
        if (idx[w] === -1) {
          idx[w] = low[w] = counter++
          stack.push(w)
          onStack[w] = true
          work.push({ v: w, next: 0 })
        }
        else if (onStack[w]) {
          low[top.v] = Math.min(low[top.v]!, idx[w]!)
        }
        continue
      }
      work.pop()
      const parent = work[work.length - 1]
      if (parent)
        low[parent.v] = Math.min(low[parent.v]!, low[top.v]!)
      if (low[top.v] === idx[top.v]) {
        const comp: number[] = []
        let w: number
        do {
          w = stack.pop()!
          onStack[w] = false
          comp.push(w)
        } while (w !== top.v)
        if (comp.length > 1 || adj[top.v]!.includes(top.v))
          sccs.push(comp)
      }
    }
  }

  // 工作表按在 cells 里第一次出现的先后排（即页签顺序），同一张表里先行后列
  const rank = new Map<string, number>()
  for (const c of cells) {
    if (!rank.has(c.sheet))
      rank.set(c.sheet, rank.size)
  }
  const cmp = (a: CellRef, b: CellRef) => rank.get(a.sheet)! - rank.get(b.sheet)! || a.row - b.row || a.col - b.col
  const cycles = sccs
    .map(comp => cycleIn(comp, adj, cells, cmp))
    .sort((a, b) => b.size - a.size || cmp(a.path[0]!, b.path[0]!))
    .slice(0, MAX_CYCLES)
  return { cycles, partial }
}

/** 一个强连通分量里的一条环：从左上角的格子出发，按引用走（广度优先）回到它的最短路径 */
function cycleIn(comp: number[], adj: number[][], cells: FormulaCell[], cmp: (a: CellRef, b: CellRef) => number): Cycle {
  const inComp = new Set(comp)
  const start = comp.reduce((best, i) => (cmp(cells[i]!, cells[best]!) < 0 ? i : best))
  const prev = new Map<number, number>()
  const queue = [start]
  let end = -1
  for (let h = 0; h < queue.length && end < 0; h++) {
    const v = queue[h]!
    for (const w of adj[v]!) {
      if (!inComp.has(w))
        continue
      if (w === start) {
        end = v
        break
      }
      if (!prev.has(w)) {
        prev.set(w, v)
        queue.push(w)
      }
    }
  }
  const ids: number[] = []
  for (let v = end; v !== undefined && ids.length <= comp.length; v = prev.get(v) ?? -1) {
    ids.push(v)
    if (v === start)
      break
  }
  ids.reverse()
  ids.push(start)
  const refs = ids.map(i => ({ sheet: cells[i]!.sheet, row: cells[i]!.row, col: cells[i]!.col }))
  const truncated = refs.length > MAX_PATH
  return { path: truncated ? refs.slice(0, MAX_PATH) : refs, size: comp.length, truncated }
}
