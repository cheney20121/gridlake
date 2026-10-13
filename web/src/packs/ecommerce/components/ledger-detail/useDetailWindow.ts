/**
 * 明细虚拟滚动的窗口取数
 *
 * - rows 长度 = 总行数，未加载的位置是占位行；表格只渲染可视区域，滚动到哪里就按块（chunkSize 行）向后端取数
 * - 已加载的块按 LRU 缓存，超过 maxChunks 时淘汰最久未访问的块（还原为占位行），内存占用与总行数无关
 * - 每次筛选/排序变化递增 version，并取消进行中的请求：迟到的旧响应不会写入新结果
 * - 写入后必须替换 rows 的数组引用：el-table-v2 按引用比较 data 属性，原地修改 + triggerRef 不会触发表格重绘
 */

import type { LedgerDetailFilter, LedgerDetailSort } from '@/packs/ecommerce/api'
import { isCancel } from 'axios'
import { ref, shallowRef } from 'vue'
import { queryLedgerRowsApi } from '@/packs/ecommerce/api'

export interface DetailRow {
  __index: number
  __loaded: boolean
  [column: string]: unknown
}

export function useDetailWindow(chunkSize = 200, maxChunks = 60) {
  const rows = shallowRef<DetailRow[]>([])
  const total = ref(0)
  const pending = ref(0)

  let version = 0
  let query: { filter: LedgerDetailFilter, sort?: LedgerDetailSort } | null = null
  /** 已加载块，Map 的插入顺序即 LRU 顺序 */
  const loaded = new Map<number, true>()
  const inflight = new Map<number, AbortController>()

  const placeholder = (i: number): DetailRow => ({ __index: i, __loaded: false })

  function reset(filter: LedgerDetailFilter, sort: LedgerDetailSort | undefined, totalCount: number) {
    version++
    inflight.forEach(c => c.abort())
    inflight.clear()
    loaded.clear()
    pending.value = 0
    query = { filter, sort }
    total.value = totalCount
    rows.value = Array.from({ length: totalCount }, (_, i) => placeholder(i))
  }

  function touch(chunk: number) {
    loaded.delete(chunk)
    loaded.set(chunk, true)
  }

  function evict(arr: DetailRow[]) {
    while (loaded.size > maxChunks) {
      const oldest = loaded.keys().next().value as number
      loaded.delete(oldest)
      const end = Math.min((oldest + 1) * chunkSize, arr.length)
      for (let i = oldest * chunkSize; i < end; i++)
        arr[i] = placeholder(i)
    }
  }

  async function loadChunk(chunk: number) {
    if (!query)
      return
    if (loaded.has(chunk)) {
      touch(chunk)
      return
    }
    if (inflight.has(chunk))
      return

    const v = version
    const controller = new AbortController()
    inflight.set(chunk, controller)
    pending.value++
    try {
      const res = await queryLedgerRowsApi(query.filter, chunk * chunkSize, chunkSize, query.sort, controller.signal)
      if (v !== version)
        return
      // 复制一份再写入（只复制引用，18 万行约 1ms），保证 data 引用变化
      const arr = rows.value.slice()
      res.rows.forEach((values, j) => {
        const index = chunk * chunkSize + j
        if (index >= arr.length)
          return
        const row: DetailRow = { __index: index, __loaded: true }
        res.columns.forEach((name, k) => {
          row[name] = values[k]
        })
        arr[index] = row
      })
      touch(chunk)
      evict(arr)
      rows.value = arr
    }
    catch (e) {
      if (!isCancel(e))
        console.warn('[ledger] 明细分段加载失败:', e)
    }
    finally {
      if (v === version) {
        inflight.delete(chunk)
        pending.value--
      }
    }
  }

  /** 确保 [start, end] 行（含前后缓冲）已加载 */
  function ensureRange(start: number, end: number) {
    if (!total.value)
      return
    const buffer = Math.floor(chunkSize / 2)
    const first = Math.max(0, Math.floor((start - buffer) / chunkSize))
    const last = Math.min(Math.floor((total.value - 1) / chunkSize), Math.floor((end + buffer) / chunkSize))
    for (let c = first; c <= last; c++)
      void loadChunk(c)
  }

  return { rows, total, pending, reset, ensureRange }
}
