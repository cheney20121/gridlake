<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { DatasetFilter, DatasetInfo, PivotAgg } from '@/api/dataset'
import type { ChartSource, ChartSpec, ChartType, DatasetSource, RangeSource } from '@/features/excel/chart/types'
import { useDebounceFn } from '@vueuse/core'
import { computed, ref, watch } from 'vue'
import { readRange, resolveChartData } from '@/features/excel/chart/chart-data'
import { selectionSource, suggestType } from '@/features/excel/chart/chart-defaults'
import { findChart, insertChart, newChartId, removeChart, updateChart } from '@/features/excel/chart/chart-manager'
import { CHART_TYPE_LABEL } from '@/features/excel/chart/types'
import { t as tr } from '@/i18n'

/**
 * 图表面板：新建（默认用当前选区）或编辑选中的图表；编辑时修改即时生效
 */
const props = defineProps<{
  univerApi: FUniver | null
  datasets: DatasetInfo[]
  /** 正在编辑的图表 ID；为空时新建 */
  chartId: string | null
  /** 从数据表「图表分析」带入的数据集与筛选 */
  initialDataset?: { datasetId: string, filters: DatasetFilter[] } | null
}>()

const emit = defineEmits<{
  (e: 'update:chartId', id: string | null): void
  (e: 'changed'): void
}>()

const TYPES: { type: ChartType, icon: string, hint: string }[] = [
  { type: 'column', icon: '▮▮', hint: tr('比较各分类的大小') },
  { type: 'bar', icon: '▬', hint: tr('分类多或名称长时比较大小') },
  { type: 'line', icon: '╱╲', hint: tr('随时间的变化趋势') },
  { type: 'area', icon: '◢', hint: tr('趋势，强调累计量') },
  { type: 'combo', icon: '▮╱', hint: tr('柱 + 线（同一数值轴）') },
  { type: 'waterfall', icon: '▟▙', hint: tr('从起点到终点的增减构成（利润桥）') },
  { type: 'pie', icon: '◔', hint: tr('占比（不超过 6 块）') },
  { type: 'donut', icon: '◯', hint: tr('占比（不超过 6 块）') },
  { type: 'scatter', icon: '⁘', hint: tr('两个数值的关系（首列为 X）') },
  { type: 'histogram', icon: '▁▃▇▅', hint: tr('数值的分布') },
]

const AGGS: { value: PivotAgg, label: string }[] = [
  { value: 'sum', label: tr('求和') },
  { value: 'count', label: tr('计数') },
  { value: 'avg', label: tr('平均') },
  { value: 'max', label: tr('最大') },
  { value: 'min', label: tr('最小') },
  { value: 'count_distinct', label: tr('去重计数') },
]

const spec = ref<ChartSpec | null>(null)
/** 最近一次写回（或打开时）的定义：没有实际修改时不写回，避免一打开面板就标记工作簿已修改 */
let lastApplied = ''
const error = ref('')
const seriesNames = ref<string[]>([])

const editing = computed(() => !!props.chartId)
const source = computed(() => spec.value?.source)
const rangeSource = computed(() => source.value?.kind === 'range' ? source.value : null)
const datasetSource = computed(() => source.value?.kind === 'dataset' ? source.value : null)
const dataset = computed(() => datasetSource.value ? props.datasets.find(d => d.id === datasetSource.value!.datasetId) : undefined)
const xColumns = computed(() => dataset.value?.columns ?? [])
const numericColumns = computed(() => dataset.value?.columns.filter(c => c.kind === 'money' || c.kind === 'number') ?? [])
const splitColumns = computed(() => dataset.value?.columns.filter(c => c.kind === 'text' || c.kind === 'id') ?? [])
const xIsDate = computed(() => xColumns.value.find(c => c.key === datasetSource.value?.x.column)?.kind === 'datetime')
const sheetName = computed(() => rangeSource.value ? props.univerApi?.getActiveWorkbook()?.getSheetBySheetId(rangeSource.value.sheetId)?.getSheetName() ?? '（已删除）' : '')
const stackable = computed(() => spec.value && ['column', 'bar', 'area'].includes(spec.value.type))

