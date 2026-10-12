<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { DatasetCursor, DatasetFilter } from '@/api/dataset'
import type { GridColumn } from '@/components/excel/big-grid/types'
import type { DataTableSpec } from '@/features/excel/datatable/types'
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import { openCursorApi } from '@/api/dataset'
import BigGrid from '@/components/excel/big-grid/BigGrid.vue'
import { compileRules } from '@/components/excel/big-grid/column-style'
import { chartBus, findFloat, updateFloat } from '@/features/excel/chart/chart-manager'
import { datasetById } from '@/features/excel/dashboard/slicer'
import { tableColumns, tableQuery, tableSheetId } from '@/features/excel/datatable/types'
import { recalculateTableFormulas } from '@/features/excel/formula/dataset-functions'

/**
 * 工作表里的数据表：标题栏可拖动、右下角可缩放（浮动对象），表格区域内部滚动、列头排序 / 筛选
 *
 * 浮动对象挂在独立的 Vue 应用里，没有 Element Plus 的全局配置：列头筛选菜单交给页面（table-menu 事件）
 */
const props = defineProps<{
  spec: DataTableSpec
  api: FUniver
}>()

// 以工作簿里的最新定义为准（挂载参数可能是首次创建时的旧定义）
const spec = shallowRef<DataTableSpec>(findFloat<DataTableSpec>(props.api, props.spec.id)?.spec ?? props.spec)
const cursor = shallowRef<DatasetCursor | null>(null)
const loading = ref(false)
const error = ref('')
const keyword = ref(spec.value.keyword ?? '')
let controller: AbortController | null = null
const offs: (() => void)[] = []

const dataset = computed(() => datasetById(spec.value.datasetId))
const columns = computed<GridColumn[]>(() => tableColumns(spec.value, dataset.value).map(c => ({
  key: c.title,
  title: c.title,
  kind: c.kind,
  width: spec.value.widths?.[c.title] ?? c.width,
  numFmt: spec.value.columnStyles?.[c.title]?.numFmt,
  style: compileRules(spec.value.columnStyles?.[c.title]?.rules),
})))
const fields = computed(() => columns.value.map(c => c.key))
/** 列变了要换一个表格实例（行数据按列取） */
const gridKey = computed(() => `${spec.value.datasetId}|${fields.value.join(',')}`)
const sortState = computed(() => spec.value.sort[0] ?? null)
const filtered = computed(() => new Set(spec.value.filters.map(f => f.column)))

let lastQuery = ''
async function load(force = false) {
  const ds = dataset.value
  if (!ds) {
    error.value = '数据集不存在、已删除或在回收站（恢复后自动显示）'
    cursor.value = null
    return
  }
  if (!columns.value.length) {
    error.value = '没有可显示的列（列可能已被删除），请编辑数据表重新选择'
    cursor.value = null
    return
  }
  const query = tableQuery(props.api, spec.value, tableSheetId(props.api, spec.value.id))
  const key = JSON.stringify([ds.id, query])
  if (!force && key === lastQuery && cursor.value)
    return
  lastQuery = key
  controller?.abort()
  const c = new AbortController()
  controller = c
  loading.value = true
  try {
    cursor.value = await openCursorApi(ds.id, query, c.signal)
    error.value = ''
  }
  catch (e) {
    if (!c.signal.aborted) {
      error.value = e instanceof Error ? e.message : String(e)
      lastQuery = ''
    }
  }
  finally {
    if (controller === c)
      loading.value = false
  }
}

function commit(next: DataTableSpec) {
  spec.value = next
  try {
    updateFloat(props.api, next)
  }
  catch {
    // 已被删除
  }
}

function filterLabel(f: DatasetFilter): string {
  switch (f.op) {
    case 'in': {
      const shown = f.values.slice(0, 2).map(v => v || '（空白）').join('、')
      return `${f.column}：${shown}${f.values.length > 2 ? ` 等 ${f.values.length} 项` : ''}`
    }
    case 'between': return `${f.column}：${f.min ?? '…'} ~ ${f.max ?? '…'}`
    case 'contains': return `${f.column} 包含「${f.value}」`
    case 'prefix': return `${f.column} 以「${f.value || '（空白）'}」开头`
    case 'prefixes': return `${f.column} 以「${f.values.map(v => v || '（空白）').join('」「')}」之一开头`
  }
}

function removeFilter(i: number) {
  commit({ ...spec.value, filters: spec.value.filters.filter((_, idx) => idx !== i) })
}

let keywordTimer: ReturnType<typeof setTimeout> | undefined
function onKeyword() {
  clearTimeout(keywordTimer)
  keywordTimer = setTimeout(() => {
    if ((spec.value.keyword ?? '') !== keyword.value.trim())
      commit({ ...spec.value, keyword: keyword.value.trim() || undefined })
  }, 400)
}

function onResize(key: string, w: number) {
  commit({ ...spec.value, widths: { ...spec.value.widths, [key]: Math.round(w) } })
}

function onHeaderMenu(column: GridColumn, rect: DOMRect) {
  chartBus.emit('table-menu', { id: spec.value.id, column, rect })
}

function edit(e: MouseEvent) {
  e.stopPropagation()
  chartBus.emit('select', { id: spec.value.id, kind: 'datatable' })
}

const gridRef = ref<InstanceType<typeof BigGrid>>()

