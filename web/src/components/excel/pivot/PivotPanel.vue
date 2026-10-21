<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import type { Area, Item } from './constants'
import type { DatasetColumn, DatasetFilter, DatasetInfo, PivotRequest, PivotResult, PivotShow, SummaryDef } from '@/api/dataset'
import type { GridColumn } from '@/components/excel/big-grid/types'
import type { PivotDisplay, PivotSpec } from '@/features/excel/pivot/pivot-sheet'
import { useDebounceFn } from '@vueuse/core'
import { isCancel } from 'axios'
import { computed, onBeforeUnmount, reactive, ref, shallowRef, watch } from 'vue'
import draggable from 'vuedraggable'
import { createSummaryApi } from '@/api/dataset'
import FilterMenu from '@/components/excel/big-grid/FilterMenu.vue'
import BusyInline from '@/components/excel/common/BusyInline.vue'
import { createPivotSheet, getPivotSpec, onPivotChanged, pivotGroupPrefixes, refreshPivot, setPivotDisplay } from '@/features/excel/pivot/pivot-sheet'
import { t as tr } from '@/i18n'
import { errorText } from '@/utils/busy-hint'
import { AGG_OPTIONS, AREAS, cloneField, GRANULARITY_OPTIONS, isNumeric, SHOW_OPTIONS, valueLabel } from './constants'
import PivotCalcDialog from './PivotCalcDialog.vue'
import PivotFieldList from './PivotFieldList.vue'
import PivotItemDialog from './PivotItemDialog.vue'
import PivotOptions from './PivotOptions.vue'

/**
 * 透视表字段面板（与 Excel「数据透视表字段」一致）：把字段拖进 筛选 / 列 / 行 / 值，结果实时写入当前透视表页签
 */
const props = defineProps<{
  univerApi: FUniver | null
  datasets: DatasetInfo[]
}>()

const emit = defineEmits<{
  /** 透视表内容变化（需要保存工作簿） */
  (e: 'changed'): void
  /** 已转为数据表（新建了汇总数据集）：页面刷新数据集并把它放进新工作表 */
  (e: 'converted', datasetId: string): void
}>()

const sheet = shallowRef<FWorksheet | null>(null)
const datasetId = ref('')
const areas = reactive<Record<Area, Item[]>>({ filters: [], columns: [], rows: [], values: [] })
const options = reactive({ sort: 'key-asc', top: undefined as number | undefined, subtotals: true, grandTotals: true })
/** 版式（只影响呈现，不重新计算） */
const display = reactive<Required<PivotDisplay>>({ layout: 'tabular', repeatLabels: false, collapsed: [] })
/** 行 / 列标签别名（字段列名 → 原值 → 别名） */
const aliases = ref<Record<string, Record<string, string>>>({})
/** 计算项：在某个行 / 列字段的取值之间做运算（期末 = 期初 + 入库 - 出库） */
interface CalcItemDef { field: string, name: string, formula: string }
const calcItems = ref<CalcItemDef[]>([])
const fieldSearch = ref('')
const loading = ref(false)
const error = ref('')
const lastResult = shallowRef<PivotResult | null>(null)
/** 正在把页签里的定义载入面板（期间不触发重算） */
let loadingSpec = false
let seq = 0

const dataset = computed(() => props.datasets.find(d => d.id === datasetId.value) ?? null)
const fields = computed(() => {
  const kw = fieldSearch.value.trim().toLowerCase()
  return (dataset.value?.columns ?? []).filter(c => !kw || c.title.toLowerCase().includes(kw))
})
const canGroupByDate = (it: Item) => it.kind === 'datetime' || /日期|时间|月份/.test(it.column)
const used = computed(() => new Set(Object.values(areas).flat().map(i => i.column)))

/** 拖入值区时设默认汇总方式，拖出值区时去掉 */
function normalize() {
  for (const it of areas.values) {
    if (!it.agg)
      it.agg = isNumeric(it.kind) ? 'sum' : 'count'
    if (!it.show)
      it.show = 'raw'
  }
  for (const key of ['filters', 'columns', 'rows'] as const) {
    for (const it of areas[key]) {
      delete it.agg
      delete it.show
    }
  }
}