function datasetDefaults(datasetId: string, filters: DatasetFilter[] = []): DatasetSource {
  const ds = props.datasets.find(d => d.id === datasetId)
  const x = ds?.columns.find(c => c.kind === 'datetime') ?? ds?.columns.find(c => c.kind === 'text') ?? ds?.columns[0]
  const v = ds?.columns.find(c => c.kind === 'money') ?? ds?.columns.find(c => c.kind === 'number')
  return {
    kind: 'dataset',
    datasetId,
    x: { column: x?.key ?? '', granularity: x?.kind === 'datetime' ? 'month' : null },
    values: v ? [{ column: v.key, agg: 'sum' }] : [{ agg: 'count' }],
    splitBy: null,
    filters,
    sort: x?.kind === 'datetime' ? 'x' : 'value',
    top: x?.kind === 'datetime' ? null : 20,
  }
}

/** 打开面板：编辑已有图表，或按选区 / 带入的数据集准备新图表 */
function init() {
  error.value = ''
  const api = props.univerApi
  if (!api)
    return
  if (props.chartId) {
    const loc = findChart(api, props.chartId)
    if (!loc) {
      emit('update:chartId', null)
      return
    }
    spec.value = JSON.parse(JSON.stringify(loc.spec))
    lastApplied = JSON.stringify(spec.value)
    refreshSeriesNames()
    return
  }
  let src: ChartSource | null = props.initialDataset ? datasetDefaults(props.initialDataset.datasetId, props.initialDataset.filters) : selectionSource(api)
  let type: ChartType = 'column'
  if (src?.kind === 'range') {
    try {
      type = suggestType(readRange(api, src))
    }
    catch {
      // 选区不是数据：保留默认，面板里提示
    }
  }
  else if (src?.kind === 'dataset') {
    type = xIsDateOf(src) ? 'line' : 'bar'
  }
  if (!src) {
    const sheet = api.getActiveWorkbook()?.getActiveSheet()
    src = { kind: 'range', sheetId: sheet?.getSheetId() ?? '', range: '', headerRow: true, firstColLabels: true, seriesIn: 'columns' }
  }
  spec.value = { v: 1, id: newChartId(), type, title: '', source: src, legend: 'top', labels: 'none', unit: 'auto', waterfallTotal: true }
  refreshSeriesNames()
}

function xIsDateOf(src: DatasetSource) {
  return props.datasets.find(d => d.id === src.datasetId)?.columns.find(c => c.key === src.x.column)?.kind === 'datetime'
}

watch(() => [props.chartId, props.initialDataset], init, { immediate: true })

async function refreshSeriesNames() {
  if (!spec.value || !props.univerApi)
    return
  try {
    const d = await resolveChartData(props.univerApi, { ...spec.value, type: spec.value.type === 'histogram' ? 'column' : spec.value.type })
    seriesNames.value = d.series.map(s => s.name)
    error.value = ''
  }
  catch (e) {
    seriesNames.value = []
    error.value = e instanceof Error ? e.message : String(e)
  }
}

// 编辑已有图表：修改即时写回（防抖）
const apply = useDebounceFn(() => {
  if (!spec.value || !props.univerApi || !editing.value)
    return
  const json = JSON.stringify(spec.value)
  if (json === lastApplied)
    return
  try {
    updateChart(props.univerApi, JSON.parse(json))
    lastApplied = json
    emit('changed')
  }
  catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  }
}, 250)

const refreshSeriesNamesLater = useDebounceFn(refreshSeriesNames, 400)

// 瀑布图的每一项都需要看到数值：切换到瀑布图时默认显示全部标签，切走时恢复为无
watch(() => spec.value?.type, (t, old) => {
  if (!spec.value || !old)
    return
  if (t === 'waterfall' && spec.value.labels === 'none')
    spec.value.labels = 'all'
  else if (old === 'waterfall' && spec.value.labels === 'all')
    spec.value.labels = 'none'
})

watch(spec, () => {
  apply()
  refreshSeriesNamesLater()
}, { deep: true })

function useSelection() {
  const api = props.univerApi
  if (!api || !spec.value)
    return
  const src = selectionSource(api)
  if (!src) {
    ElMessage.warning(tr('请先在工作表中选中要做图的数据区域'))
    return
  }
  spec.value.source = src
}

function setSourceKind(kind: 'range' | 'dataset') {
  if (!spec.value || spec.value.source.kind === kind)
    return
  if (kind === 'dataset') {
    const first = props.datasets[0]
    if (!first) {
      ElMessage.warning(tr('还没有数据集'))
      return
    }
    spec.value.source = datasetDefaults(first.id)
  }
  else {
    spec.value.source = selectionSource(props.univerApi!) ?? { kind: 'range', sheetId: props.univerApi?.getActiveWorkbook()?.getActiveSheet().getSheetId() ?? '', range: '', headerRow: true, firstColLabels: true, seriesIn: 'columns' }
  }
}

