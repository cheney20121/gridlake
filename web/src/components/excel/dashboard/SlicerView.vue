<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { DatePreset, SlicerSpec } from '@/features/excel/dashboard/types'
import { computed, onBeforeUnmount, onMounted, ref, shallowRef } from 'vue'
import { getDistinctApi } from '@/api/dataset'
import { chartBus, sheetIdOf, updateFloat } from '@/features/excel/chart/chart-manager'
import { dashboardFiltersFor } from '@/features/excel/dashboard/cross-filter'
import { saveWidgetImage } from '@/features/excel/dashboard/export-image'
import { slicerRange } from '@/features/excel/dashboard/slicer'
import { DATE_PRESETS } from '@/features/excel/dashboard/types'

/**
 * 切片器：按值多选，或按日期范围（相对时间 / 自定义起止）
 * 选择写回定义（随工作簿保存）并通知同一工作表的部件重新取数；选项按其他切片器的选择联动缩小
 */
const props = defineProps<{
  spec: SlicerSpec
  api: FUniver
}>()

const spec = shallowRef<SlicerSpec>(props.spec)
const options = ref<{ value: string, count: number }[]>([])
const total = ref(0)
const search = ref('')
const loading = ref(false)
const error = ref('')
let controller: AbortController | null = null
const offs: (() => void)[] = []

const selected = computed(() => new Set(spec.value.values))
const shown = computed(() => {
  const kw = search.value.trim().toLowerCase()
  return kw ? options.value.filter(o => o.value.toLowerCase().includes(kw)) : options.value
})
const active = computed(() => spec.value.mode === 'date' ? !!slicerRange(spec.value) : spec.value.values.length > 0)
const range = computed(() => slicerRange(spec.value))

async function loadOptions() {
  if (spec.value.mode !== 'values')
    return
  controller?.abort()
  const c = new AbortController()
  controller = c
  loading.value = true
  try {
    const filters = dashboardFiltersFor(props.api, sheetIdOf(props.api, spec.value.id), spec.value.datasetId, spec.value.id)
    const r = await getDistinctApi(spec.value.datasetId, { column: spec.value.column, filters }, c.signal)
    options.value = r.values
    total.value = r.totalDistinct
    error.value = ''
  }
  catch (e) {
    if (!c.signal.aborted)
      error.value = e instanceof Error ? e.message : String(e)
  }
  finally {
    if (controller === c)
      loading.value = false
  }
}

/** 写回定义并通知同一工作表的部件 */
function commit(next: SlicerSpec) {
  spec.value = next
  try {
    updateFloat(props.api, next)
  }
  catch {
    return
  }
  const sheetId = sheetIdOf(props.api, next.id)
  if (sheetId)
    chartBus.emit('slicer', sheetId)
}

function toggle(v: string) {
  const set = new Set(spec.value.values)
  if (set.has(v))
    set.delete(v)
  else
    set.add(v)
  commit({ ...spec.value, values: [...set] })
}

function only(v: string) {
  commit({ ...spec.value, values: [v] })
}

function clear() {
  commit({ ...spec.value, values: [], range: null })
}

function setPreset(p: DatePreset) {
  commit({ ...spec.value, range: spec.value.range?.preset === p ? null : { preset: p } })
}

function setCustom(which: 'min' | 'max', v: string) {
  const r = slicerRange(spec.value)
  commit({ ...spec.value, range: { preset: null, min: which === 'min' ? v : (spec.value.range?.min ?? r?.min ?? null), max: which === 'max' ? v : (spec.value.range?.max ?? r?.max ?? null) } })
}

async function saveImage(e: MouseEvent) {
  e.stopPropagation()
  try {
    ElMessage.success(`已保存图片：${await saveWidgetImage(e, '.slicer', spec.value.title || spec.value.column)}`)
  }
  catch (err) {
    ElMessage.error(`保存图片失败：${err instanceof Error ? err.message : String(err)}`)
  }
}

function edit(e: MouseEvent) {
  e.stopPropagation()
  chartBus.emit('select', { id: spec.value.id, kind: 'slicer' })
}

onMounted(() => {
  loadOptions()
  offs.push(
    chartBus.on('spec', (s: SlicerSpec) => {
      if (s.id !== spec.value.id)
        return
      const reload = s.datasetId !== spec.value.datasetId || s.column !== spec.value.column || s.mode !== spec.value.mode
      spec.value = s
      if (reload)
        loadOptions()
    }),
    // 其他切片器变化：联动更新选项
    chartBus.on('slicer', (sheetId: string) => {
      if (sheetId === sheetIdOf(props.api, spec.value.id))
        loadOptions()
    }),
    chartBus.on('refresh-dataset', (id?: string) => {
      if (!id || id === spec.value.datasetId)
        loadOptions()
    }),
  )
})

onBeforeUnmount(() => {
  offs.forEach(off => off())
  controller?.abort()
})
</script>

