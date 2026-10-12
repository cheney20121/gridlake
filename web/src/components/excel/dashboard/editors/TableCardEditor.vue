<script setup lang="ts">
import type { DatasetColumn } from '@/api/dataset'
import type { TableCardSpec } from '@/features/excel/dashboard/types'
import { AGGS, SHOWS } from './constants'

/** 表格部件的编辑表单：分组字段、多个汇总值、前 N 项与排序 */
defineProps<{
  columns: DatasetColumn[]
  numeric: DatasetColumn[]
  groupable: DatasetColumn[]
  kindOf: (title: string) => DatasetColumn['kind'] | undefined
}>()

const form = defineModel<TableCardSpec>({ required: true })
</script>

<template>
  <div class="contents">
    <div class="row">
      <span class="label">分组</span>
      <el-select v-model="form.row.column" size="small" filterable class="flex-1">
        <el-option v-for="c in groupable" :key="c.key" :label="c.title" :value="c.title" />
      </el-select>
      <el-select v-if="kindOf(form.row.column) === 'datetime'" v-model="form.row.granularity" size="small" class="!w-20">
        <el-option label="年" value="year" />
        <el-option label="月" value="month" />
        <el-option label="日" value="day" />
      </el-select>
    </div>
    <div v-for="(v, i) in form.values" :key="i" class="row">
      <span class="label">{{ i === 0 ? '数值' : '' }}</span>
      <el-select v-model="v.agg" size="small" class="!w-20">
        <el-option v-for="a in AGGS" :key="a.value" :label="a.label" :value="a.value" />
      </el-select>
      <el-select v-if="v.agg !== 'count'" v-model="v.column" size="small" filterable class="min-w-0 flex-1">
        <el-option v-for="c in (v.agg === 'count_distinct' ? columns : numeric)" :key="c.key" :label="c.title" :value="c.title" />
      </el-select>
      <el-select v-model="v.show" size="small" class="!w-20" placeholder="原值">
        <el-option v-for="s in SHOWS" :key="s.value" :label="s.label" :value="s.value" />
      </el-select>
      <el-button v-if="form.values.length > 1" link size="small" type="danger" @click="form.values.splice(i, 1)">
        ×
      </el-button>
    </div>
    <el-button link size="small" type="primary" class="self-start" @click="form.values.push({ column: numeric[0]?.title ?? null, agg: numeric.length ? 'sum' : 'count' })">
      + 再加一个数值
    </el-button>
    <div class="row">
      <span class="label">前 N 项</span>
      <el-input-number v-model="form.top" :min="0" :max="200" size="small" :controls="false" class="!w-20" />
      <el-radio-group v-model="form.sort" size="small">
        <el-radio value="value">
          按数值
        </el-radio>
        <el-radio value="key">
          按名称
        </el-radio>
      </el-radio-group>
    </div>
    <div class="text-xs text-gray-400">
      同比 / 环比需要分组字段是日期（按年 / 月 / 日）
    </div>
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