function onDatasetChange(id: string) {
  if (spec.value)
    spec.value.source = datasetDefaults(id)
}

function onXChange() {
  const s = datasetSource.value
  if (!s)
    return
  s.x.granularity = xIsDate.value ? (s.x.granularity ?? 'month') : null
  s.sort = xIsDate.value ? 'x' : 'value'
}

function addValue() {
  datasetSource.value?.values.push({ column: numericColumns.value[0]?.key ?? null, agg: numericColumns.value.length ? 'sum' : 'count' })
  if (datasetSource.value)
    datasetSource.value.splitBy = null
}

function insert() {
  const api = props.univerApi
  if (!api || !spec.value)
    return
  if (error.value) {
    ElMessage.warning(error.value)
    return
  }
  try {
    const s = JSON.parse(JSON.stringify(spec.value)) as ChartSpec
    if (s.source.kind === 'range') {
      // 图表放在数据所在的工作表上
      const sheet = api.getActiveWorkbook()?.getSheetBySheetId(s.source.sheetId) ?? undefined
      insertChart(api, s, sheet)
    }
    else {
      insertChart(api, s)
    }
    lastApplied = JSON.stringify(s)
    emit('update:chartId', s.id)
    emit('changed')
    ElMessage.success(tr('已插入图表，可拖动位置、拖动边角调整大小'))
  }
  catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e))
  }
}

async function remove() {
  if (!props.univerApi || !props.chartId)
    return
  const ok = await ElMessageBox.confirm(tr('删除这个图表？'), tr('删除图表'), { type: 'warning', confirmButtonText: tr('删除') }).then(() => true, () => false)
  if (!ok)
    return
  removeChart(props.univerApi, props.chartId)
  emit('update:chartId', null)
  emit('changed')
}

function seriesType(name: string, i: number) {
  return spec.value?.seriesTypes?.[name] ?? (i === 0 ? 'bar' : 'line')
}

function setSeriesType(name: string, t: 'bar' | 'line') {
  if (!spec.value)
    return
  spec.value.seriesTypes = { ...spec.value.seriesTypes, [name]: t }
}
</script>

