/**
 * 分类汇总的行号换算：数据按分组列排好序（每组的行连在一起），每组最后一行后面插一行小计；
 * 折叠的组只显示小计行。「显示行」= 展开的组的数据行 + 每组一行小计。
 * 只依赖每组的行数和是否折叠，分组再多也是二分查找
 */
export type VirtualRow = { kind: 'data', row: number, group: number } | { kind: 'sub', group: number }

export class GroupMap {
  /** 第 g 组第一行的数据行号（前缀和），最后多一个 = 总行数 */
  private readonly starts: number[]
  /** 第 g 组第一行的显示行号，最后多一个 = 显示行总数 */
  private readonly vStarts: number[]
  readonly sizes: number[]
  private readonly collapsed: boolean[]

  constructor(sizes: number[], collapsed: boolean[] = []) {
    this.sizes = sizes
    this.collapsed = collapsed
    this.starts = [0]
    this.vStarts = [0]
    sizes.forEach((n, g) => {
      this.starts.push(this.starts[g]! + n)
      this.vStarts.push(this.vStarts[g]! + this.shown(g) + 1)
    })
  }

  /** 第 g 组显示出来的数据行数（折叠时为 0） */
  private shown(g: number): number {
    return this.collapsed[g] ? 0 : this.sizes[g]!
  }

  isCollapsed(g: number): boolean {
    return !!this.collapsed[g]
  }

  get dataTotal(): number {
    return this.starts[this.starts.length - 1]!
  }

  /** 显示行总数 */
  get virtualTotal(): number {
    return this.vStarts[this.vStarts.length - 1]!
  }

  /** 第 g 组小计行的显示行号 */
  subRow(g: number): number {
    return this.vStarts[g]! + this.shown(g)
  }

  /** 最后一个 arr[g] ≤ x 的 g（arr 为前缀和，只看前 sizes.length 个） */
  private find(arr: number[], x: number): number {
    let lo = 0
    let hi = this.sizes.length - 1
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1
      if (arr[mid]! <= x)
        lo = mid
      else
        hi = mid - 1
    }
    return lo
  }

  /** 显示行 → 数据行或小计 */
  at(v: number): VirtualRow {
    const g = this.find(this.vStarts, v)
    const offset = v - this.vStarts[g]!
    return offset >= this.shown(g) ? { kind: 'sub', group: g } : { kind: 'data', row: this.starts[g]! + offset, group: g }
  }

  /** 数据行 → 显示行（所在的组折叠了时是它的小计行） */
  toVirtual(row: number): number {
    const g = this.find(this.starts, row)
    return this.collapsed[g] ? this.subRow(g) : this.vStarts[g]! + row - this.starts[g]!
  }

  /**
   * 显示行区间 [v1, v2] 代表的数据行区间（统计、复制用）：展开的组的小计行不代表数据，
   * 折叠的组的小计行代表整组。只有展开组的小计行时为 null
   */
  dataRange(v1: number, v2: number): [number, number] | null {
    const first = this.at(v1)
    const last = this.at(v2)
    const d1 = first.kind === 'data' ? first.row : this.collapsed[first.group] ? this.starts[first.group]! : this.starts[first.group + 1]!
    const d2 = last.kind === 'data' ? last.row : this.starts[last.group + 1]! - 1
    return d1 <= d2 ? [d1, d2] : null
  }

  /** 显示行区间 [v1, v2] 里真正显示出来的数据行（取数用；折叠的组不取） */
  dataSpans(v1: number, v2: number): [number, number][] {
    if (v2 < v1 || !this.sizes.length)
      return []
    const out: [number, number][] = []
    const g1 = this.find(this.vStarts, v1)
    const g2 = this.find(this.vStarts, v2)
    for (let g = g1; g <= g2; g++) {
      const n = this.shown(g)
      if (!n)
        continue
      const a = Math.max(v1, this.vStarts[g]!) - this.vStarts[g]!
      const b = Math.min(v2, this.vStarts[g]! + n - 1) - this.vStarts[g]!
      if (a > b)
        continue
      const span: [number, number] = [this.starts[g]! + a, this.starts[g]! + b]
      const prev = out[out.length - 1]
      if (prev && prev[1] + 1 === span[0])
        prev[1] = span[1]
      else
        out.push(span)
    }
    return out
  }
}