/** 空白透视表上给几个一键起步的组合：按店铺 / 渠道这类分类看金额，按月看走势 */
const suggestions = computed(() => {
  const cols = dataset.value?.columns ?? []
  const measure = cols.find(c => c.kind === 'money') ?? cols.find(c => c.kind === 'number')
  const texts = cols.filter(c => c.kind === 'text').slice(0, 2)
  const date = cols.find(c => c.kind === 'datetime')
  const out: { label: string, apply: () => void }[] = []
  const setValue = () => {
    areas.values = [measure ? { ...cloneField(measure), agg: 'sum', show: 'raw' } : { uid: `v-${Math.random().toString(36).slice(2, 8)}`, column: '（行数）', kind: 'number', agg: 'count', show: 'raw' }]
  }
  const start = (row: Item) => () => {
    areas.rows = [row]
    setValue()
  }
  for (const t of texts)
    out.push({ label: tr('按「{0}」看{1}', [t.title, measure ? tr('「{0}」合计', [measure.title]) : tr('行数')]), apply: start(cloneField(t)) })
  if (date)
    out.push({ label: tr('按月看{0}走势', [measure ? `「${measure.title}」` : tr('行数')]), apply: start({ ...cloneField(date), granularity: 'month' }) })
  return out
})
const isBlank = computed(() => !areas.rows.length && !areas.columns.length && !areas.values.length && !areas.filters.length)

function addTo(area: Area, col: DatasetColumn) {
  areas[area].push(cloneField(col))
  normalize()
}

/** 复制一个值字段并设为某种显示方式（常见用法：金额 + 金额的同比并排） */
function addShowCopy(it: Item, show: PivotShow) {
  const i = areas.values.findIndex(x => x.uid === it.uid)
  areas.values.splice(i + 1, 0, { ...it, uid: `v-${Math.random().toString(36).slice(2, 8)}`, show })
}

const usesPeriod = computed(() => areas.values.some(v => v.show && ['yoy', 'yoy_diff', 'mom', 'mom_diff'].includes(v.show)))

function remove(area: Area, uid: string) {
  areas[area] = areas[area].filter(i => i.uid !== uid)
}

// ── 页签 ↔ 面板 ─────────────────────────────────────
function itemsFrom(fieldsList: { column: string, granularity?: string | null, label?: string }[]): Item[] {
  return fieldsList.map(f => ({
    uid: `${f.column}-${Math.random().toString(36).slice(2, 8)}`,
    column: f.column,
    kind: dataset.value?.columns.find(c => c.title === f.column)?.kind ?? 'text',
    granularity: (f.granularity ?? null) as Item['granularity'],
    ...(f.label ? { label: f.label } : {}),
  }))
}

function loadSpec(spec: PivotSpec | null) {
  loadingSpec = true
  datasetId.value = spec?.datasetId ?? ''
  const r = spec?.request
  areas.rows = itemsFrom(r?.rows ?? [])
  areas.columns = itemsFrom(r?.columns ?? [])
  areas.values = (r?.values ?? []).map(v => ({
    uid: `v-${Math.random().toString(36).slice(2, 8)}`,
    column: v.agg === 'calc' ? '（计算值）' : (v.column ?? '（行数）'),
    kind: v.agg === 'calc' ? 'number' : (dataset.value?.columns.find(c => c.title === v.column)?.kind ?? 'number'),
    agg: v.agg,
    show: v.show ?? 'raw',
    ...(v.label ? { label: v.label } : {}),
    ...(v.agg === 'calc' ? { formula: v.formula ?? '', format: v.format ?? 'number' } : {}),
  }))
  // 筛选：每列一个条目，条件挂在条目上
  areas.filters = (r?.filters ?? []).map(f => ({
    uid: `f-${Math.random().toString(36).slice(2, 8)}`,
    column: f.column,
    kind: dataset.value?.columns.find(c => c.title === f.column || c.key === f.column)?.kind ?? 'text',
    filter: f,
  }))
  options.sort = r?.sort ? `${r.sort.by}-${r.sort.desc ? 'desc' : 'asc'}` : 'key-asc'
  options.top = r?.top ?? undefined
  options.subtotals = r?.subtotals ?? true
  options.grandTotals = r?.grandTotals ?? true
  aliases.value = structuredClone(r?.aliases ?? {})
  calcItems.value = structuredClone(r?.calcItems ?? [])
  display.layout = spec?.display?.layout ?? 'tabular'
  display.repeatLabels = spec?.display?.repeatLabels ?? false
  display.collapsed = [...(spec?.display?.collapsed ?? [])]
  lastResult.value = null
  error.value = ''
  setTimeout(() => (loadingSpec = false), 0)
}