<template>
  <div v-if="spec" class="flex flex-col gap-3 text-[13px]">
    <div class="flex items-center justify-between">
      <span class="font-medium">{{ editing ? $t('编辑图表') : $t('新建图表') }}</span>
      <el-button v-if="editing" link size="small" @click="emit('update:chartId', null)">
        {{ $t('+ 新建另一个') }}
      </el-button>
    </div>

    <!-- 类型 -->
    <div class="grid grid-cols-5 gap-1">
      <button
        v-for="t in TYPES"
        :key="t.type"
        class="type-btn"
        :class="{ active: spec.type === t.type }"
        :title="`${CHART_TYPE_LABEL[t.type]}：${t.hint}`"
        @click="spec.type = t.type"
      >
        <span class="text-[13px] leading-4">{{ t.icon }}</span>
        <span class="text-[11px]">{{ CHART_TYPE_LABEL[t.type] }}</span>
      </button>
    </div>
    <div class="text-xs text-gray-400">
      {{ TYPES.find(t => t.type === spec!.type)?.hint }}
    </div>

    <!-- 数据来源 -->
    <div class="section">
      <div class="section-title">
        {{ $t('数据') }}
      </div>
      <el-radio-group :model-value="spec.source.kind" size="small" @update:model-value="(v: any) => setSourceKind(v)">
        <el-radio-button value="range">
          {{ $t('单元格区域') }}
        </el-radio-button>
        <el-radio-button value="dataset" :disabled="!datasets.length">
          {{ $t('数据集（千万行）') }}
        </el-radio-button>
      </el-radio-group>

      <template v-if="rangeSource">
        <div class="row">
          <el-input v-model="(spec.source as RangeSource).range" size="small" :placeholder="$t('如 A1:D13')" class="!w-32" />
          <el-button size="small" @click="useSelection">
            {{ $t('用当前选区') }}
          </el-button>
        </div>
        <div class="text-xs text-gray-500">
          {{ $t('工作表：{0}；数据变化后图表自动更新', [sheetName]) }}
        </div>
        <div class="flex flex-wrap gap-x-3">
          <el-checkbox v-model="(spec.source as RangeSource).headerRow" size="small">
            {{ $t('首行是系列名') }}
          </el-checkbox>
          <el-checkbox v-model="(spec.source as RangeSource).firstColLabels" size="small">
            {{ $t('首列是分类') }}
          </el-checkbox>
        </div>
        <div class="row">
          <span class="label">{{ $t('系列在') }}</span>
          <el-radio-group v-model="(spec.source as RangeSource).seriesIn" size="small">
            <el-radio value="columns">
              {{ $t('列') }}
            </el-radio>
            <el-radio value="rows">
              {{ $t('行') }}
            </el-radio>
          </el-radio-group>
        </div>
      </template>

      <template v-else-if="datasetSource">
        <el-select :model-value="datasetSource.datasetId" size="small" filterable @update:model-value="onDatasetChange">
          <el-option v-for="d in datasets" :key="d.id" :label="$t('{0}（{1} 行）', [d.name, d.rowCount.toLocaleString()])" :value="d.id" />
        </el-select>
        <div class="row">
          <span class="label">{{ $t('分类轴') }}</span>
          <el-select v-model="datasetSource.x.column" size="small" filterable class="flex-1" @change="onXChange">
            <el-option v-for="c in xColumns" :key="c.key" :label="c.title" :value="c.key" />
          </el-select>
          <el-select v-if="xIsDate" v-model="datasetSource.x.granularity" size="small" class="!w-20">
            <el-option :label="$t('年')" value="year" />
            <el-option :label="$t('月')" value="month" />
            <el-option :label="$t('日')" value="day" />
          </el-select>
        </div>
        <div v-for="(v, i) in datasetSource.values" :key="i" class="row">
          <span class="label">{{ i === 0 ? $t('数值') : '' }}</span>
          <el-select v-model="v.agg" size="small" class="!w-24">
            <el-option v-for="a in AGGS" :key="a.value" :label="a.label" :value="a.value" />
          </el-select>
          <el-select v-if="v.agg !== 'count'" v-model="v.column" size="small" filterable class="flex-1">
            <el-option v-for="c in (v.agg === 'count_distinct' ? xColumns : numericColumns)" :key="c.key" :label="c.title" :value="c.key" />
          </el-select>
          <span v-else class="flex-1 text-xs text-gray-400">{{ $t('按行数') }}</span>
          <el-button v-if="datasetSource.values.length > 1" link size="small" type="danger" @click="datasetSource.values.splice(i, 1)">
            {{ $t('删除') }}
          </el-button>
        </div>
        <el-button link size="small" type="primary" class="self-start" @click="addValue">
          {{ $t('+ 再加一个数值') }}
        </el-button>
        <div class="row">
          <span class="label">{{ $t('拆分系列') }}</span>
          <el-select v-model="datasetSource.splitBy" size="small" clearable filterable class="flex-1" :disabled="datasetSource.values.length > 1" :placeholder="$t('不拆分（如按店铺拆分）')">
            <el-option v-for="c in splitColumns" :key="c.key" :label="c.title" :value="c.key" />
          </el-select>
        </div>
        <div class="row">
          <span class="label">{{ $t('排序') }}</span>
          <el-radio-group v-model="datasetSource.sort" size="small">
            <el-radio value="x">
              {{ $t('按分类') }}
            </el-radio>
            <el-radio value="value">
              {{ $t('按数值降序') }}
            </el-radio>
          </el-radio-group>
        </div>
        <div class="row">
          <span class="label">{{ $t('前 N 项') }}</span>
          <el-input-number v-model="datasetSource.top" :min="1" :max="500" size="small" :controls="false" class="!w-20" :placeholder="$t('全部')" />
          <span class="text-xs text-gray-400">{{ $t('按第一个数值排名，空为全部') }}</span>
        </div>
        <div v-if="datasetSource.filters?.length" class="flex items-center gap-2 text-xs text-gray-500">
          {{ $t('已带入 {0} 个筛选条件', [datasetSource.filters.length]) }}
          <el-button link size="small" @click="datasetSource.filters = []">
            {{ $t('清除') }}
          </el-button>
        </div>
        <div class="text-xs text-gray-400">
          {{ $t('在服务端按分类汇总，千万行只返回汇总结果；数据集更新后自动刷新') }}
        </div>
      </template>
    </div>

    <div v-if="error" class="rounded bg-[#FEF2F2] px-2 py-1.5 text-xs text-[#B91C1C]">
      {{ error }}
    </div>

    <!-- 样式 -->
    <div class="section">
      <div class="section-title">
        {{ $t('显示') }}
      </div>
      <el-input v-model="spec.title" size="small" :placeholder="$t('图表标题（单系列时标题即说明）')" maxlength="60" />
      <div v-if="stackable" class="row">
        <el-checkbox v-model="spec.stacked" size="small">
          {{ $t('堆叠') }}
        </el-checkbox>
      </div>
      <div class="row">
        <span class="label">{{ $t('图例') }}</span>
        <el-radio-group v-model="spec.legend" size="small">
          <el-radio-button value="top">
            {{ $t('上') }}
          </el-radio-button>
          <el-radio-button value="bottom">
            {{ $t('下') }}
          </el-radio-button>
          <el-radio-button value="right">
            {{ $t('右') }}
          </el-radio-button>
        </el-radio-group>
        <span class="text-xs text-gray-400">{{ $t('单系列不显示') }}</span>
      </div>
      <div class="row">
        <span class="label">{{ $t('数据标签') }}</span>
        <el-radio-group v-model="spec.labels" size="small">
          <el-radio-button value="none">
            {{ $t('无') }}
          </el-radio-button>
          <el-radio-button value="key">
            {{ $t('关键点') }}
          </el-radio-button>
          <el-radio-button value="all">
            {{ $t('全部') }}
          </el-radio-button>
        </el-radio-group>
      </div>
      <div class="text-xs text-gray-400">
        {{ $t('关键点：最后一个与最大的一个；点数多时「全部」会自动改为关键点') }}
      </div>
      <div class="row">
        <span class="label">{{ $t('单位') }}</span>
        <el-radio-group v-model="spec.unit" size="small">
          <el-radio-button value="auto">
            {{ $t('自动（万 / 亿）') }}
          </el-radio-button>
          <el-radio-button value="raw">
            {{ $t('原值') }}
          </el-radio-button>
          <el-radio-button value="percent">
            {{ $t('百分比') }}
          </el-radio-button>
        </el-radio-group>
      </div>
      <div v-if="spec.type === 'waterfall'" class="row">
        <el-checkbox v-model="spec.waterfallTotal" size="small">
          {{ $t('末尾显示合计') }}
        </el-checkbox>
        <span class="text-xs text-gray-400">{{ $t('每行是一项增减（如 收入、成本、费用）') }}</span>
      </div>
      <div v-if="spec.type === 'histogram'" class="row">
        <span class="label">{{ $t('分组数') }}</span>
        <el-input-number v-model="spec.bins" :min="2" :max="50" size="small" :controls="false" class="!w-20" :placeholder="$t('自动')" />
      </div>
      <template v-if="spec.type === 'combo'">
        <div v-for="(n, i) in seriesNames" :key="n" class="row">
          <span class="min-w-0 flex-1 truncate" :title="n">{{ n }}</span>
          <el-radio-group :model-value="seriesType(n, i)" size="small" @update:model-value="(v: any) => setSeriesType(n, v)">
            <el-radio-button value="bar">
              {{ $t('柱') }}
            </el-radio-button>
            <el-radio-button value="line">
              {{ $t('线') }}
            </el-radio-button>
          </el-radio-group>
        </div>
        <div class="text-xs text-gray-400 leading-5">
          {{ $t('柱与线共用一个数值轴。量纲差别大的指标（如金额与增长率）请分成两张图，双坐标轴容易让人误读两者的关系。') }}
        </div>
      </template>
    </div>

    <div class="flex gap-2">
      <el-button v-if="!editing" type="primary" size="small" :disabled="!!error" @click="insert">
        {{ $t('插入图表') }}
      </el-button>
      <el-button v-else size="small" type="danger" plain @click="remove">
        {{ $t('删除图表') }}
      </el-button>
    </div>
    <div class="text-xs text-gray-400 leading-5">
      {{ $t('图表随工作簿保存；双击图表或点右上角「编辑」修改；「数据」可切换为表格查看全部数值。导出 Excel 时图表以图片保存。') }}
    </div>
  </div>
</template>

<style scoped lang="scss">
.type-btn {
  @apply flex flex-col items-center gap-0.5 rounded border border-[#e5e7eb] border-solid bg-white px-0.5 py-1.5 text-gray-600 cursor-pointer hover:border-[#217346];

  &.active {
    @apply border-[#217346] bg-[#E7F4EC] text-[#0E5A31];
  }
}

.section {
  @apply flex flex-col gap-2 border-t border-[#eef0f3] border-t-solid pt-3;
}

.section-title {
  @apply text-xs text-gray-500 font-medium;
}

.row {
  @apply flex items-center gap-2;
}

.label {
  @apply w-14 shrink-0 text-xs text-gray-500;
}
</style>
