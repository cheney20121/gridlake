import type { CellValue } from '@/api/dataset'
import { getRowsApi } from '@/api/dataset'

const CHUNK = 200
/** 约 3 万行常驻内存，足够来回滚动 */
const MAX_CHUNKS = 150
const MAX_IN_FLIGHT = 4

/**
 * 按「段」（200 行）缓存的行数据：只请求可视区域需要的段，LRU 淘汰；
 * 换游标（筛选 / 排序变化）时整体作废，迟到的旧响应直接丢弃
 */
export class RowCache {
  private chunks = new Map<number, CellValue[][]>()
  private loading = new Set<number>()
  private wanted: number[] = []
  private generation = 0
  private cursorId = ''
  total = 0

  constructor(
    private datasetId: string,
    private onLoaded: () => void,
    private onExpired: () => void,
    /** 只取这些列（工作表里的数据表）；省略为全部列 */
    private fields?: string[],
  ) {}

  reset(cursorId: string, total: number) {
    this.generation++
    this.cursorId = cursorId
    this.total = total
    this.chunks.clear()
    this.loading.clear()
    this.wanted = []
  }

  get(row: number): CellValue[] | undefined {
    const chunk = this.chunks.get(Math.floor(row / CHUNK))
    if (!chunk)
      return undefined
    // 命中即刷新 LRU 顺序
    const idx = Math.floor(row / CHUNK)
    this.chunks.delete(idx)
    this.chunks.set(idx, chunk)
    return chunk[row % CHUNK]
  }

  /** 声明当前需要的行区间；只保留最新视口的待取段，快速滚动时不会排长队 */
  ensure(from: number, to: number) {
    if (!this.cursorId || this.total === 0)
      return
    const first = Math.floor(Math.max(0, from) / CHUNK)
    const last = Math.floor(Math.min(this.total - 1, to) / CHUNK)
    this.wanted = []
    for (let i = first; i <= last; i++) {
      if (!this.chunks.has(i) && !this.loading.has(i))
        this.wanted.push(i)
    }
    this.pump()
  }

  private pump() {
    while (this.loading.size < MAX_IN_FLIGHT && this.wanted.length) {
      const idx = this.wanted.shift()!
      this.load(idx)
    }
  }

  private async load(idx: number) {
    const gen = this.generation
    this.loading.add(idx)
    try {
      const res = await getRowsApi(this.datasetId, this.cursorId, idx * CHUNK, CHUNK, undefined, this.fields)
      if (gen !== this.generation)
        return
      if (res.expired) {
        this.onExpired()
        return
      }
      this.chunks.set(idx, res.rows)
      while (this.chunks.size > MAX_CHUNKS)
        this.chunks.delete(this.chunks.keys().next().value!)
      this.onLoaded()
    }
    catch (e) {
      console.warn('[big-grid] 取数失败:', e)
    }
    finally {
      if (gen === this.generation) {
        this.loading.delete(idx)
        this.pump()
      }
    }
  }

  /** 取一段完整数据（复制选区用），不经过缓存淘汰逻辑 */
  async fetchRange(from: number, to: number): Promise<CellValue[][]> {
    const out: CellValue[][] = []
    for (let start = from; start <= to; start += 2000) {
      const limit = Math.min(2000, to - start + 1)
      const res = await getRowsApi(this.datasetId, this.cursorId, start, limit, undefined, this.fields)
      if (res.expired)
        throw new Error('数据已更新，请重新选择后复制')
      out.push(...res.rows)
    }
    return out
  }
}
