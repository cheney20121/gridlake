<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { ChartData, ChartSpec } from '@/features/excel/chart/types'
import type { CrossFilter } from '@/features/excel/dashboard/cross-filter'
import { computed, onBeforeUnmount, onMounted, ref, shallowRef } from 'vue'
import BusyInline from '@/components/excel/common/BusyInline.vue'
import { resolveChartData } from '@/features/excel/chart/chart-data'
import { registerChartInstance, unregisterChartInstance } from '@/features/excel/chart/chart-images'
import { chartBus, sheetIdOf } from '@/features/excel/chart/chart-manager'
import { buildOption, formatCell } from '@/features/excel/chart/chart-option'
import { echarts } from '@/features/excel/chart/echarts'
import { clearCrossFilter, crossFilterOf, crossFilters, dashboardFiltersFor, dropCrossFiltersOf, persistCrossFilter, toggleCrossFilter } from '@/features/excel/dashboard/cross-filter'
import { downloadBlob, safeFileName } from '@/features/excel/dashboard/export-image'
import { t } from '@/i18n'
import { errorText } from '@/utils/busy-hint'

/**
 * 浮动在工作表上的图表（挂载在 Univer 浮动对象里的独立 Vue 应用）
 * - 数据变化时自动重算：区域图表监听所在工作表的单元格变化，数据集图表在数据集更新时刷新
 * - 刷新时保留旧图（半透明），不闪白
 * - 「数据」切换为表格视图：所有数值都能看到（不只靠悬停提示）
 * - 数据集图表可点击分类联动同一工作表上的其他看板部件（再点一次取消），选中的分类高亮
 */
const props = defineProps<{
  spec: ChartSpec
  api: FUniver
}>()

const spec = shallowRef<ChartSpec>(props.spec)
const data = shallowRef<ChartData | null>(null)
const notes = ref<string[]>([])
const error = ref('')
const loading = ref(false)
const view = ref<'chart' | 'table'>('chart')
const el = ref<HTMLDivElement | null>(null)
let chart: echarts.ECharts | null = null
let observer: ResizeObserver | null = null
let controller: AbortController | null = null
const offs: (() => void)[] = []

/** 放弃这次计算（等得太久时）；已有图表保留，没有则显示「已取消」与重试 */
function cancel() {
  controller?.abort()
  loading.value = false
  if (!data.value)
    error.value = '已取消'
}

async function load() {
  controller?.abort()
  const c = new AbortController()
  controller = c
  loading.value = true
  error.value = ''
  try {
    // 数据集图表叠加所在工作表上切片器与其他图表联动的筛选
    const src = spec.value.source
    const extra = src.kind === 'dataset' ? dashboardFiltersFor(props.api, sheetIdOf(props.api, spec.value.id), src.datasetId, spec.value.id) : []
    const d = await resolveChartData(props.api, spec.value, c.signal, extra)
    if (c.signal.aborted)
      return
    data.value = d
    error.value = ''
    render()
  }
  catch (e) {
    if (!c.signal.aborted)
      error.value = errorText(e, '图表计算失败')
  }
  finally {
    if (controller === c)
      loading.value = false
  }
}

/** 本图表发起的联动（选中的分类） */
const selection = computed(() => {
  void crossFilters.value
  const f = crossFilterOf(sheetIdOf(props.api, spec.value.id))
  return f && f.chartId === spec.value.id ? f : null
})

/** 能否点击联动：数据集图表，且分类是一个字段（散点、直方图没有分类） */
const clickable = computed(() => spec.value.source.kind === 'dataset' && !['scatter', 'histogram'].includes(spec.value.type))

interface SeriesLike { type?: string, data?: unknown[], markLine?: unknown }

/** 选中的分类高亮：柱 / 扇区其余变淡，折线画竖线；按系列联动时没选中的系列整体变淡 */
function highlight(option: Record<string, unknown>, sel: CrossFilter) {
  const cats = data.value?.categories ?? []
  const names = new Set(sel.values.map(v => v || '（空）'))
  const indexes = new Set(cats.flatMap((c, i) => (names.has(c) ? [i] : [])))
  const seriesOn = sel.series?.values.length ? new Set(sel.series.values) : null
  for (const s of (option.series as SeriesLike[] | undefined) ?? []) {
    if (!Array.isArray(s.data))
      continue
    if (s.type === 'pie') {
      s.data = s.data.map((d) => {
        const item = d as { name?: string, itemStyle?: Record<string, unknown> }
        return { ...item, itemStyle: { ...item.itemStyle, opacity: names.has(item.name ?? '') ? 1 : 0.3 } }
      })
    }
    else if (s.type === 'bar' && indexes.size) {
      const dimmed = !!seriesOn && !seriesOn.has((s as { name?: string }).name ?? '')
      s.data = s.data.map((d, i) => {
        const item = d && typeof d === 'object' && !Array.isArray(d) ? d as { itemStyle?: Record<string, unknown> } : { value: d }
        return { ...item, itemStyle: { ...(item as { itemStyle?: Record<string, unknown> }).itemStyle, opacity: indexes.has(i) && !dimmed ? 1 : 0.3 } }
      })
    }
    else if (s.type === 'line' && indexes.size) {
      s.markLine = { symbol: 'none', silent: true, animation: false, label: { show: false }, lineStyle: { color: '#217346', type: 'dashed', width: 1 }, data: [...indexes].map(i => ({ xAxis: i })) }
    }
  }
}

