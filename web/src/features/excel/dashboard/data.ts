import type { KpiSpec, TableCardSpec } from './types'
import type { DatasetFilter, PivotResult } from '@/api/dataset'
import { getPivotApi } from '@/api/dataset'
import { datasetById } from './slicer'

/** 指标卡数据 */
export interface KpiData {
  value: number | null
  /** 同比 / 环比增长率（小数） */
  delta: number | null
  /** 如「较上月」「较上年同期」 */
  deltaLabel: string
  /** 指标对应的时间（未按日期筛选时为最近一期，如 2026-10） */
  period: string | null
  money: boolean
  /** 趋势：最近 12 期 */
  spark: { keys: string[], values: (number | null)[], current: string[] } | null
}

function sameColumn(f: DatasetFilter, column: string, datasetId: string) {
  if (f.column === column)
    return true
  const ds = datasetById(datasetId)
  const col = ds?.columns.find(c => c.key === column || c.title === column)
  return !!col && (f.column === col.key || f.column === col.title)
}

function deltaLabel(spec: KpiSpec) {
  if (spec.compare === 'yoy')
    return '较上年同期'
  return spec.granularity === 'day' ? '较前一天' : spec.granularity === 'year' ? '较上年' : '较上月'
}

/**
 * 指标卡：
 * - 有日期字段时先按期取趋势；没有按日期筛选（切片器 / 自身筛选）时，指标取最近一期，与上一期 / 上年同期比较
 * - 已按日期筛选时取筛选范围的合计，同比 / 环比由透视引擎计算（基期不受日期筛选限制）
 */
export async function loadKpi(spec: KpiSpec, extra: DatasetFilter[], signal?: AbortSignal): Promise<KpiData> {
  const filters = [...(spec.filters ?? []), ...extra]
  const value = { column: spec.value.column ?? null, agg: spec.value.agg }
  const dateCol = spec.dateColumn || null
  const money = !!value.column && datasetById(spec.datasetId)?.columns.find(c => c.key === value.column || c.title === value.column)?.kind === 'money' && value.agg !== 'count' && value.agg !== 'count_distinct'

  // 趋势（不受日期筛选影响，便于看出当前所处位置）
  let trend: { keys: string[], values: (number | null)[] } | null = null
  if (dateCol) {
    const r = await getPivotApi(spec.datasetId, {
      rows: [{ column: dateCol, granularity: spec.granularity }],
      columns: [],
      values: [value],
      filters: filters.filter(f => !sameColumn(f, dateCol, spec.datasetId)),
      sort: { by: 'key', desc: false },
      subtotals: false,
      grandTotals: false,
    }, signal)
    const rows = r.rows.filter(x => x.kind === 'data' && x.keys[0])
    trend = { keys: rows.map(x => x.keys[0]!), values: rows.map(x => x.cells[0]?.[0] ?? null) }
  }

  const hasDateFilter = !!dateCol && filters.some(f => sameColumn(f, dateCol, spec.datasetId))
  let period: string | null = null
  const scoped = [...filters]
  if (dateCol && !hasDateFilter && spec.compare !== 'none') {
    // 最近一期：取有数据的最后一个时间键
    for (let i = (trend?.keys.length ?? 0) - 1; i >= 0 && !period; i--) {
      if (trend!.values[i] != null)
        period = trend!.keys[i]!
    }
    if (period)
      scoped.push({ op: 'prefix', column: dateCol, value: period })
  }

  let result: PivotResult
  if (dateCol && spec.compare !== 'none') {
    result = await getPivotApi(spec.datasetId, {
      rows: [{ column: dateCol, granularity: spec.granularity }],
      columns: [],
      values: [value, { ...value, show: spec.compare }],
      filters: scoped,
      subtotals: false,
      grandTotals: true,
      baseField: dateCol,
    }, signal)
  }
  else {
    result = await getPivotApi(spec.datasetId, { rows: [], columns: [], values: [value], filters: scoped, grandTotals: true }, signal)
  }
  const total = result.rows.find(r => r.kind === 'total')
  const cell = total?.cells[total.cells.length - 1] ?? []
  // 当前范围内出现的时间键（趋势图上高亮）
  const current = dateCol ? result.rows.filter(r => r.kind === 'data').map(r => r.keys[0]).filter((k): k is string => !!k) : []
  if (dateCol && !period && current.length === 1)
    period = current[0]!

  let spark: KpiData['spark'] = null
  if (spec.sparkline && trend?.keys.length) {
    const lastKey = current.length ? current[current.length - 1]! : trend.keys[trend.keys.length - 1]!
    const end = trend.keys.findIndex(k => k > lastKey)
    const upto = end < 0 ? trend.keys.length : end
    const from = Math.max(0, upto - 12)
    spark = { keys: trend.keys.slice(from, upto), values: trend.values.slice(from, upto), current: current.length ? current : (period ? [period] : []) }
  }

  return {
    value: cell[0] ?? null,
    delta: spec.compare !== 'none' && dateCol ? (cell[1] ?? null) : null,
    deltaLabel: deltaLabel(spec),
    period,
    money,
    spark,
  }
}

/** 表格卡片：按一个字段分组汇总（前 N 项） */
export async function loadTable(spec: TableCardSpec, extra: DatasetFilter[], signal?: AbortSignal): Promise<PivotResult> {
  return getPivotApi(spec.datasetId, {
    rows: [{ column: spec.row.column, granularity: spec.row.granularity ?? null }],
    columns: [],
    values: spec.values,
    filters: [...(spec.filters ?? []), ...extra],
    sort: spec.sort === 'value' ? { by: 'value', valueIndex: 0, desc: true } : { by: 'key', desc: false },
    top: spec.top > 0 ? spec.top : null,
    subtotals: false,
    grandTotals: true,
  }, signal)
}

/** 大数自动用「万」「亿」 */
export function compact(v: number | null | undefined, money = false) {
  if (v == null || Number.isNaN(v))
    return '—'
  const a = Math.abs(v)
  if (a >= 1e8)
    return `${Number((v / 1e8).toFixed(2))}亿`
  if (a >= 1e4)
    return `${Number((v / 1e4).toFixed(2))}万`
  return v.toLocaleString('zh-CN', money ? { minimumFractionDigits: 2, maximumFractionDigits: 2 } : { maximumFractionDigits: 2 })
}

export function full(v: number | null | undefined, money = false) {
  if (v == null || Number.isNaN(v))
    return '—'
  return v.toLocaleString('zh-CN', money ? { minimumFractionDigits: 2, maximumFractionDigits: 2 } : { maximumFractionDigits: 4 })
}