function currentRequest(): PivotRequest {
  const [by, dir] = options.sort.split('-') as ['key' | 'value', 'asc' | 'desc']
  const label = (i: Item) => (i.label ? { label: i.label } : {})
  const field = (i: Item) => ({ column: i.column, granularity: i.granularity ?? null, ...label(i) })
  // 只保留仍在行 / 列里的字段的别名
  const inUse = new Set([...areas.rows, ...areas.columns].map(i => i.column))
  const kept = Object.fromEntries(Object.entries(aliases.value).filter(([col, map]) => inUse.has(col) && Object.keys(map).length))
  return {
    rows: areas.rows.map(field),
    columns: areas.columns.map(field),
    values: areas.values.map(i => i.agg === 'calc'
      ? { column: null, agg: 'calc' as const, formula: i.formula, format: i.format ?? 'number', ...label(i) }
      : { column: i.column === '（行数）' ? null : i.column, agg: i.agg ?? 'count', ...(i.show && i.show !== 'raw' ? { show: i.show } : {}), ...label(i) }),
    filters: areas.filters.map(i => i.filter).filter((f): f is DatasetFilter => !!f),
    sort: { by, valueIndex: 0, desc: dir === 'desc' },
    top: options.top || null,
    subtotals: options.subtotals,
    grandTotals: options.grandTotals,
    ...(Object.keys(kept).length ? { aliases: kept } : {}),
    // 只保留所在字段仍在行 / 列里的计算项
    ...(calcItems.value.some(c => inUse.has(c.field)) ? { calcItems: calcItems.value.filter(c => inUse.has(c.field)).map(c => ({ ...c })) } : {}),
  }
}

// ── 转为数据表：透视结果按行展平成一个汇总数据集（随源数据实时变化），之后可按值筛选、加计算列 ──

const canConvert = computed(() => !!dataset.value && (areas.rows.length + areas.columns.length) > 0 && areas.values.length > 0)
const converting = ref(false)

/** 不会带入数据表的设置（展平后没有意义），转换前告诉用户 */
function convertDrops(req: PivotRequest): string[] {
  const out: string[] = []
  const shown = req.values.filter(v => v.show && v.show !== 'raw')
  if (shown.length)
    out.push(tr('值的显示方式（{0}）——按行展平后没有可比的总计或上一期，这些值按原值带入', [shown.map(v => SHOW_OPTIONS.find(o => o.value === v.show)?.label ?? v.show).join('、')]))
  if (req.calcItems?.length)
    out.push(tr('计算项'))
  if (req.top)
    out.push(tr('「前 {0} 项」——数据表里可以按值排序后自行筛选', [req.top]))
  if (req.subtotals || req.grandTotals)
    out.push(tr('小计 / 总计行'))
  return out
}

async function convertToTable() {
  const ds = dataset.value
  if (!ds || !canConvert.value)
    return
  const req = currentRequest()
  const drops = convertDrops(req)
  let name: string
  try {
    const r = await ElMessageBox.prompt(
      `行字段、列字段成为分组列，汇总值与计算值成为数值列；之后可以在数据表里按值筛选、排序、加计算列，源数据变化时自动更新。${drops.length ? `<br><br><span style="color:#92400E">不带入：${drops.join('；')}</span>` : ''}`,
      tr('转为数据表'),
      { inputValue: tr('{0}汇总', [ds.name]), inputPlaceholder: tr('汇总数据集的名称'), confirmButtonText: tr('转换'), dangerouslyUseHTMLString: true, inputValidator: v => !!v.trim() || tr('请填写名称') },
    )
    name = r.value.trim()
  }
  catch {
    return
  }
  const def: SummaryDef = {
    source: ds.id,
    rows: [...req.rows, ...req.columns],
    values: req.values.map(({ show: _show, ...v }) => v),
    filters: req.filters ?? [],
  }
  converting.value = true
  try {
    const id = await createSummaryApi(name, def)
    emit('converted', id)
  }
  catch (e) {
    error.value = errorText(e)
  }
  finally {
    converting.value = false
  }
}

