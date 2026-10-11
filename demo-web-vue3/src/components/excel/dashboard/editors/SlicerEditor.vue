<script setup lang="ts">
import type { DatasetColumn } from '@/api/dataset'
import type { SlicerSpec } from '@/features/excel/dashboard/types'

/** 切片器的编辑表单：选字段（日期字段按相对时间 / 起止日期，其他按值多选） */
const props = defineProps<{
  columns: DatasetColumn[]
  kindOf: (title: string) => DatasetColumn['kind'] | undefined
}>()

const form = defineModel<SlicerSpec>({ required: true })

function onColumn() {
  form.value.mode = props.kindOf(form.value.column) === 'datetime' ? 'date' : 'values'
  form.value.values = []
  form.value.range = null
  form.value.title = form.value.column
}
</script>

<template>
  <div class="row">
    <span class="label">字段</span>
    <el-select v-model="form.column" size="small" filterable class="flex-1" @change="onColumn">
      <el-option v-for="c in columns" :key="c.key" :label="c.title" :value="c.title" />
    </el-select>
  </div>
  <div class="text-xs text-gray-400 leading-5">
    {{ form.mode === 'date' ? '日期字段：按相对时间（本月、近 30 天…）或起止日期筛选' : '按值多选；本页其他切片器的选择会联动缩小可选项' }}
  </div>
</template>

<style scoped lang="scss">
.row {
  @apply flex items-center gap-2;
}

.label {
  @apply w-14 shrink-0 text-xs text-gray-500;
}
</style>
