import type { FUniver } from '@univerjs/core/facade'
import { LEDGER_BASE } from '@/api/ledger-base'
import { listWorkbooksApi } from '@/api/workbook'
import { t } from '@/i18n'
import request from '@/utils/request'

/**
 * 跨工作簿引用：=IMPORTRANGE("工作簿名或 ID", "Sheet1!A1:C10")，区域也可以是对方的命名区域；结果按区域大小溢出到相邻格子。
 * 取的是对方「已保存」的最新版本（与 Excel 外部链接的缓存值一致），对方保存后自动更新（每分钟检查一次版本）。
 *
 * 与数据集函数一样「两遍计算」：第一遍未命中的先返回占位、合并成一次批量请求，取回后再重算一次
 */

type Cell = string | number | boolean | null

interface Result {
  values?: Cell[][]
  error?: string
  workbookId?: string
  version?: number
}

interface RangeResult {
  workbookId: string | null
  workbookName: string | null
  version: number | null
  values: Cell[][]
  error: string | null
}

const PLACEHOLDER = t('加载中…')
const POLL_MS = 60_000

const cache = new Map<string, Result>()
const pending = new Map<string, { workbook: string, range: string }>()
let flushTimer: ReturnType<typeof setTimeout> | null = null
let pollTimer: ReturnType<typeof setInterval> | null = null
let api: FUniver | null = null
/** 当前工作簿（不能引用自己） */
let self: { id: string | null, name: string } = { id: null, name: '' }

function scalar(v: unknown): string {
  let x = v
  while (Array.isArray(x))
    x = x[0]
  return x == null ? '' : String(x).trim()
}

function importRange(...args: unknown[]): Cell | Cell[][] {
  if (args.length !== 2)
    return '#参数个数不正确：IMPORTRANGE("工作簿", "区域")'
  const workbook = scalar(args[0])
  const range = scalar(args[1])
  if (!workbook || !range)
    return '#缺少工作簿或区域'
  if (workbook === self.id || (self.name && workbook === self.name))
    return '#引用的是当前工作簿，请直接写单元格地址'
  const key = JSON.stringify([workbook, range])
  const hit = cache.get(key)
  if (hit)
    return hit.error ?? (hit.values?.length ? hit.values.map(r => r.map(v => v ?? '')) : '')
  pending.set(key, { workbook, range })
  if (flushTimer)
    clearTimeout(flushTimer)
  flushTimer = setTimeout(flush, 30)
  return PLACEHOLDER
}

async function flush() {
  flushTimer = null
  const entries = [...pending.entries()]
  pending.clear()
  if (!entries.length)
    return
  try {
    for (let i = 0; i < entries.length; i += 200) {
      const chunk = entries.slice(i, i + 200)
      const res = await request<RangeResult[]>({
        url: `${LEDGER_BASE}/api/ledger/workbooks/ranges`,
        method: 'post',
        data: { items: chunk.map(([, v]) => v) },
      })
      chunk.forEach(([key], j) => {
        const r = res[j]
        cache.set(key, r?.error || !r
          ? { error: `#${r?.error ?? '取数失败'}` }
          : { values: r.values, workbookId: r.workbookId ?? undefined, version: r.version ?? undefined })
      })
    }
  }
  catch (e) {
    console.warn('[IMPORTRANGE] 取数失败:', e)
    entries.forEach(([key]) => cache.set(key, { error: t('#服务不可用') }))
  }
  ensurePolling()
  api?.getFormula().executeCalculation()
}

/** 被引用的工作簿保存了新版本：清掉它的缓存并重算 */
async function checkVersions() {
  const used = [...cache.values()].filter(r => r.workbookId)
  if (!used.length || !api)
    return
  const list = await listWorkbooksApi().catch(() => null)
  if (!list)
    return
  const version = new Map(list.map(w => [w.id, w.version]))
  let changed = false
  for (const [key, r] of cache) {
    if (r.workbookId && version.get(r.workbookId) !== r.version) {
      cache.delete(key)
      changed = true
    }
  }
  if (changed)
    api.getFormula().executeCalculation()
}

function ensurePolling() {
  pollTimer ??= setInterval(checkVersions, POLL_MS)
}

/** 立即重新取所有跨工作簿引用（「刷新跨工作簿引用」命令、打开工作簿后） */
export function refreshImportRanges(recalculate = true) {
  cache.clear()
  if (recalculate)
    api?.getFormula().executeCalculation()
}

/** 打开工作簿时告知当前工作簿（不能引用自己），并清空缓存 */
export function setImportRangeContext(workbook: { id: string | null, name: string }) {
  self = workbook
  cache.clear()
}

/** 已取回的跨工作簿引用（诊断面板用） */
export function importRangeStats() {
  const list = [...cache.entries()].map(([key, r]) => {
    const [workbook, range] = JSON.parse(key) as [string, string]
    return { workbook, range, error: r.error ?? null, cells: r.values ? r.values.length * (r.values[0]?.length ?? 0) : 0 }
  })
  return list
}

export function registerImportRange(univerAPI: FUniver) {
  if (api === univerAPI)
    return
  api = univerAPI
  univerAPI.getFormula().registerFunction(
    'IMPORTRANGE',
    (...args) => importRange(...args),
    '引用其他工作簿的区域（取对方已保存的最新版本，结果溢出到相邻格子）：IMPORTRANGE("工作簿名或 ID", "Sheet1!A1:C10" 或命名区域)',
  )
}