function currentDisplay(): PivotDisplay {
  return { layout: display.layout, repeatLabels: display.repeatLabels, collapsed: [...display.collapsed] }
}

let controller: AbortController | null = null
async function apply() {
  const s = sheet.value
  const ds = dataset.value
  if (!s || !ds || loadingSpec)
    return
  const spec: PivotSpec = { version: 1, datasetId: ds.id, datasetName: ds.name, request: currentRequest(), display: currentDisplay() }
  if (!spec.request.values.length) {
    // 还没有值字段：只保存定义，页签里给出提示
    s.setCustomMetadata({ ...(s.getCustomMetadata() ?? {}), pivot: spec })
    s.getRange(0, 0).setValue(tr('数据透视表 · {0}：从右侧字段列表把字段拖入「行」「列」「值」', [ds.name]))
    lastResult.value = null
    error.value = ''
    return
  }
  controller?.abort()
  const c = new AbortController()
  controller = c
  const my = ++seq
  loading.value = true
  error.value = ''
  try {
    const result = await refreshPivot(s, spec, c.signal)
    if (my === seq) {
      lastResult.value = result
      emit('changed')
    }
  }
  catch (e) {
    if (!isCancel(e) && my === seq) {
      error.value = errorText(e, tr('计算失败'))
    }
  }
  finally {
    if (my === seq)
      loading.value = false
  }
}
const debouncedApply = useDebounceFn(apply, 350)

/** 放弃这次计算：工作表里保留上一次的结果，点「刷新」再算 */
function cancelApply() {
  controller?.abort()
  seq++
  loading.value = false
  error.value = tr('已取消，点「刷新」重新计算')
}

watch(() => [areas, options, aliases.value, calcItems.value], () => {
  normalize()
  if (!loadingSpec)
    debouncedApply()
}, { deep: true })

// 版式变化：用上次结果重画
watch(display, () => {
  const s = sheet.value
  if (!s || loadingSpec)
    return
  setPivotDisplay(s, currentDisplay()).then(() => emit('changed')).catch((e) => {
    error.value = e instanceof Error ? e.message : tr('重画失败')
  })
}, { deep: true })

/** 紧凑版式：全部折叠到第一级 / 全部展开 */
function collapseAll(collapse: boolean) {
  display.collapsed = collapse && sheet.value ? pivotGroupPrefixes(sheet.value.getSheetId(), 0) : []
}

const aliasCount = computed(() => Object.values(aliases.value).reduce((n, m) => n + Object.keys(m).length, 0)
  + [...areas.rows, ...areas.columns, ...areas.values].filter(i => i.label && i.agg !== 'calc').length)

function clearAliases() {
  aliases.value = {}
  for (const it of [...areas.rows, ...areas.columns, ...areas.values]) {
    // 计算值的名称放在 label 里，不是别名
    if (it.agg !== 'calc')
      delete it.label
  }
}

// ── 计算值 ──
const calcDialog = reactive({ visible: false, editing: null as Item | null })
/** 公式里可以引用的值：透视表里普通的汇总值（用它们的表头文字） */
const calcNames = computed(() => areas.values.filter(v => v.agg !== 'calc').map(valueLabel))

function openCalc(it: Item | null) {
  calcDialog.editing = it
  calcDialog.visible = true
}

// ── 计算项（行 / 列字段的取值之间的运算）──
const itemDialog = reactive({ visible: false, editing: null as CalcItemDef | null })
// 列字段排在前面：计算项最常用在列上（期末 = 期初 + 入库 - 出库）
const itemFields = computed(() => [...areas.columns, ...areas.rows].map(i => i.column))

