import type { FUniver } from '@univerjs/core/facade'
import type { ChartData, ChartSpec, DatasetSource, RangeSource } from './types'
import type { DatasetFilter } from '@/api/dataset'
import { getPivotApi } from '@/api/dataset'
import { MAX_SCATTER_SERIES, MAX_SERIES, MAX_SLICES } from './types'

/** 单元格值 → 数字（文本数字也认：去掉千分位、货币符号、百分号） */
export function toNumber(v: unknown): number | null {
  if (typeof v === 'number')
    return Number.isFinite(v) ? v : null
  if (typeof v !== 'string')
    return null
  const s = v.trim().replace(/[,，¥￥$\s]/g, '')
  if (!s)
    return null
  const pct = s.endsWith('%')
  const n = Number(pct ? s.slice(0, -1) : s)
  if (!Number.isFinite(n))
    return null
  return pct ? n / 100 : n
}

function label(v: unknown): string {
  if (v == null)
    return ''
  return String(v).trim()
}

/** 单元格区域 → 分类 × 系列 */
export function fromMatrix(values: unknown[][], display: unknown[][], src: Pick<RangeSource, 'headerRow' | 'firstColLabels' | 'seriesIn'>): ChartData {
  // 按行取系列时先转置，之后统一按「列是系列」处理
  let raw = values
  let text = display
  if (src.seriesIn === 'rows') {
    const t = (m: unknown[][]) => (m[0] ?? []).map((_, c) => m.map(r => r[c]))
    raw = t(values)
    text = t(display)
  }
  const rows = raw.length
  const cols = raw[0]?.length ?? 0
  const startRow = src.headerRow ? 1 : 0
  const startCol = src.firstColLabels ? 1 : 0
  const categories: string[] = []
  for (let r = startRow; r < rows; r++)
    categories.push(src.firstColLabels ? label(text[r]?.[0]) : String(r - startRow + 1))
  const series: ChartData['series'] = []
  for (let c = startCol; c < cols; c++) {
    const name = src.headerRow ? label(text[0]?.[c]) || `系列${c - startCol + 1}` : `系列${c - startCol + 1}`
    const vals: (number | null)[] = []
    for (let r = startRow; r < rows; r++)
      vals.push(toNumber(raw[r]?.[c]))
    // 整列都不是数字的（如备注列）不作为系列
    if (vals.some(v => v != null))
      series.push({ name, values: vals })
  }
  // 去掉尾部全空的行（选了整列时常见）
  let last = categories.length
  while (last > 0 && !categories[last - 1] && series.every(s => s.values[last - 1] == null))
    last--
  return {
    categories: categories.slice(0, last),
    series: series.map(s => ({ ...s, values: s.values.slice(0, last) })),
    notes: [],
  }
}

export function readRange(api: FUniver, src: RangeSource): ChartData {
  const wb = api.getActiveWorkbook()
  const sheet = wb?.getSheetBySheetId(src.sheetId)
  if (!sheet)
    throw new Error('数据所在的工作表已删除')
  let range
  try {
    range = sheet.getRange(src.range)
  }
  catch {
    throw new Error(`数据区域无效：${src.range}`)
  }
  const values = range.getValues() as unknown[][]
  const display = (range.getDisplayValues?.() ?? values) as unknown[][]
  const data = fromMatrix(values, display, src)
  if (!data.series.length)
    throw new Error('选中的区域里没有数字')
  return data
}

/** 数据集 → 服务端聚合（复用透视引擎，千万行只返回聚合结果） */
export async function readDataset(src: DatasetSource, signal?: AbortSignal, extraFilters: DatasetFilter[] = []): Promise<ChartData> {
  if (!src.values.length)
    throw new Error('请选择数值字段')
  const split = src.splitBy && src.values.length === 1 ? src.splitBy : null
  const res = await getPivotApi(src.datasetId, {
    rows: [{ column: src.x.column, granularity: src.x.granularity ?? null }],
    columns: split ? [{ column: split }] : [],
    values: src.values,
    filters: [...(src.filters ?? []), ...extraFilters],
    sort: src.sort === 'value' ? { by: 'value', valueIndex: 0, desc: true } : { by: 'key', desc: false },
    top: src.top ?? null,
    subtotals: false,
    grandTotals: false,
  }, signal)
  const rows = res.rows.filter(r => r.kind === 'data')
  const categories = rows.map(r => r.keys[0] ?? '（空）')
  const series: ChartData['series'] = split
    ? res.columnKeys.map((k, ci) => ({ name: k[0] ?? '（空）', values: rows.map(r => r.cells[ci]?.[0] ?? null) }))
    : res.values.map((v, vi) => ({ name: v.label, values: rows.map(r => r.cells[0]?.[vi] ?? null) }))
  return { categories, series, money: res.values.every(v => v.kind === 'money'), notes: [] }
}