function render() {
  if (!data.value)
    return
  const built = buildOption(spec.value, data.value)
  notes.value = built.notes
  if (selection.value)
    highlight(built.option as Record<string, unknown>, selection.value)
  if (!chart && el.value) {
    chart = echarts.init(el.value, null, { renderer: 'canvas' })
    registerChartInstance(spec.value.id, chart)
    // 点击分类联动（柱、扇区、折线上的点）
    chart.on('click', (e) => {
      // 参数里的 event 是 zrender 的事件，其中 event 才是浏览器的鼠标事件
      const p = e as { name?: string, componentType?: string, value?: unknown, seriesName?: string, event?: { event?: MouseEvent } }
      const axis = p.componentType === 'xAxis' || p.componentType === 'yAxis'
      onPick(axis ? String(p.value ?? '') : p.name, axis || p.componentType !== 'series' ? undefined : p.seriesName, additive(p.event?.event))
    })
  }
  chart?.setOption(built.option, { notMerge: true })
  chart?.getZr().off('click')
  chart?.getZr().on('click', onCanvasClick)
}

/** 系列点击刚处理过（同一次点击 zrender 会再通知一次） */
let pickedAt = 0

/**
 * 画布点击：柱形 / 折线等有坐标轴的图，点在网格内按位置换算成分类（整列都可点；
 * 悬停时的分类阴影盖在柱子上，系列收不到点击）；点空白处取消本图表发起的联动
 */
function onCanvasClick(e: { target?: unknown, offsetX: number, offsetY: number, event?: MouseEvent }) {
  if (performance.now() - pickedAt < 80 || !chart)
    return
  const pt = [e.offsetX, e.offsetY]
  const axisChart = !['pie', 'donut', 'scatter', 'histogram'].includes(spec.value.type)
  if (clickable.value && axisChart && chart.containPixel('grid', pt)) {
    const horizontal = spec.value.type === 'bar'
    // 按网格换算得到 [横轴值, 纵轴值]；分类轴的值是分类序号（条形图的分类在纵轴）
    const v = chart.convertFromPixel({ gridIndex: 0 }, pt) as unknown as number[] | null
    const index = v ? Math.round(Number(horizontal ? v[1] : v[0])) : -1
    const name = data.value?.categories[index]
    if (name != null) {
      onPick(name, barSeriesAt(pt, index), additive(e.event))
      return
    }
  }
  const sheetId = sheetIdOf(props.api, spec.value.id)
  if (!e.target && selection.value && sheetId)
    clearFilter(sheetId)
}

interface BarLayout { x: number, y: number, width: number, height: number }

/**
 * 点在第 index 个分类的哪个系列的柱子上（按字段拆分了系列的柱 / 条形图）：悬停的分类阴影盖住了柱子，系列收不到点击，
 * 所以按每根柱子的布局矩形判断（堆叠、并排都适用）；不在任何柱子上返回 undefined（只选分类）
 */
function barSeriesAt(pt: number[], index: number): string | undefined {
  const src = spec.value.source
  if (!chart || src.kind !== 'dataset' || !src.splitBy)
    return undefined
  interface ModelLike { getModel: () => { getSeries: () => { subType?: string, name?: string, getData: () => { getItemLayout: (i: number) => BarLayout | undefined } }[] } }
  try {
    for (const s of (chart as unknown as ModelLike).getModel().getSeries()) {
      const l = s.subType === 'bar' ? s.getData().getItemLayout(index) : undefined
      if (l && pt[0]! >= Math.min(l.x, l.x + l.width) && pt[0]! <= Math.max(l.x, l.x + l.width) && pt[1]! >= Math.min(l.y, l.y + l.height) && pt[1]! <= Math.max(l.y, l.y + l.height))
        return s.name
    }
  }
  catch {}
  return undefined
}

/** Ctrl（Mac 为 ⌘）+ 点击：在已选的基础上增减 */
function additive(e: MouseEvent | undefined) {
  return !!(e && (e.ctrlKey || e.metaKey))
}