/** 某个行 / 列字段下现有的取值（取自最近一次计算的结果，不含计算项自己） */
function namesOfField(field: string): string[] {
  const r = lastResult.value
  if (!r)
    return []
  const mine = new Set(calcItems.value.filter(c => c.field === field).map(c => c.name))
  const out = new Set<string>()
  const ri = r.rowFields.indexOf(field)
  if (ri >= 0) {
    for (const row of r.rows) {
      const key = row.keys[ri]
      if (key != null)
        out.add(key)
    }
  }
  const ci = r.columnFields.indexOf(field)
  if (ci >= 0) {
    for (const ck of r.columnKeys) {
      const key = ck[ci]
      if (key != null)
        out.add(key)
    }
  }
  return [...out].filter(n => !mine.has(n))
}

function openItem(it: CalcItemDef | null) {
  itemDialog.editing = it
  itemDialog.visible = true
}

function saveItem(v: CalcItemDef) {
  const it = itemDialog.editing
  if (it)
    Object.assign(it, v)
  else
    calcItems.value = [...calcItems.value, v]
}

function removeItem(it: CalcItemDef) {
  calcItems.value = calcItems.value.filter(x => x !== it)
}

function saveCalc(v: { label: string, formula: string, format: 'money' | 'number' | 'percent' }) {
  const it = calcDialog.editing
  if (it) {
    Object.assign(it, v)
  }
  else {
    areas.values.push({ uid: `v-${Math.random().toString(36).slice(2, 8)}`, column: '（计算值）', kind: 'number', agg: 'calc', show: 'raw', ...v })
  }
}

// 在透视表格子里折叠 / 改名后同步到面板（不重复计算）
const offChanged = onPivotChanged((sheetId, spec, result) => {
  if (sheetId !== sheet.value?.getSheetId())
    return
  loadSpec(spec)
  lastResult.value = result
  emit('changed')
})
onBeforeUnmount(offChanged)

// 换数据集：清空字段
watch(datasetId, (id, old) => {
  if (!loadingSpec && old && id !== old) {
    areas.filters = []
    areas.columns = []
    areas.rows = []
    areas.values = []
    calcItems.value = []
  }
})

/** 跟随当前页签：是透视表就载入它的定义 */
function syncActiveSheet() {
  const s = props.univerApi?.getActiveWorkbook()?.getActiveSheet() ?? null
  const spec = getPivotSpec(s)
  if (s && spec && s.getSheetId() === sheet.value?.getSheetId())
    return
  sheet.value = spec ? s : null
  loadSpec(spec)
}

let disposer: { dispose: () => void } | null = null
watch(() => props.univerApi, (api) => {
  disposer?.dispose()
  if (api)
    disposer = api.addEvent(api.Event.ActiveSheetChanged, () => syncActiveSheet())
  syncActiveSheet()
}, { immediate: true })
onBeforeUnmount(() => disposer?.dispose())

// ── 新建 ──
const newDatasetId = ref('')
async function createNew() {
  const api = props.univerApi
  const ds = props.datasets.find(d => d.id === (newDatasetId.value || props.datasets[0]?.id))
  if (!api || !ds)
    return
  const s = await createPivotSheet(api, { version: 1, datasetId: ds.id, datasetName: ds.name, request: { rows: [], columns: [], values: [] } })
  sheet.value = s
  loadSpec(getPivotSpec(s))
  s.getRange(0, 0).setValue(tr('数据透视表 · {0}：从右侧字段列表把字段拖入「行」「列」「值」', [ds.name]))
  emit('changed')
}

