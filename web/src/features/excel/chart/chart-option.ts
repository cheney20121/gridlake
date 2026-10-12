import type { EChartsOption, SeriesOption } from 'echarts'
import type { ChartData, ChartSpec } from './types'
import { toNumber, toSlices } from './chart-data'

/**
 * ChartSpec + ChartData → ECharts 配置
 *
 * 规范（docs/rowzero-benchmark.md R1 与可视化规范）：
 * - 分类色板 8 色按固定顺序分配，颜色跟随系列名（不随排名 / 筛选变化），「其他」为中性灰
 * - 柱宽 ≤ 24px、数据端 4px 圆角、基线端直角；线宽 2px；标记点 ≥ 8px 带 2px 白环；面积 10% 透明度
 * - 网格线、坐标轴为实线细线且弱化；文字一律用中性墨色，不用系列色
 * - 单系列不显示图例；数据标签有节制（关键点：末端与最大值）
 * - 只有一个数值轴
 */

/** 分类色板（已用校验脚本验证：相邻色在色盲模拟下 ΔE ≥ 9.1，正常视觉 ≥ 19.6） */
export const PALETTE = ['#2a78d6', '#eb6834', '#1baf7a', '#eda100', '#e87ba4', '#008300', '#4a3aa7', '#e34948']
const OTHER_COLOR = '#b5b3ab'
const INK = { primary: '#0b0b0b', secondary: '#52514e', muted: '#898781', grid: '#e1e0d9', axis: '#c3c2b7', surface: '#ffffff' }
/** 瀑布图：增加 / 减少为冷暖两极，合计为中性 */
const WATERFALL = { up: '#2a78d6', down: '#e34948', total: '#52514e' }
const FONT = 'system-ui, -apple-system, "PingFang SC", "Microsoft YaHei", "Segoe UI", sans-serif'

export function seriesColor(name: string, index: number) {
  return name === '其他' ? OTHER_COLOR : PALETTE[index % PALETTE.length]!
}

// ── 数字格式 ──

function fmtFull(v: number | null | undefined, money: boolean, unit: ChartSpec['unit']) {
  if (v == null || Number.isNaN(v))
    return '—'
  if (unit === 'percent')
    return `${(v * 100).toLocaleString('zh-CN', { maximumFractionDigits: 2 })}%`
  return v.toLocaleString('zh-CN', money ? { minimumFractionDigits: 2, maximumFractionDigits: 2 } : { maximumFractionDigits: 4 })
}

/** 轴刻度 / 标签：大数自动用「万」「亿」 */
function fmtShort(v: number, unit: ChartSpec['unit']) {
  if (unit === 'percent')
    return `${Number((v * 100).toFixed(2))}%`
  const a = Math.abs(v)
  if (unit !== 'raw') {
    if (a >= 1e8)
      return `${Number((v / 1e8).toFixed(2))}亿`
    if (a >= 1e4)
      return `${Number((v / 1e4).toFixed(2))}万`
  }
  return Number(v.toFixed(2)).toLocaleString('zh-CN')
}

// ── 公共部件 ──

function baseText() {
  return { fontFamily: FONT, color: INK.secondary, fontSize: 12 }
}

function valueAxis(unit: ChartSpec['unit']) {
  return {
    type: 'value' as const,
    axisLine: { show: false },
    axisTick: { show: false },
    splitLine: { lineStyle: { color: INK.grid, width: 1, type: 'solid' as const } },
    axisLabel: { color: INK.muted, fontFamily: FONT, formatter: (v: number) => fmtShort(v, unit) },
  }
}

function categoryAxis(categories: string[]) {
  return {
    type: 'category' as const,
    data: categories,
    axisLine: { lineStyle: { color: INK.axis, width: 1 } },
    axisTick: { show: false },
    axisLabel: { color: INK.muted, fontFamily: FONT, hideOverlap: true },
  }
}

function legend(spec: ChartSpec, count: number): EChartsOption['legend'] {
  if (count < 2)
    return { show: false }
  const pos = spec.legend ?? 'top'
  return {
    show: true,
    type: 'scroll',
    icon: 'roundRect',
    itemWidth: 10,
    itemHeight: 10,
    textStyle: { color: INK.secondary, fontFamily: FONT },
    ...(pos === 'right' ? { orient: 'vertical', right: 8, top: 'middle' } : pos === 'bottom' ? { bottom: 4 } : { top: spec.title ? 26 : 4 }),
  }
}

