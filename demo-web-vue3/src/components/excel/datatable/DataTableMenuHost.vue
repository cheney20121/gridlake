<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { DatasetFilter, DatasetQuery } from '@/api/dataset'
import type { GridColumn } from '@/components/excel/big-grid/types'
import type { DataTableSpec } from '@/features/excel/datatable/types'
import { computed, onBeforeUnmount, shallowRef } from 'vue'
import FilterMenu from '@/components/excel/big-grid/FilterMenu.vue'
import { chartBus, findFloat, updateFloat } from '@/features/excel/chart/chart-manager'
import { tableQuery } from '@/features/excel/datatable/types'

/** 工作表里数据表的列头菜单（排序 / 筛选）：浮动对象里没有 Element Plus 配置，菜单由页面显示 */
const props = defineProps<{
  univerApi: FUniver | null
}>()

const emit = defineEmits<{
  (e: 'changed'): void
}>()

const menu = shallowRef<{ spec: DataTableSpec, column: GridColumn, rect: DOMRect, query: DatasetQuery } | null>(null)

const off = chartBus.on('table-menu', (p: { id: string, column: GridColumn, rect: DOMRect }) => {
  const api = props.univerApi
  const loc = api ? findFloat<DataTableSpec>(api, p.id) : null
  if (!api || !loc)
    return
  // 候选值按其他列的筛选统计（与 Excel 自动筛选一致）
  const query = tableQuery(api, loc.spec, loc.sheet.getSheetId())
  menu.value = { spec: loc.spec, column: p.column, rect: p.rect, query: { ...query, sort: [] } }
})
onBeforeUnmount(off)

const current = computed(() => menu.value?.spec.filters.find(f => f.column === menu.value!.column.key) ?? null)
const sortDesc = computed(() => {
  const s = menu.value?.spec.sort[0]
  return s && s.column === menu.value!.column.key ? s.desc : null
})

function commit(next: DataTableSpec) {
  menu.value = null
  if (!props.univerApi)
    return
  try {
    updateFloat(props.univerApi, next)
    emit('changed')
  }
  catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e))
  }
}

function onApply(filter: DatasetFilter | null) {
  const m = menu.value!
  const col = m.column.key
  commit({ ...m.spec, filters: [...m.spec.filters.filter(f => f.column !== col), ...(filter ? [filter] : [])] })
}

function onSort(desc: boolean | null) {
  const m = menu.value!
  commit({ ...m.spec, sort: desc == null ? [] : [{ column: m.column.key, desc }] })
}
</script>

<template>
  <FilterMenu
    v-if="menu"
    :dataset-id="menu.spec.datasetId"
    :column="menu.column"
    :query="menu.query"
    :current="current"
    :sort-desc="sortDesc"
    :anchor="menu.rect"
    @apply="onApply"
    @sort="onSort"
    @close="menu = null"
  />
</template>
