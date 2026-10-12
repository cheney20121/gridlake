<script setup lang="ts">
import type { FloatLocation } from '@/features/excel/chart/chart-manager'
import type { CellTableLocation } from '@/features/excel/datatable/types'
import { CHART_COMPONENT_KEY } from '@/features/excel/chart/types'
import { KPI_KEY, SLICER_KEY, TABLE_KEY } from '@/features/excel/dashboard/types'
import { DATA_TABLE_KEY } from '@/features/excel/datatable/types'
import { t } from '@/i18n'

/** 「本页部件」：当前工作表上的切片器 / 指标卡 / 表格 / 图表 / 数据表，可编辑、删除 */
defineProps<{
  parts: FloatLocation[]
  cellParts: CellTableLocation[]
  editId: string | null
}>()

const emit = defineEmits<{
  (e: 'refresh'): void
  (e: 'edit', p: FloatLocation): void
  (e: 'editCells', id: string): void
  (e: 'remove', id: string): void
}>()

const KIND_LABEL: Record<string, string> = { [SLICER_KEY]: t('切片器'), [KPI_KEY]: t('指标卡'), [TABLE_KEY]: t('表格'), [CHART_COMPONENT_KEY]: t('图表'), [DATA_TABLE_KEY]: t('数据表') }

function titleOf(p: FloatLocation) {
  const s = p.spec as unknown as { title?: string, column?: string, name?: string }
  return s.title || s.name || s.column || KIND_LABEL[p.key] || t('部件')
}
</script>

<template>
  <div class="section">
    <div class="section-title flex items-center justify-between">
      <span>{{ $t('本页部件（{0}）', [parts.length + cellParts.length]) }}</span>
      <el-button link size="small" @click="emit('refresh')">
        {{ $t('刷新列表') }}
      </el-button>
    </div>
    <div v-if="!parts.length && !cellParts.length" class="text-xs text-gray-400">
      {{ $t('还没有部件') }}
    </div>
    <div v-for="p in cellParts" :key="p.spec.id" class="part" :class="{ on: p.spec.id === editId }">
      <span class="tag">{{ $t('数据表 · 单元格') }}</span>
      <span class="min-w-0 flex-1 truncate" :title="p.spec.name">{{ p.spec.name }}</span>
      <el-button link size="small" @click="emit('editCells', p.spec.id)">
        {{ $t('编辑') }}
      </el-button>
      <el-button link size="small" type="danger" @click="emit('remove', p.spec.id)">
        {{ $t('删除') }}
      </el-button>
    </div>
    <div v-for="p in parts" :key="p.spec.id" class="part" :class="{ on: p.spec.id === editId }">
      <span class="tag">{{ KIND_LABEL[p.key] }}</span>
      <span class="min-w-0 flex-1 truncate" :title="titleOf(p)">{{ titleOf(p) }}</span>
      <el-button link size="small" @click="emit('edit', p)">
        {{ $t('编辑') }}
      </el-button>
      <el-button link size="small" type="danger" @click="emit('remove', p.spec.id)">
        {{ $t('删除') }}
      </el-button>
    </div>
  </div>
</template>

<style scoped lang="scss">
.section {
  @apply flex flex-col gap-2 border-t border-[#eef0f3] border-t-solid pt-3;
}

.section-title {
  @apply text-xs text-gray-500 font-medium;
}

.part {
  @apply flex items-center gap-2 rounded px-1 py-0.5 text-xs;

  &.on {
    @apply bg-[#E7F4EC];
  }
}

.tag {
  @apply shrink-0 rounded bg-[#F4F4F1] px-1.5 text-[11px] text-gray-500;
}
</style>