// ── 筛选条件 ──
const filterMenu = shallowRef<{ item: Item, column: GridColumn, rect: DOMRect } | null>(null)
function openFilter(item: Item, e: MouseEvent) {
  const col = dataset.value?.columns.find(c => c.title === item.column)
  if (!col)
    return
  filterMenu.value = { item, column: { ...col }, rect: (e.currentTarget as HTMLElement).getBoundingClientRect() }
}
function onFilterApply(f: DatasetFilter | null) {
  const target = areas.filters.find(i => i.uid === filterMenu.value?.item.uid)
  if (target)
    target.filter = f ? { ...f, column: target.column } : null
  filterMenu.value = null
}
const otherFilters = computed(() => ({
  filters: areas.filters.filter(i => i.uid !== filterMenu.value?.item.uid).map(i => i.filter).filter((f): f is DatasetFilter => !!f),
}))
function filterSummary(f: DatasetFilter | null | undefined) {
  if (!f)
    return '全部'
  if (f.op === 'in')
    return f.values.length === 1 ? (f.values[0] || tr('（空白）')) : tr('{0} 项', [f.values.length])
  if (f.op === 'between')
    return `${f.min ?? '…'} ~ ${f.max ?? '…'}`
  if (f.op === 'contains')
    return tr('包含 {0}', [f.value])
  if (f.op === 'prefixes')
    return tr('{0} 项…', [f.values.length])
  return `${f.value}…`
}
</script>