/** 取消联动并写回工作簿 */
function clearFilter(sheetId: string) {
  clearCrossFilter(sheetId)
  persistCrossFilter(props.api, sheetId)
}

function onPick(name: string | undefined, seriesName: string | undefined, add: boolean) {
  pickedAt = performance.now()
  const src = spec.value.source
  const sheetId = sheetIdOf(props.api, spec.value.id)
  if (!clickable.value || src.kind !== 'dataset' || !sheetId || name == null)
    return
  // 合并出来的「其他」、瀑布图的「合计」不是一个分类
  if (name === '其他' || (spec.value.type === 'waterfall' && name === '合计') || !data.value?.categories.includes(name))
    return
  // 按字段拆分了系列的图表，点在某个系列的柱子上时一并选中这个系列（合并出来的「其他」不是一个取值）
  const series = src.splitBy && seriesName && seriesName !== '其他' ? seriesName : undefined
  toggleCrossFilter(sheetId, spec.value.id, src, { category: name, series, additive: add })
  persistCrossFilter(props.api, sheetId)
}

/**
 * 按在柱、扇区、数据点上时不让工作表接管（否则 Univer 会把这次点击当成选中 / 拖动浮动对象，图表收不到点击）；
 * 按在空白处仍可选中、拖动图表
 */
function onPointerDown(e: PointerEvent | MouseEvent) {
  if (!clickable.value || !chart || !el.value)
    return
  const r = el.value.getBoundingClientRect()
  const hit = (chart.getZr() as unknown as { handler?: { findHover: (x: number, y: number) => { target?: unknown } } }).handler?.findHover(e.clientX - r.left, e.clientY - r.top)
  if (hit?.target)
    e.stopPropagation()
}

function clearSelection(e: MouseEvent) {
  e.stopPropagation()
  const sheetId = sheetIdOf(props.api, spec.value.id)
  if (sheetId)
    clearFilter(sheetId)
}

function edit(e: MouseEvent) {
  e.stopPropagation()
  chartBus.emit('select', spec.value.id)
}

/** 保存为图片（2 倍清晰度，白底，与导出 xlsx 时嵌入的图一致） */
async function saveImage(e: MouseEvent) {
  e.stopPropagation()
  if (!chart || view.value !== 'chart') {
    ElMessage.warning(t('请先切回图表再保存图片'))
    return
  }
  try {
    const blob = await (await fetch(chart.getDataURL({ type: 'png', pixelRatio: 2, backgroundColor: '#fff' }))).blob()
    const name = `${safeFileName(spec.value.title, '图表')}.png`
    downloadBlob(blob, name)
    ElMessage.success(t('已保存图片：{0}', [name]))
  }
  catch (err) {
    ElMessage.error(t('保存图片失败：{0}', [err instanceof Error ? err.message : String(err)]))
  }
}

function toggleView(e: MouseEvent) {
  e.stopPropagation()
  view.value = view.value === 'chart' ? 'table' : 'chart'
  if (view.value === 'chart')
    requestAnimationFrame(() => chart?.resize())
}

onMounted(() => {
  load()
  if (el.value) {
    observer = new ResizeObserver(() => chart?.resize())
    observer.observe(el.value)
  }
  offs.push(
    chartBus.on('spec', (s: ChartSpec) => {
      if (s.id !== spec.value.id)
        return
      spec.value = s
      load()
    }),
    chartBus.on('refresh-range', (sheetId: string) => {
      if (spec.value.source.kind === 'range' && spec.value.source.sheetId === sheetId)
        load()
    }),
    chartBus.on('slicer', (sheetId: string) => {
      if (spec.value.source.kind === 'dataset' && sheetIdOf(props.api, spec.value.id) === sheetId)
        load()
    }),
    chartBus.on('refresh-dataset', (datasetId?: string) => {
      if (spec.value.source.kind === 'dataset' && (!datasetId || spec.value.source.datasetId === datasetId))
        load()
    }),
  )
})

onBeforeUnmount(() => {
  offs.forEach(off => off())
  controller?.abort()
  observer?.disconnect()
  unregisterChartInstance(spec.value.id, chart)
  // 图表删除（或页签关闭）时去掉它发起的联动
  dropCrossFiltersOf(spec.value.id)
  chart?.dispose()
  chart = null
})
</script>