/** 系列超过上限：按合计保留最大的 N-1 个，其余合并为「其他」（颜色按系列名固定，不随排名变化） */
export function foldSeries(data: ChartData, max = MAX_SERIES): ChartData {
  if (data.series.length <= max)
    return data
  const total = (s: ChartData['series'][number]) => s.values.reduce<number>((a, v) => a + Math.abs(v ?? 0), 0)
  const ranked = [...data.series].sort((a, b) => total(b) - total(a))
  const keep = new Set(ranked.slice(0, max - 1).map(s => s.name))
  const others = data.series.filter(s => !keep.has(s.name))
  const other = {
    name: '其他',
    values: data.categories.map((_, i) => others.reduce<number | null>((a, s) => (s.values[i] == null ? a : (a ?? 0) + (s.values[i] as number)), null)),
  }
  return {
    ...data,
    series: [...data.series.filter(s => keep.has(s.name)), other],
    notes: [...data.notes, `系列有 ${data.series.length} 个，较小的 ${others.length} 个已合并为「其他」`],
  }
}

/** 饼 / 环形图：只用第一个系列，最多 6 块（其余合并为「其他」），负数与空值不参与 */
export function toSlices(data: ChartData): { slices: { name: string, value: number }[], notes: string[] } {
  const s = data.series[0]
  const notes: string[] = []
  if (data.series.length > 1)
    notes.push(`饼图只显示第一个系列「${s?.name}」`)
  let slices = data.categories.map((name, i) => ({ name, value: s?.values[i] ?? 0 })).filter(x => x.value > 0)
  const skipped = data.categories.length - slices.length
  if (skipped)
    notes.push(`${skipped} 个分类为 0、空或负数，未显示`)
  slices.sort((a, b) => b.value - a.value)
  if (slices.length > MAX_SLICES) {
    const rest = slices.slice(MAX_SLICES - 1)
    slices = [...slices.slice(0, MAX_SLICES - 1), { name: '其他', value: rest.reduce((a, x) => a + x.value, 0) }]
    notes.push(`分类有 ${data.categories.length} 个，较小的 ${rest.length} 个已合并为「其他」；比较很多分类时建议用条形图`)
  }
  return { slices, notes }
}

/** 直方图：取第一个系列的全部数值，等宽分组（默认按 Sturges 规则） */
export function toHistogram(data: ChartData, bins?: number | null): ChartData {
  const values = (data.series[0]?.values ?? []).filter((v): v is number => v != null)
  if (!values.length)
    return { categories: [], series: [], notes: ['没有可分组的数值'] }
  const min = Math.min(...values)
  const max = Math.max(...values)
  const n = Math.max(1, Math.min(50, bins || Math.ceil(Math.log2(values.length) + 1)))
  const width = (max - min) / n || 1
  const counts: number[] = Array.from({ length: n }, () => 0)
  for (const v of values)
    counts[Math.min(n - 1, Math.floor((v - min) / width))]!++
  const fmt = (x: number) => Number(x.toPrecision(4)).toLocaleString('zh-CN')
  return {
    categories: counts.map((_, i) => `${fmt(min + i * width)} ~ ${fmt(min + (i + 1) * width)}`),
    series: [{ name: '个数', values: counts }],
    notes: [`${values.length.toLocaleString()} 个数值，分为 ${n} 组`],
  }
}

/** extraFilters：仪表盘切片器的筛选（只作用于数据集来源） */
export async function resolveChartData(api: FUniver, spec: ChartSpec, signal?: AbortSignal, extraFilters: DatasetFilter[] = []): Promise<ChartData> {
  let data = spec.source.kind === 'range' ? readRange(api, spec.source) : await readDataset(spec.source, signal, extraFilters)
  if (spec.type === 'histogram')
    return toHistogram(data, spec.bins)
  if (spec.type === 'scatter')
    data = foldSeries(data, MAX_SCATTER_SERIES)
  else if (spec.type !== 'pie' && spec.type !== 'donut')
    data = foldSeries(data)
  return data
}
