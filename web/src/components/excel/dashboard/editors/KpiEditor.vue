<script setup lang="ts">
import type { DatasetColumn } from '@/api/dataset'
import type { KpiSpec } from '@/features/excel/dashboard/types'
import { AGGS } from './constants'

/** 指标卡的编辑表单：汇总值、日期字段、同比 / 环比、趋势小图 */
defineProps<{
  columns: DatasetColumn[]
  numeric: DatasetColumn[]
  dates: DatasetColumn[]
}>()

const form = defineModel<KpiSpec>({ required: true })
</script>

<template>
  <div class="contents">
    <div class="row">
      <span class="label">{{ $t('数值') }}</span>
      <el-select v-model="form.value.agg" size="small" class="!w-24">
        <el-option v-for="a in AGGS" :key="a.value" :label="a.label" :value="a.value" />
      </el-select>
      <el-select v-if="form.value.agg !== 'count'" v-model="form.value.column" size="small" filterable class="flex-1">
        <el-option v-for="c in (form.value.agg === 'count_distinct' ? columns : numeric)" :key="c.key" :label="c.title" :value="c.title" />
      </el-select>
      <span v-else class="flex-1 text-xs text-gray-400">{{ $t('按行数') }}</span>
    </div>
    <div class="row">
      <span class="label">{{ $t('日期字段') }}</span>
      <el-select v-model="form.dateColumn" size="small" clearable class="flex-1" :placeholder="$t('无（不比较、无趋势）')">
        <el-option v-for="c in dates" :key="c.key" :label="c.title" :value="c.title" />
      </el-select>
    </div>
    <template v-if="form.dateColumn">
      <div class="row">
        <span class="label">{{ $t('比较') }}</span>
        <el-radio-group v-model="form.compare" size="small">
          <el-radio-button value="none">
            {{ $t('不比较') }}
          </el-radio-button>
          <el-radio-button value="mom">
            {{ $t('环比') }}
          </el-radio-button>
          <el-radio-button value="yoy">
            {{ $t('同比') }}
          </el-radio-button>
        </el-radio-group>
      </div>
      <div class="row">
        <span class="label">{{ $t('按') }}</span>
        <el-radio-group v-model="form.granularity" size="small">
          <el-radio-button value="day">
            {{ $t('日') }}
          </el-radio-button>
          <el-radio-button value="month">
            {{ $t('月') }}
          </el-radio-button>
          <el-radio-button value="year">
            {{ $t('年') }}
          </el-radio-button>
        </el-radio-group>
      </div>
      <div class="row">
        <el-checkbox v-model="form.sparkline" size="small">
          {{ $t('趋势小图（最近 12 期）') }}
        </el-checkbox>
      </div>
    </template>
    <div class="row">
      <span class="label">{{ $t('上升表示') }}</span>
      <el-radio-group v-model="form.upIsGood" size="small">
        <el-radio-button :value="true">
          {{ $t('好（销售额）') }}
        </el-radio-button>
        <el-radio-button :value="false">
          {{ $t('坏（成本、退款）') }}
        </el-radio-button>
      </el-radio-group>
    </div>
    <div class="text-xs text-gray-400 leading-5">
      {{ $t('没有按日期筛选时显示最近一期（如本月）并与上期比较；本页有日期切片器时按切片器的范围计算') }}
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