<template>
  <div class="chart-root group" @dblclick="edit" @pointerdown="onPointerDown" @mousedown="onPointerDown">
    <div ref="el" class="h-full w-full transition-opacity duration-150" :class="{ 'opacity-50': loading && data, 'invisible': view === 'table' }" />

    <div v-if="view === 'table' && data" class="table-view">
      <table>
        <thead>
          <tr>
            <th>{{ spec.type === 'histogram' ? $t('区间') : $t('分类') }}</th>
            <th v-for="s in data.series" :key="s.name">
              {{ s.name }}
            </th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(c, i) in data.categories" :key="i">
            <td class="text-left">
              {{ c || $t('（空）') }}
            </td>
            <td v-for="s in data.series" :key="s.name">
              {{ formatCell(s.values[i] ?? null, !!data.money, spec.unit) }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <div v-if="error" class="state flex-col gap-1 text-[#B91C1C]">
      <span>{{ error }}</span>
      <button class="retry" @pointerdown.stop @mousedown.stop @click="load">
        {{ $t('重试') }}
      </button>
    </div>
    <div v-else-if="loading && !data" class="state px-4 text-center text-[#898781]">
      <BusyInline :loading="true" :text="$t('正在计算')" cancellable @cancel="cancel" />
    </div>

    <div class="toolbar">
      <button :title="$t('编辑图表（或双击图表）')" @pointerdown.stop @mousedown.stop @click="edit">
        {{ $t('编辑') }}
      </button>
      <button :title="$t('保存为图片（PNG）')" @pointerdown.stop @mousedown.stop @click="saveImage">
        {{ $t('图片') }}
      </button>
      <button :title="view === 'chart' ? $t('以表格查看全部数值') : $t('回到图表')" @pointerdown.stop @mousedown.stop @click="toggleView">
        {{ view === 'chart' ? $t('数据') : $t('图表') }}
      </button>
    </div>
    <div v-if="selection" class="pick" :title="$t('同一工作表上的其他图表、指标卡、表格按「{0}」筛选；Ctrl（⌘）+ 点击可多选，点空白处或 × 取消', [selection.label])" @pointerdown.stop @mousedown.stop>
      <span class="t">{{ $t('已筛选 {0}', [selection.label]) }}</span>
      <span class="x" @click="clearSelection">×</span>
    </div>
    <div v-if="notes.length && view === 'chart'" class="notes" :title="notes.join('\n')">
      ⓘ {{ notes[0] }}<span v-if="notes.length > 1"> {{ $t('等 {0} 条', [notes.length]) }}</span>
    </div>
  </div>
</template>

<style scoped>
.chart-root {
  position: relative;
  width: 100%;
  height: 100%;
  box-sizing: border-box;
  background: #fff;
  border: 1px solid rgba(11, 11, 11, 0.1);
  border-radius: 6px;
  overflow: hidden;
  font-family: system-ui, -apple-system, 'PingFang SC', 'Microsoft YaHei', sans-serif;
}

.retry {
  padding: 2px 10px;
  font-size: 12px;
  color: #217346;
  cursor: pointer;
  background: #fff;
  border: 1px solid #217346;
  border-radius: 4px;
}

.state {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 16px;
  text-align: center;
  font-size: 12px;
  background: rgba(255, 255, 255, 0.85);
}

.toolbar {
  position: absolute;
  top: 6px;
  right: 6px;
  display: flex;
  gap: 4px;
  opacity: 0;
  transition: opacity 0.15s;
}

.chart-root:hover .toolbar {
  opacity: 1;
}

.toolbar button {
  border: 1px solid #e1e0d9;
  background: #fff;
  color: #52514e;
  font-size: 12px;
  line-height: 18px;
  padding: 1px 8px;
  border-radius: 4px;
  cursor: pointer;
}

.toolbar button:hover {
  border-color: #217346;
  color: #0e5a31;
}

.notes {
  position: absolute;
  left: 8px;
  bottom: 4px;
  max-width: calc(100% - 16px);
  font-size: 11px;
  color: #898781;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.pick {
  /* 右下角：不挡标题（左上）和工具按钮（右上） */
  position: absolute;
  bottom: 4px;
  right: 8px;
  display: flex;
  align-items: center;
  gap: 4px;
  max-width: calc(100% - 16px);
  padding: 1px 6px;
  border-radius: 3px;
  background: #e7f4ec;
  color: #0b5a2e;
  font-size: 11px;
  white-space: nowrap;
}

.pick .t {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

.pick .x {
  flex-shrink: 0;
  cursor: pointer;
  color: #52514e;
}

.table-view {
  position: absolute;
  inset: 30px 0 0;
  overflow: auto;
  font-size: 12px;
}

.table-view table {
  width: 100%;
  border-collapse: collapse;
  font-variant-numeric: tabular-nums;
}

.table-view th,
.table-view td {
  padding: 4px 10px;
  border-bottom: 1px solid #f0efec;
  text-align: right;
  white-space: nowrap;
  color: #0b0b0b;
}

.table-view th {
  position: sticky;
  top: 0;
  background: #f9f9f7;
  color: #52514e;
  font-weight: 600;
}

.table-view th:first-child {
  text-align: left;
}
</style>
