<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { PivotResult } from '@/api/dataset'
import type { TableCardSpec } from '@/features/excel/dashboard/types'
import { computed, onBeforeUnmount, onMounted, ref, shallowRef } from 'vue'
import { chartBus, sheetIdOf } from '@/features/excel/chart/chart-manager'
import { crossFilterAffecting, dashboardFiltersFor } from '@/features/excel/dashboard/cross-filter'
import { full, loadTable } from '@/features/excel/dashboard/data'
import { saveWidgetImage } from '@/features/excel/dashboard/export-image'
import { t } from '@/i18n'

/** 表格卡片：按一个字段分组汇总（如销售额前 10 的店铺），带合计行；切片器变化时重算 */
const props = defineProps<{
  spec: TableCardSpec
  api: FUniver
}>()

const spec = shallowRef<TableCardSpec>(props.spec)
/** 受图表联动筛选时显示标记 */
const cross = computed(() => crossFilterAffecting(sheetIdOf(props.api, spec.value.id), spec.value.datasetId, spec.value.id))
const result = shallowRef<PivotResult | null>(null)
const loading = ref(false)
const error = ref('')
let controller: AbortController | null = null
const offs: (() => void)[] = []

async function load() {
  controller?.abort()
  const c = new AbortController()
  controller = c
  loading.value = true
  try {
    const extra = dashboardFiltersFor(props.api, sheetIdOf(props.api, spec.value.id), spec.value.datasetId)
    result.value = await loadTable(spec.value, extra, c.signal)
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

function fmt(v: number | null | undefined, vi: number) {
  const info = result.value?.values[vi]
  if (v == null)
    return '—'
  if (info?.kind === 'percent')
    return `${(v * 100).toFixed(1)}%`
  return full(v, info?.kind === 'money')
}

async function saveImage(e: MouseEvent) {
  e.stopPropagation()
  try {
    ElMessage.success(t('已保存图片：{0}', [await saveWidgetImage(e, '.card', spec.value.title || '汇总表')]))
  }
  catch (err) {
    ElMessage.error(t('保存图片失败：{0}', [err instanceof Error ? err.message : String(err)]))
  }
}

function edit(e: MouseEvent) {
  e.stopPropagation()
  chartBus.emit('select', { id: spec.value.id, kind: 'table' })
}

onMounted(() => {
  load()
  offs.push(
    chartBus.on('spec', (s: TableCardSpec) => {
      if (s.id === spec.value.id) {
        spec.value = s
        load()
      }
    }),
    chartBus.on('slicer', (sheetId: string) => {
      if (sheetId === sheetIdOf(props.api, spec.value.id))
        load()
    }),
    chartBus.on('refresh-dataset', (id?: string) => {
      if (!id || id === spec.value.datasetId)
        load()
    }),
  )
})

onBeforeUnmount(() => {
  offs.forEach(off => off())
  controller?.abort()
})
</script>

<template>
  <div class="card" @dblclick="edit">
    <div class="head">
      <span class="title">{{ spec.title || $t('汇总表') }}</span>
      <span v-if="cross" class="cross" :title="$t('按图表点选的「{0}」筛选；在图表上再点一次或点空白处取消', [cross.label])">{{ $t('联动') }}</span>
      <span class="edit" :title="$t('保存为图片')" @pointerdown.stop @mousedown.stop @click="saveImage">📷</span>
      <span class="edit" :title="$t('编辑表格')" @pointerdown.stop @mousedown.stop @click="edit">⚙</span>
    </div>
    <div v-if="error" class="state text-[#B91C1C]">
      {{ error }}
    </div>
    <div v-else-if="result" class="body" :class="{ dim: loading }" @pointerdown.stop @mousedown.stop @wheel.stop>
      <table>
        <thead>
          <tr>
            <th>{{ result.rowFields[0] }}</th>
            <th v-for="(v, i) in result.values" :key="i">
              {{ v.label }}
            </th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(r, ri) in result.rows" :key="ri" :class="{ total: r.kind === 'total' }">
            <td class="key">
              {{ r.kind === 'total' ? (spec.top > 0 ? $t('前 {0} 项合计', [spec.top]) : $t('合计')) : (r.keys[0] ?? $t('（空白）')) }}
            </td>
            <td v-for="(_, vi) in result.values" :key="vi">
              {{ fmt(r.cells[r.cells.length - 1]?.[vi], vi) }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <div v-else class="state">
      {{ $t('正在计算…') }}
    </div>
  </div>
</template>

<style scoped>
.cross {
  flex-shrink: 0;
  padding: 0 5px;
  border-radius: 3px;
  background: #e7f4ec;
  color: #0b5a2e;
  font-size: 11px;
  line-height: 16px;
}

.card {
  width: 100%;
  height: 100%;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
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
  padding: 8px 12px 4px;
}

.title {
  flex: 1;
  min-width: 0;
  font-size: 14px;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.edit {
  color: #898781;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.15s;
}

.card:hover .edit {
  opacity: 1;
}

.body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  transition: opacity 0.15s;
}

.body.dim {
  opacity: 0.5;
}

table {
  width: 100%;
  border-collapse: collapse;
  font-variant-numeric: tabular-nums;
}

th,
td {
  padding: 5px 12px;
  border-bottom: 1px solid #f0efec;
  text-align: right;
  white-space: nowrap;
}

th {
  position: sticky;
  top: 0;
  background: #f9f9f7;
  color: #52514e;
  font-weight: 600;
}

th:first-child,
td.key {
  text-align: left;
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
}

tr.total td {
  font-weight: 600;
  border-top: 1px solid #e1e0d9;
}

.state {
  padding: 12px;
  color: #898781;
}
</style>