<template>
  <div class="pivot-panel">
    <!-- 当前页签不是透视表 -->
    <div v-if="!sheet" class="flex flex-col gap-3 text-[13px]">
      <div class="text-gray-600 leading-5">
        {{ $t('数据透视表直接在服务端对数据集计算（千万行秒级），结果写入新页签；任意格子可下钻到明细并自动核对。') }}
      </div>
      <el-select v-model="newDatasetId" :placeholder="$t('选择数据集')" size="small">
        <el-option v-for="d in datasets" :key="d.id" :label="$t('{0}（{1} 行）', [d.name, d.rowCount.toLocaleString()])" :value="d.id" />
      </el-select>
      <el-button type="primary" size="small" :disabled="!datasets.length" @click="createNew">
        {{ $t('新建数据透视表') }}
      </el-button>
      <div class="text-xs text-gray-400">
        {{ $t('切换到已有的透视表页签，可在这里继续调整字段') }}
      </div>
    </div>

    <template v-else>
      <div class="flex items-center gap-2 text-xs text-gray-500">
        <span>{{ $t('数据集') }}</span>
        <el-select v-model="datasetId" size="small" class="flex-1">
          <el-option v-for="d in datasets" :key="d.id" :label="d.name" :value="d.id" />
        </el-select>
      </div>

      <PivotFieldList v-model:search="fieldSearch" :fields="fields" :used="used" @add="addTo" />

      <!-- 空白时：一键起步 -->
      <div v-if="isBlank && suggestions.length" class="flex flex-col gap-1.5 rounded-md bg-[#F4F9F6] p-2.5" data-testid="pivot-suggestions">
        <div class="text-xs text-gray-600">
          {{ $t('不知道从哪开始？点一个：') }}
        </div>
        <button v-for="sg in suggestions" :key="sg.label" class="suggest" @click="sg.apply">
          {{ sg.label }}
        </button>
      </div>

      <!-- 四个区域 -->
      <div v-for="a in AREAS" :key="a.key" class="section">
        <div class="section-title">
          {{ a.label }}
        </div>
        <draggable v-model="areas[a.key]" group="pivot" item-key="uid" class="drop" :class="{ empty: !areas[a.key].length }" @change="normalize">
          <template #item="{ element }">
            <div class="chip" :class="{ 'flex-wrap': a.key === 'values' }">
              <span v-if="element.agg === 'calc'" class="min-w-0 flex-1 truncate" :title="$t('计算值：{0}', [element.formula])">
                <span class="text-[#8250df]">ƒ</span> {{ element.label }}
              </span>
              <span v-else class="min-w-0 flex-1 truncate" :title="element.label ? $t('{0}（显示为「{1}」）', [element.column, element.label]) : element.column">
                {{ element.column }}<span v-if="element.label" class="text-gray-400"> → {{ element.label }}</span>
              </span>
              <span v-if="element.agg === 'calc'" class="cursor-pointer text-[#217346] hover:underline" :title="$t('修改公式')" @click="openCalc(element)">{{ $t('编辑') }}</span>
              <el-select v-else-if="a.key === 'values'" v-model="element.agg" size="small" class="!w-26">
                <el-option v-for="o in AGG_OPTIONS.filter(o => !o.numeric || isNumeric(element.kind))" :key="o.value" :label="o.label" :value="o.value" />
              </el-select>
              <div v-if="a.key === 'values' && element.agg !== 'calc'" class="order-last w-full flex items-center gap-1.5 pt-1">
                <span class="text-gray-400">{{ $t('显示为') }}</span>
                <el-select v-model="element.show" size="small" class="!w-28" :class="{ 'show-active': element.show && element.show !== 'raw' }">
                  <el-option v-for="o in SHOW_OPTIONS" :key="o.value" :label="o.label" :value="o.value" :title="o.hint" />
                </el-select>
                <el-dropdown trigger="click" size="small" @command="(sh: PivotShow) => addShowCopy(element, sh)">
                  <span class="cursor-pointer text-[#217346] hover:underline" :title="$t('再加一个同字段的值，显示为占比 / 同比 / 环比')">{{ $t('+ 对比') }}</span>
                  <template #dropdown>
                    <el-dropdown-menu>
                      <el-dropdown-item v-for="o in SHOW_OPTIONS.filter(o => o.value !== 'raw')" :key="o.value" :command="o.value">
                        {{ o.label }}
                      </el-dropdown-item>
                    </el-dropdown-menu>
                  </template>
                </el-dropdown>
              </div>
              <el-select v-if="(a.key === 'rows' || a.key === 'columns') && canGroupByDate(element)" v-model="element.granularity" size="small" clearable :placeholder="$t('原值')" class="!w-22">
                <el-option v-for="g in GRANULARITY_OPTIONS" :key="g.value" :label="g.label" :value="g.value" />
              </el-select>
              <button v-if="a.key === 'filters'" class="filter-btn" @click="openFilter(element, $event)">
                {{ filterSummary(element.filter) }} ▾
              </button>
              <span class="remove" :title="$t('移除')" @click="remove(a.key, element.uid)">×</span>
            </div>
          </template>
          <template #footer>
            <div v-if="!areas[a.key].length" class="placeholder">
              {{ a.hint }}
            </div>
          </template>
        </draggable>
        <button v-if="a.key === 'values'" class="add-calc" :title="$t('用公式组合已有的值，如毛利率、客单价')" @click="openCalc(null)">
          {{ $t('+ 计算值（公式）') }}
        </button>
      </div>

      <!-- 计算项：在行 / 列字段的取值之间运算（期末 = 期初 + 入库 - 出库） -->
      <div v-if="areas.rows.length || areas.columns.length" class="section" data-testid="pivot-items">
        <div class="section-title">
          {{ $t('计算项') }}
          <span class="text-gray-400 font-normal">{{ $t('在字段的取值之间运算') }}</span>
        </div>
        <div v-for="c in calcItems" :key="`${c.field}:${c.name}`" class="chip">
          <span class="min-w-0 flex-1 truncate" :title="`${c.field}：${c.name} = ${c.formula}`">
            <span class="text-[#8250df]">ƒ</span> {{ c.name }} <span class="text-gray-400">= {{ c.formula }}（{{ c.field }}）</span>
          </span>
          <span class="cursor-pointer text-[#217346] hover:underline" @click="openItem(c)">{{ $t('编辑') }}</span>
          <span class="remove" :title="$t('移除')" @click="removeItem(c)">×</span>
        </div>
        <button class="add-calc" :title="$t('如列字段「单据类型」里新增「期末 = 期初 + 入库 - 出库」')" @click="openItem(null)">
          {{ $t('+ 计算项') }}
        </button>
      </div>

      <PivotOptions v-model:options="options" v-model:display="display" :rows-count="areas.rows.length" :alias-count="aliasCount" @collapse-all="collapseAll" @clear-aliases="clearAliases" />

      <div class="status">
        <BusyInline v-if="loading" :loading="loading" cancellable @cancel="cancelApply" />
        <span v-else-if="error" class="text-[#B91C1C]">{{ error }}</span>
        <span v-else-if="lastResult" class="text-gray-500">
          {{ $t('{0} 行 · 计算 {1} ms', [lastResult.leafRows.toLocaleString(), lastResult.elapsedMs]) }}
          <template v-if="lastResult.firstLevelCount != null && options.top">{{ $t('（共 {0} 项）', [lastResult.firstLevelCount]) }}</template>
          <template v-if="lastResult.baseField && usesPeriod"> {{ $t('· 同比 / 环比按「{0}」', [lastResult.baseField]) }}</template>
        </span>
        <el-button size="small" link type="primary" class="ml-auto" :disabled="loading" @click="apply">
          {{ $t('刷新') }}
        </el-button>
        <el-button size="small" link type="primary" :disabled="!canConvert || loading" :loading="converting" :title="$t('把透视结果按行展平成一个数据集：可按值筛选、排序、加计算列，随源数据自动更新')" data-testid="pivot-to-table" @click="convertToTable">
          {{ $t('转为数据表') }}
        </el-button>
      </div>
      <div class="text-xs text-gray-400 leading-5">
        {{ $t('选中透视表中的格子，点顶部「查看明细」可下钻到组成它的数据并自动核对。双击行 / 列标签或表头可直接改名，清空即恢复原名') }}
      </div>
      <div v-if="display.layout === 'compact' && areas.rows.length >= 2" class="text-xs text-gray-400 leading-5">
        {{ $t('点透视表最左侧的 ▾ / ▸ 折叠或展开分组；勾选「小计」时分组行显示该组合计') }}
      </div>
      <div v-if="usesPeriod" class="text-xs text-gray-400 leading-5">
        {{ $t('同比 / 环比：日期字段放在行或列（按年 / 月 / 日分组）；筛选了日期时，基期仍按上年同期 / 上一期取数（不受该筛选限制），基期没有数据时显示为「-」') }}
      </div>
    </template>

    <PivotItemDialog v-model:visible="itemDialog.visible" :initial="itemDialog.editing" :fields="itemFields" :names-of="namesOfField" @save="saveItem" />

    <PivotCalcDialog
      v-model:visible="calcDialog.visible"
      :initial="calcDialog.editing ? { label: calcDialog.editing.label ?? '', formula: calcDialog.editing.formula ?? '', format: calcDialog.editing.format ?? 'number' } : null"
      :names="calcNames"
      @save="saveCalc"
    />

    <FilterMenu
      v-if="filterMenu && dataset"
      :dataset-id="dataset.id"
      :column="filterMenu.column"
      :query="otherFilters"
      :current="filterMenu.item.filter ?? null"
      :sort-desc="null"
      :anchor="filterMenu.rect"
      hide-sort
      @apply="onFilterApply"
      @sort="() => {}"
      @close="filterMenu = null"
    />
  </div>