function grid(spec: ChartSpec, count: number, horizontal = false) {
  const top = (spec.title ? 30 : 10) + (count >= 2 && (spec.legend ?? 'top') === 'top' ? 26 : 0)
  return {
    top,
    bottom: count >= 2 && spec.legend === 'bottom' ? 36 : 12,
    left: 12,
    right: count >= 2 && spec.legend === 'right' ? 120 : horizontal ? 40 : 20,
    containLabel: true,
  }
}

function title(spec: ChartSpec): EChartsOption['title'] {
  return spec.title
    ? { text: spec.title, left: 8, top: 4, textStyle: { color: INK.primary, fontFamily: FONT, fontSize: 14, fontWeight: 600 } }
    : { show: false }
}

/** 关键点：最后一个与最大的一个（按绝对值） */
function keyIndexes(values: (number | null)[]) {
  const idx = new Set<number>()
  let last = -1
  let maxI = -1
  values.forEach((v, i) => {
    if (v == null)
      return
    last = i
    if (maxI < 0 || Math.abs(v) > Math.abs(values[maxI] ?? 0))
      maxI = i
  })
  if (last >= 0)
    idx.add(last)
  if (maxI >= 0)
    idx.add(maxI)
  return idx
}

/** 标签模式：全部标注只在点数少时生效，否则降级为关键点 */
function effectiveLabels(spec: ChartSpec, data: ChartData) {
  const mode = spec.labels ?? (spec.type === 'waterfall' && data.categories.length <= 12 ? 'all' : 'none')
  if (mode === 'all' && data.categories.length * Math.max(1, data.series.length) > 36)
    return 'key'
  return mode
}

function tooltipFormatter(money: boolean, unit: ChartSpec['unit']) {
  return (params: unknown) => {
    const list = (Array.isArray(params) ? params : [params]) as { marker: string, seriesName: string, name: string, value: unknown, data: unknown }[]
    const head = list[0]?.name ?? ''
    const lines = list
      .filter(p => p.seriesName !== '__base')
      .map((p) => {
        const raw = Array.isArray(p.value) ? p.value[1] : p.value
        return `${p.marker}<span style="color:${INK.secondary}">${p.seriesName}</span>&emsp;<b style="color:${INK.primary}">${fmtFull(toNumber(raw), money, unit)}</b>`
      })
    return [`<div style="color:${INK.primary};font-weight:600;margin-bottom:2px">${head}</div>`, ...lines].join('<br>')
  }
}

// ── 各图表类型 ──

function barRadius(horizontal: boolean, negative: boolean): number[] {
  // 数据端圆角、基线端直角
  if (horizontal)
    return negative ? [4, 0, 0, 4] : [0, 4, 4, 0]
  return negative ? [0, 0, 4, 4] : [4, 4, 0, 0]
}

function cartesian(spec: ChartSpec, data: ChartData): EChartsOption {
  const money = !!data.money
  const unit = spec.unit ?? 'auto'
  const horizontal = spec.type === 'bar'
  const labelMode = effectiveLabels(spec, data)
  const stacked = !!spec.stacked && ['column', 'bar', 'area'].includes(spec.type)
  const isLineLike = spec.type === 'line' || spec.type === 'area'
  const many = data.categories.length > 24

  const series: SeriesOption[] = data.series.map((s, si) => {
    const color = seriesColor(s.name, si)
    const kind: 'bar' | 'line' = spec.type === 'combo'
      ? (spec.seriesTypes?.[s.name] ?? (si === 0 ? 'bar' : 'line'))
      : isLineLike ? 'line' : 'bar'
    const keys = keyIndexes(s.values)
    const showLabel = (i: number) => labelMode === 'all' || (labelMode === 'key' && keys.has(i))
    const label = {
      show: labelMode !== 'none',
      color: INK.secondary,
      fontFamily: FONT,
      fontSize: 11,
      position: kind === 'bar' ? (horizontal ? 'right' : 'top') : 'top',
      formatter: (p: { dataIndex: number, value: unknown }) => (showLabel(p.dataIndex) ? fmtShort(toNumber(p.value) ?? 0, unit) : ''),
    }
    if (kind === 'bar') {
      return {
        type: 'bar',
        name: s.name,
        stack: stacked ? 'total' : undefined,
        barMaxWidth: 24,
        barGap: '12%',
        data: s.values.map(v => ({
          value: v,
          // 堆叠时各段之间用 2px 白色间隙分隔，不加圆角（只有最外段才是数据端）
          itemStyle: stacked ? { borderRadius: 0 } : { borderRadius: barRadius(horizontal, (v ?? 0) < 0) },
        })),
        itemStyle: { color, ...(stacked ? { borderColor: INK.surface, borderWidth: 1 } : {}) },
        emphasis: { focus: 'series' },
        label: stacked ? { show: false } : label,
      } as SeriesOption
    }
    return {
      type: 'line',
      name: s.name,
      stack: stacked ? 'total' : undefined,
      data: s.values,
      connectNulls: false,
      lineStyle: { width: 2, color, cap: 'round', join: 'round' },
      itemStyle: { color, borderColor: INK.surface, borderWidth: 2 },
      symbol: 'circle',
      symbolSize: 8,
      showSymbol: !many,
      areaStyle: spec.type === 'area' ? { color, opacity: 0.1 } : undefined,
      emphasis: { focus: 'series', scale: 1.4 },
      label,
    } as SeriesOption
  })

  const cat = categoryAxis(data.categories)
  const val = valueAxis(unit)
  return {
    textStyle: baseText(),
    title: title(spec),
    legend: legend(spec, data.series.length),
    grid: grid(spec, data.series.length, horizontal),
    tooltip: {
      trigger: isLineLike ? 'axis' : 'item',
      axisPointer: { type: 'line', lineStyle: { color: INK.axis, width: 1 } },
      confine: true,
      formatter: tooltipFormatter(money, unit),
    },
    xAxis: horizontal ? val : cat,
    yAxis: horizontal ? { ...cat, inverse: true } : val,
    series,
  }
}

