/**
 * 分类汇总的行号换算：数据按各级分组列依次排好序（每组的行连在一起，内层的组嵌在外层的组里），
 * 每组最后一行后面插一行小计（先内层的小计，再外层的小计）；折叠的组只显示它的小计行。
 * 「显示行」由一段段「块」组成：一段数据行，或一行小计。只依赖每一级每组的行数和是否折叠，分组再多也是二分查找
 */
export type VirtualRow = { kind: 'data', row: number } | { kind: 'sub', level: number, group: number }

export class GroupMap {
  /** 每一级每组的行数（外层在前） */
  readonly levels: number[][]
  /** 各级的行数加起来一样、内层的组恰好落在外层的组里 */
  readonly consistent: boolean
  private readonly collapsed: boolean[][]
  /** 每一级每组第一行的数据行号（前缀和），最后多一个 = 总行数 */
  private readonly starts: number[][]
  /** 块：第一个显示行（最后多一个 = 显示行总数）、数据段的第一个数据行（小计为 -1）、小计的级别和组号 */
  private readonly bV: number[] = [0]
  private readonly bData: number[] = []
  private readonly bLevel: number[] = []
  private readonly bGroup: number[] = []
  /** 最内层每组：显示出来时第一行的显示行号；被折叠（自己或外层）时为 -(显示出来的那一行小计的显示行号) - 1 */
  private readonly leafV: number[]
  /** 每一级每组小计行的显示行号；被外层折叠时为 -1 */
  private readonly subV: number[][]

  constructor(levels: number[][], collapsed: boolean[][] = []) {
    this.levels = levels
    this.collapsed = collapsed
    this.starts = levels.map((sizes) => {
      const s = [0]
      sizes.forEach((n, g) => s.push(s[g]! + n))
      return s
    })
    const depth = levels.length
    const leaves = levels[depth - 1] ?? []
    this.leafV = Array.from({ length: leaves.length }, () => 0)
    this.subV = levels.map(l => Array.from({ length: l.length }, () => -1))
    // 每组的第一个子组（最后多一个）：按行数把内层的组依次分给外层
    let consistent = new Set(this.starts.map(s => s[s.length - 1])).size <= 1
    const firstChild = levels.slice(0, -1).map((sizes, d) => {
      const inner = levels[d + 1]!
      const out: number[] = []
      let j = 0
      for (const size of sizes) {
        out.push(j)
        let rows = 0
        while (rows < size && j < inner.length)
          rows += inner[j++]!
        if (rows !== size)
          consistent = false
      }
      out.push(j)
      return out
    })
    this.consistent = consistent
    if (!consistent)
      return
    // 每组的最内层组区间 [leafFirst[d][g], leafFirst[d][g + 1])
    const leafFirst: number[][] = Array.from({ length: depth })
    leafFirst[depth - 1] = Array.from({ length: leaves.length + 1 }, (_, g) => g)
    for (let d = depth - 2; d >= 0; d--)
      leafFirst[d] = firstChild[d]!.map(c => leafFirst[d + 1]![c]!)

    const push = (v: number, data: number, level: number, group: number) => {
      this.bData.push(data)
      this.bLevel.push(level)
      this.bGroup.push(group)
      this.bV.push(this.bV[this.bV.length - 1]! + v)
    }
    const sub = (d: number, g: number) => {
      this.subV[d]![g] = this.bV[this.bV.length - 1]!
      push(1, -1, d, g)
    }
    const emit = (d: number, g: number) => {
      if (collapsed[d]?.[g]) {
        const v = this.bV[this.bV.length - 1]!
        for (let leaf = leafFirst[d]![g]!; leaf < leafFirst[d]![g + 1]!; leaf++)
          this.leafV[leaf] = -v - 1
        sub(d, g)
        return
      }
      if (d === depth - 1) {
        this.leafV[g] = this.bV[this.bV.length - 1]!
        push(levels[d]![g]!, this.starts[d]![g]!, -1, g)
      }
      else {
        for (let c = firstChild[d]![g]!; c < firstChild[d]![g + 1]!; c++)
          emit(d + 1, c)
      }
      sub(d, g)
    }
    levels[0]?.forEach((_, g) => emit(0, g))
  }

  get depth(): number {
    return this.levels.length
  }

  isCollapsed(level: number, group: number): boolean {
    return !!this.collapsed[level]?.[group]
  }

  get dataTotal(): number {
    const s = this.starts[0]
    return s ? s[s.length - 1]! : 0
  }

  /** 显示行总数 */
  get virtualTotal(): number {
    return this.bV[this.bV.length - 1]!
  }

  /** 第 level 级第 g 组小计行的显示行号；被外层折叠时为 -1 */
  subRow(level: number, group: number): number {
    return this.subV[level]?.[group] ?? -1
  }

  /** 显示行 v 所在的块 */
  private block(v: number): number {
    let lo = 0
    let hi = this.bData.length - 1
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1
      if (this.bV[mid]! <= v)
        lo = mid
      else
        hi = mid - 1
    }
    return lo
  }

  /** 显示行 → 数据行或小计 */
  at(v: number): VirtualRow {
    const b = this.block(v)
    const data = this.bData[b]!
    return data >= 0 ? { kind: 'data', row: data + v - this.bV[b]! } : { kind: 'sub', level: this.bLevel[b]!, group: this.bGroup[b]! }
  }

  /** 数据行 → 显示行（所在的组折叠了时是显示出来的那一行小计） */
  toVirtual(row: number): number {
    const s = this.starts[this.depth - 1]!
    let lo = 0
    let hi = s.length - 2
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1
      if (s[mid]! <= row)
        lo = mid
      else
        hi = mid - 1
    }
    const v = this.leafV[lo]!
    return v >= 0 ? v + row - s[lo]! : -v - 1
  }

  /**
   * 显示行区间 [v1, v2] 代表的数据行区间（统计、复制用）：展开的组的小计行不代表数据，
   * 折叠的组的小计行代表整组。只有展开组的小计行时为 null
   */
  dataRange(v1: number, v2: number): [number, number] | null {
    const first = this.at(v1)
    const last = this.at(v2)
    const d1 = first.kind === 'data' ? first.row : this.starts[first.level]![first.group + (this.isCollapsed(first.level, first.group) ? 0 : 1)]!
    const d2 = last.kind === 'data' ? last.row : this.starts[last.level]![last.group + 1]! - 1
    return d1 <= d2 ? [d1, d2] : null
  }

  /** 显示行区间 [v1, v2] 里真正显示出来的数据行（取数用；折叠的组不取） */
  dataSpans(v1: number, v2: number): [number, number][] {
    if (v2 < v1 || !this.bData.length)
      return []
    const out: [number, number][] = []
    const b2 = this.block(v2)
    for (let b = this.block(v1); b <= b2; b++) {
      const data = this.bData[b]!
      if (data < 0)
        continue
      const a = Math.max(v1, this.bV[b]!) - this.bV[b]!
      const z = Math.min(v2, this.bV[b + 1]! - 1) - this.bV[b]!
      if (a > z)
        continue
      const span: [number, number] = [data + a, data + z]
      const prev = out[out.length - 1]
      if (prev && prev[1] + 1 === span[0])
        prev[1] = span[1]
      else
        out.push(span)
    }
    return out
  }
}