</template>

<style scoped lang="scss">
.pivot-panel {
  @apply flex flex-col gap-3 text-[13px];
}

.section {
  @apply flex flex-col gap-1;
}

.section-title {
  @apply flex items-center justify-between text-xs font-semibold text-gray-600;
}

.drop {
  @apply min-h-9 rounded border border-[#d1d5db] border-dashed bg-[#FAFAFA] p-1;

  &.empty {
    @apply flex items-center justify-center;
  }
}

.chip {
  @apply mb-1 flex cursor-grab items-center gap-1 rounded border border-[#d1d5db] border-solid bg-white px-1.5 py-0.5 last:mb-0;
}

.placeholder {
  @apply text-xs text-gray-400;
}

.add-calc {
  @apply mt-1 cursor-pointer self-start rounded border-none bg-transparent px-0 py-0.5 text-xs text-[#217346] hover:underline;
}

.suggest {
  @apply cursor-pointer rounded border border-[#cfe5d8] border-solid bg-white px-2.5 py-1.5 text-left text-[13px] text-[#0E5A31] hover:border-[#217346];
}

.remove {
  @apply cursor-pointer px-1 text-gray-400 hover:text-[#B91C1C];
}

.filter-btn {
  @apply max-w-28 truncate rounded border-none bg-[#EEF2FF] px-1.5 text-xs text-[#3730A3];
}

.status {
  @apply flex items-center gap-2 border-t border-[#eee] border-t-solid pt-2 text-xs;
}
.show-active :deep(.#{$namespace}-select__wrapper) {
  box-shadow: 0 0 0 1px #217346 inset;
}
</style>