function pie(spec: ChartSpec, data: ChartData): EChartsOption & { notes: string[] } {
  const { slices, notes } = toSlices(data)
  // 颜色按分类在原始数据中的顺序分配，不随大小排名变化
  const order = new Map(data.categories.map((c, i) => [c, i]))
  const money = !!data.money
  const unit = spec.unit ?? 'auto'
  const total = slices.reduce((a, s) => a + s.value, 0)
  return {
    notes,
    textStyle: baseText(),
    title: title(spec),
    legend: { ...(legend(spec, slices.length) as object), show: slices.length >= 2 },
    tooltip: {
      trigger: 'item',
      confine: true,
      formatter: (p: unknown) => {
        const x = p as { marker: string, name: string, value: number }
        return `${x.marker}<span style="color:${INK.secondary}">${x.name}</span>&emsp;<b style="color:${INK.primary}">${fmtFull(x.value, money, unit)}</b>&emsp;<span style="color:${INK.muted}">${total ? ((x.value / total) * 100).toFixed(1) : 0}%</span>`
      },
    },
    series: [{
      type: 'pie',
      // 半径留出外侧标签的空间：小尺寸时标签不压到图例、提示
      radius: spec.type === 'donut' ? ['38%', '58%'] : [0, '58%'],
      center: ['50%', spec.title ? '57%' : '53%'],
      data: slices.map(s => ({ name: s.name, value: s.value, itemStyle: { color: seriesColor(s.name, order.get(s.name) ?? 0) } })),
      itemStyle: { borderColor: INK.surface, borderWidth: 2 },
      label: { color: INK.secondary, fontFamily: FONT, formatter: '{b}  {d}%' },
      labelLine: { lineStyle: { color: INK.axis } },
      emphasis: { scale: true, scaleSize: 4 },
    }],
  }
}

function scatter(spec: ChartSpec, data: ChartData): EChartsOption {
  const unit = spec.unit ?? 'auto'
  const xs = data.categories.map(c => toNumber(c))
  return {
    textStyle: baseText(),
    title: title(spec),
    legend: legend(spec, data.series.length),
    grid: grid(spec, data.series.length),
    tooltip: {
      trigger: 'item',
      confine: true,
      formatter: (p: unknown) => {
        const x = p as { marker: string, seriesName: string, value: [number, number] }
        return `${x.marker}${x.seriesName}<br>X&emsp;<b>${fmtFull(x.value[0], false, 'raw')}</b><br>Y&emsp;<b>${fmtFull(x.value[1], !!data.money, unit)}</b>`
      },
    },
    xAxis: { ...valueAxis('raw'), splitLine: { show: false }, axisLine: { show: true, lineStyle: { color: INK.axis } } },
    yAxis: valueAxis(unit),
    series: data.series.map((s, si) => ({
      type: 'scatter',
      name: s.name,
      symbolSize: 8,
      itemStyle: { color: seriesColor(s.name, si), borderColor: INK.surface, borderWidth: 2 },
      data: s.values.map((v, i) => [xs[i], v]).filter(([x, y]) => x != null && y != null),
    })) as SeriesOption[],
  }
}