<template>
  <div class="slicer" @dblclick="edit">
    <div class="head">
      <span class="title" :title="spec.title || spec.column">{{ spec.title || spec.column }}</span>
      <span v-if="active" class="clear" title="清除筛选（显示全部）" @pointerdown.stop @mousedown.stop @click.stop="clear">清除</span>
      <span class="edit" title="保存为图片" @pointerdown.stop @mousedown.stop @click="saveImage">📷</span>
      <span class="edit" title="编辑切片器" @pointerdown.stop @mousedown.stop @click="edit">⚙</span>
    </div>

    <template v-if="spec.mode === 'values'">
      <input
        v-if="options.length > 8"
        v-model="search"
        class="search"
        placeholder="搜索"
        @pointerdown.stop @mousedown.stop
        @keydown.stop
      >
      <div class="list" :class="{ 'opacity-50': loading && options.length }" @pointerdown.stop @mousedown.stop @wheel.stop>
        <div v-if="error" class="state text-[#B91C1C]">
          {{ error }}
        </div>
        <div v-else-if="!options.length && !loading" class="state">
          没有可选的值
        </div>
        <div
          v-for="o in shown"
          :key="o.value"
          class="item"
          :class="{ on: selected.has(o.value) }"
          :title="`点选：多选；双击：只看「${o.value || '（空白）'}」`"
          @click="toggle(o.value)"
          @dblclick.stop="only(o.value)"
        >
          <span class="box">{{ selected.has(o.value) ? '✓' : '' }}</span>
          <span class="label">{{ o.value || '（空白）' }}</span>
          <span class="count">{{ o.count.toLocaleString() }}</span>
        </div>
        <div v-if="total > options.length" class="state">
          共 {{ total.toLocaleString() }} 项，仅显示前 {{ options.length }} 项，可搜索
        </div>
      </div>
    </template>

    <template v-else>
      <div class="presets" @pointerdown.stop @mousedown.stop>
        <span v-for="p in DATE_PRESETS" :key="p.value" class="chip" :class="{ on: spec.range?.preset === p.value }" @click="setPreset(p.value)">{{ p.label }}</span>
      </div>
      <div class="custom" @pointerdown.stop @mousedown.stop @keydown.stop>
        <input type="date" :value="range?.min ?? ''" @change="setCustom('min', ($event.target as HTMLInputElement).value)">
        <span>~</span>
        <input type="date" :value="range?.max ?? ''" @change="setCustom('max', ($event.target as HTMLInputElement).value)">
      </div>
      <div class="range-text">
        {{ range ? `${range.min} ~ ${range.max}` : '全部日期' }}
      </div>
    </template>
  </div>
</template>

<style scoped>
.slicer {
  position: relative;
  width: 100%;
  height: 100%;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px 10px;
  background: #fff;
  border: 1px solid rgba(11, 11, 11, 0.1);
  border-radius: 6px;
  font-family: system-ui, -apple-system, 'PingFang SC', 'Microsoft YaHei', sans-serif;
  font-size: 12px;
  color: #0b0b0b;
  overflow: hidden;
}

.head {
  display: flex;
  align-items: center;
  gap: 8px;
  cursor: move;
}

.title {
  flex: 1;
  min-width: 0;
  font-size: 13px;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.clear {
  color: #217346;
  cursor: pointer;
}

.edit {
  color: #898781;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.15s;
}

.slicer:hover .edit {
  opacity: 1;
}

.search {
  border: 1px solid #e1e0d9;
  border-radius: 4px;
  padding: 3px 6px;
  font-size: 12px;
  outline: none;
}

.search:focus {
  border-color: #217346;
}

.list {
  flex: 1;
  min-height: 0;
  overflow: auto;
  transition: opacity 0.15s;
}

.item {
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 26px;
  padding: 0 4px;
  border-radius: 4px;
  cursor: pointer;
  user-select: none;
}

.item:hover {
  background: #f4f4f1;
}

.item.on {
  background: #e7f4ec;
}

.box {
  width: 14px;
  height: 14px;
  flex: none;
  border: 1px solid #c3c2b7;
  border-radius: 3px;
  font-size: 11px;
  line-height: 13px;
  text-align: center;
  color: #fff;
}

.item.on .box {
  background: #217346;
  border-color: #217346;
}

.label {
  flex: 1;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.count {
  color: #898781;
  font-variant-numeric: tabular-nums;
}

.state {
  padding: 6px 4px;
  color: #898781;
}

.presets {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.chip {
  padding: 2px 8px;
  border: 1px solid #e1e0d9;
  border-radius: 10px;
  cursor: pointer;
  user-select: none;
}

.chip:hover {
  border-color: #217346;
}

.chip.on {
  background: #217346;
  border-color: #217346;
  color: #fff;
}

.custom {
  display: flex;
  align-items: center;
  gap: 4px;
}

.custom input {
  flex: 1;
  min-width: 0;
  border: 1px solid #e1e0d9;
  border-radius: 4px;
  padding: 2px 4px;
  font-size: 12px;
}

.range-text {
  color: #52514e;
  font-variant-numeric: tabular-nums;
}
</style>
