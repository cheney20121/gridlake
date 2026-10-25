/**
 * 分类汇总的行号换算：数据按分组列排好序（每组的行连在一起），每组最后一行后面插一行小计。
 * 「显示行」= 数据行 + 小计行；第 g 组的数据行在显示行 vStart[g] .. vStart[g] + size - 1，小计在 vStart[g] + size。
 * 只依赖每组的行数，分组再多也是二分查找
 */
export type VirtualRow = { kind: 'data', row: number, group: number } | { kind: 'sub', group: number }

export class GroupMap {
  /** 第 g 组第一行的数据行号（前缀和），最后多一个 = 总行数 */
  private readonly starts: number[]
  readonly sizes: number[]

  constructor(sizes: number[]) {
    this.sizes = sizes
    this.starts = [0]
    for (const n of sizes)
      this.starts.push(this.starts[this.starts.length - 1]! + n)
  }

  get dataTotal(): number {
    return this.starts[this.starts.length - 1]!
  }

  /** 显示行总数：数据行 + 每组一行小计 */
  get virtualTotal(): number {
    return this.dataTotal + this.sizes.length
  }

  /** 第 g 组第一行的显示行号 */
  private vStart(g: number): number {
    return this.starts[g]! + g
  }

  /** 第 g 组小计行的显示行号 */
  subRow(g: number): number {
    return this.vStart(g) + this.sizes[g]!
  }

  /** 显示行 → 数据行或小计 */
  at(v: number): VirtualRow {
    // 最后一个 vStart(g) ≤ v 的组
    let lo = 0
    let hi = this.sizes.length - 1
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1
      if (this.vStart(mid) <= v)
        lo = mid
      else
        hi = mid - 1
    }
    const offset = v - this.vStart(lo)
    return offset >= this.sizes[lo]! ? { kind: 'sub', group: lo } : { kind: 'data', row: this.starts[lo]! + offset, group: lo }
  }

  /** 数据行 → 显示行 */
  toVirtual(row: number): number {
    let lo = 0
    let hi = this.sizes.length - 1
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1
      if (this.starts[mid]! <= row)
        lo = mid
      else
        hi = mid - 1
    }
    return row + lo
  }

  /** 显示行区间 [v1, v2] 里的数据行区间（数据行在显示顺序里是连续的）；只有小计行时为 null */
  dataRange(v1: number, v2: number): [number, number] | null {
    const first = this.at(v1)
    const last = this.at(v2)
    const d1 = first.kind === 'data' ? first.row : this.starts[first.group + 1]!
    const d2 = last.kind === 'data' ? last.row : this.starts[last.group + 1]! - 1
    return d1 <= d2 ? [d1, d2] : null
  }
}