function waterfall(spec: ChartSpec, data: ChartData): EChartsOption {
  const s = data.series[0]
  const unit = spec.unit ?? 'auto'
  const money = !!data.money
  const cats = [...data.categories]
  const deltas = (s?.values ?? []).map(v => v ?? 0)
  const kinds: ('up' | 'down' | 'total')[] = deltas.map(v => (v >= 0 ? 'up' : 'down'))
  if (spec.waterfallTotal !== false) {
    cats.push('合计')
    deltas.push(deltas.reduce((a, v) => a + v, 0))
    kinds.push('total')
  }
  // 透明的底座 + 可见的变化量
  const base: number[] = []
  const shown: number[] = []
  let running = 0
  deltas.forEach((d, i) => {
    if (kinds[i] === 'total') {
      base.push(Math.min(0, d))
      shown.push(Math.abs(d))
      return
    }
    const next = running + d
    base.push(Math.min(running, next))
    shown.push(Math.abs(d))
    running = next
  })
  const labelMode = effectiveLabels(spec, { ...data, categories: cats })
  const color = (i: number) => WATERFALL[kinds[i]!]
  const legendData = [{ name: '增加' }, { name: '减少' }, ...(spec.waterfallTotal !== false ? [{ name: '合计' }] : [])]
  return {
    textStyle: baseText(),
    title: title(spec),
    legend: { ...(legend(spec, 2) as object), data: legendData.map(l => l.name) },
    grid: grid(spec, 2),
    tooltip: {
      trigger: 'item',
      confine: true,
      formatter: (p: unknown) => {
        const x = p as { dataIndex: number, marker: string, name: string }
        const kind = kinds[x.dataIndex]
        return `<div style="font-weight:600">${x.name}</div>${kind === 'total' ? '合计' : kind === 'up' ? '增加' : '减少'}&emsp;<b>${fmtFull(deltas[x.dataIndex], money, unit)}</b>`
      },
    },
    xAxis: categoryAxis(cats),
    yAxis: valueAxis(unit),
    series: [
      // stackStrategy all：底座为负（跨过 0 轴）时也按代数和堆叠，默认的同号堆叠会画错
      { type: 'bar', name: '__base', stack: 'wf', stackStrategy: 'all', data: base, itemStyle: { color: 'transparent' }, emphasis: { disabled: true }, tooltip: { show: false }, barMaxWidth: 24, silent: true },
      {
        type: 'bar',
        name: '变化',
        stack: 'wf',
        stackStrategy: 'all',
        barMaxWidth: 24,
        data: shown.map((v, i) => ({ value: v, itemStyle: { color: color(i), borderRadius: [4, 4, 4, 4] } })),
        label: {
          show: labelMode !== 'none',
          position: 'top',
          color: INK.secondary,
          fontFamily: FONT,
          fontSize: 11,
          formatter: (p: { dataIndex: number }) => {
            if (labelMode === 'key' && kinds[p.dataIndex] !== 'total')
              return ''
            const d = deltas[p.dataIndex] ?? 0
            return `${kinds[p.dataIndex] === 'total' || d < 0 ? '' : '+'}${fmtShort(d, unit)}`
          },
        },
      },
      // 图例占位（不画数据）
      { type: 'bar', name: '增加', data: [], itemStyle: { color: WATERFALL.up } },
      { type: 'bar', name: '减少', data: [], itemStyle: { color: WATERFALL.down } },
      ...(spec.waterfallTotal !== false ? [{ type: 'bar' as const, name: '合计', data: [], itemStyle: { color: WATERFALL.total } }] : []),
    ] as SeriesOption[],
  }
}

export interface BuiltOption {
  option: EChartsOption
  notes: string[]
}

export function buildOption(spec: ChartSpec, data: ChartData): BuiltOption {
  if (!data.categories.length || !data.series.length)
    return { option: { title: { text: '没有数据', left: 'center', top: 'middle', textStyle: { color: INK.muted, fontFamily: FONT, fontSize: 13, fontWeight: 400 } } }, notes: data.notes }
  switch (spec.type) {
    case 'pie':
    case 'donut': {
      const { notes, ...option } = pie(spec, data)
      return { option, notes: [...data.notes, ...notes] }
    }
    case 'scatter':
      return { option: scatter(spec, data), notes: data.notes }
    case 'waterfall':
      return { option: waterfall(spec, data), notes: data.series.length > 1 ? [...data.notes, `瀑布图只使用第一个系列「${data.series[0]!.name}」`] : data.notes }
    case 'histogram':
      return { option: cartesian({ ...spec, type: 'column', stacked: false }, data), notes: data.notes }
    default:
      return { option: cartesian(spec, data), notes: data.notes }
  }
}

/** 数据表视图用的格式化 */
export function formatCell(v: number | null, money: boolean, unit: ChartSpec['unit']) {
  return fmtFull(v, money, unit)
}