onMounted(() => {
  load()
  // 打开工作簿时单元格里是保存时的结果：重算一次引用数据表的公式（多个数据表合并为一次）
  recalculateTableFormulas()
  offs.push(
    chartBus.on('spec', (s: DataTableSpec) => {
      if (s.id === spec.value.id) {
        spec.value = s
        keyword.value = s.keyword ?? ''
        load()
      }
    }),
    chartBus.on('slicer', (sheetId: string) => {
      if (spec.value.followSlicers !== false && sheetId === tableSheetId(props.api, spec.value.id))
        load()
    }),
    chartBus.on('refresh-dataset', (id?: string) => {
      if (!id || id === spec.value.datasetId)
        load(true)
    }),
  )
})

// 数据集目录刷新（改了计算列、关联定义）：列可能变了
watch(() => JSON.stringify(dataset.value?.columns.map(c => [c.key, c.kind, c.formula]) ?? null), (now, before) => {
  if (before !== undefined && now !== before)
    load(true)
})

onBeforeUnmount(() => {
  offs.forEach(off => off())
  controller?.abort()
  clearTimeout(keywordTimer)
})
</script>

<template>
  <div class="dt" @dblclick.self="edit">
    <div class="head" @dblclick="edit">
      <span class="name" :title="`公式引用：=TSUM(&quot;${spec.name}[列名]&quot;)`">{{ spec.name }}</span>
      <span class="meta">{{ dataset?.name ?? '' }}</span>
      <span class="meta tabular-nums">{{ cursor ? `${cursor.total.toLocaleString()} 行` : '…' }}</span>
      <div class="chips" @pointerdown.stop @mousedown.stop>
        <span v-for="(f, i) in spec.filters" :key="i" class="chip" :title="filterLabel(f)">
          <span class="t">{{ filterLabel(f) }}</span>
          <span class="x" title="去掉这个筛选" @click="removeFilter(i)">×</span>
        </span>
      </div>
      <input
        v-if="dataset?.searchable"
        v-model="keyword"
        class="search"
        placeholder="搜索"
        @input="onKeyword"
        @pointerdown.stop
        @mousedown.stop
        @keydown.stop
      >
      <span class="btn" title="复制选中的单元格" @pointerdown.stop @mousedown.stop @click="gridRef?.copySelection()">复制</span>
      <span class="btn" title="编辑数据表" @pointerdown.stop @mousedown.stop @click="edit">⚙</span>
    </div>
    <div class="body" @pointerdown.stop @mousedown.stop @wheel.stop @keydown.stop @dblclick.stop>
      <BigGrid
        v-if="cursor && !error"
        ref="gridRef"
        :key="gridKey"
        :dataset-id="spec.datasetId"
        :columns="columns"
        :fields="fields"
        :cursor-id="cursor.cursorId"
        :total="cursor.total"
        :sort="sortState"
        :filtered="filtered"
        hide-letters
        @header-menu="onHeaderMenu"
        @resize-column="onResize"
        @expired="load(true)"
      />
      <div v-if="error" class="state err">
        {{ error }}
      </div>
      <div v-else-if="!cursor" class="state">
        正在查询…
      </div>
      <div v-else-if="!cursor.total" class="state">
        没有符合条件的数据
      </div>
      <div v-if="loading && cursor" class="busy" />
    </div>
  </div>
</template>

<style scoped>
.dt {
  width: 100%;
  height: 100%;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  background: #fff;
  border: 1px solid rgba(11, 11, 11, 0.14);
  border-radius: 4px;
  font-family: system-ui, -apple-system, 'PingFang SC', 'Microsoft YaHei', sans-serif;
  font-size: 12px;
  color: #0b0b0b;
  overflow: hidden;
}

.head {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 30px;
  padding: 0 8px;
  background: #f7f7f5;
  border-bottom: 1px solid #e6e6e6;
  cursor: move;
  flex-shrink: 0;
}

.name {
  font-size: 13px;
  font-weight: 600;
  white-space: nowrap;
}

.meta {
  color: #898781;
  white-space: nowrap;
}

.chips {
  flex: 1;
  min-width: 0;
  display: flex;
  gap: 4px;
  overflow: hidden;
  cursor: default;
}

.chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-width: 48px;
  max-width: 220px;
  padding: 1px 6px;
  border-radius: 3px;
  background: #e7f4ec;
  color: #0b5a2e;
  white-space: nowrap;
}

.t {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

.x {
  flex-shrink: 0;
  cursor: pointer;
  color: #52514e;
}

.search {
  width: 120px;
  height: 22px;
  padding: 0 6px;
  border: 1px solid #dcdcd8;
  border-radius: 3px;
  font-size: 12px;
  outline: none;
  cursor: text;
}

.search:focus {
  border-color: #217346;
}

.btn {
  color: #52514e;
  cursor: pointer;
  white-space: nowrap;
}

.btn:hover {
  color: #217346;
}

.body {
  position: relative;
  flex: 1;
  min-height: 0;
  cursor: default;
}

.state {
  position: absolute;
  inset: 40px 0 auto;
  text-align: center;
  color: #898781;
}

.state.err {
  color: #b91c1c;
  padding: 0 16px;
}

.busy {
  position: absolute;
  inset: 0;
  background: rgba(255, 255, 255, 0.45);
  pointer-events: none;
}
</style>
