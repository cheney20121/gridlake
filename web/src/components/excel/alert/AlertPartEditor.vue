<script setup lang="ts">
import type { AlertAgg, AlertPart } from '@/api/alert'
import type { DatasetColumn } from '@/api/dataset'
import { computed } from 'vue'
import { AGG_LABEL } from '@/api/alert'
import { isNumericKind } from '@/features/excel/dataset/column-kinds'

/** 一个汇总值：对某列求和 / 计数…（计数可不选列） */
const props = defineProps<{
  modelValue: AlertPart
  columns: DatasetColumn[]
  /** 只允许的汇总方式（默认全部） */
  aggs?: AlertAgg[]
}>()

const emit = defineEmits<{ (e: 'update:modelValue', v: AlertPart): void }>()

const aggOptions = computed(() => props.aggs ?? (Object.keys(AGG_LABEL) as AlertAgg[]))

/** 求和 / 平均 / 最小 / 最大只能用数值列 */
const columnOptions = computed(() => {
  const numeric = ['sum', 'avg', 'min', 'max'].includes(props.modelValue.agg)
  return numeric ? props.columns.filter(c => isNumericKind(c.kind)) : props.columns
})

/** 换汇总方式时，已选的列如果不适用（如给文本列求和）就清掉 */
function setAgg(agg: AlertAgg) {
  const col = props.columns.find(c => c.key === props.modelValue.column)
  const ok = !col || !['sum', 'avg', 'min', 'max'].includes(agg) || isNumericKind(col.kind)
  emit('update:modelValue', { agg, column: ok ? props.modelValue.column : null })
}
</script>

<template>
  <div class="w-full flex gap-2">
    <el-select :model-value="modelValue.agg" class="!w-28" @update:model-value="setAgg">
      <el-option v-for="a in aggOptions" :key="a" :label="AGG_LABEL[a]" :value="a" />
    </el-select>
    <el-select
      :model-value="modelValue.column ?? ''"
      class="min-w-0 flex-1"
      filterable
      :clearable="modelValue.agg === 'count'"
      :placeholder="modelValue.agg === 'count' ? $t('行数（可不选列）') : $t('选择列')"
      @update:model-value="emit('update:modelValue', { ...modelValue, column: $event || null })"
    >
      <el-option v-for="c in columnOptions" :key="c.key" :label="c.title" :value="c.key" />
    </el-select>
  </div>
</template>
