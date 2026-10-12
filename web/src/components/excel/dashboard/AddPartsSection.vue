<script setup lang="ts">
import type { DatasetInfo } from '@/api/dataset'

/** 「添加到当前工作表」：选数据集，再选要添加的部件类型 */
defineProps<{
  datasets: DatasetInfo[]
}>()

const emit = defineEmits<{
  (e: 'add', kind: 'slicer' | 'kpi' | 'table' | 'datatable'): void
  (e: 'addChart'): void
}>()

const datasetId = defineModel<string>({ required: true })
</script>

<template>
  <div class="section">
    <div class="section-title">
      {{ $t('添加到当前工作表') }}
    </div>
    <el-select v-model="datasetId" size="small" filterable>
      <el-option v-for="d in datasets" :key="d.id" :label="$t('{0}（{1} 行）', [d.name, d.rowCount.toLocaleString()])" :value="d.id" />
    </el-select>
    <div class="grid grid-cols-5 gap-1.5">
      <button class="add-btn" :title="$t('按字段筛选本页所有部件')" @click="emit('add', 'slicer')">
        <span class="text-base">☰</span>{{ $t('切片器') }}
      </button>
      <button class="add-btn" :title="$t('一个汇总值 + 同比 / 环比 + 趋势')" @click="emit('add', 'kpi')">
        <span class="text-base">№</span>{{ $t('指标卡') }}
      </button>
      <button class="add-btn" :title="$t('柱 / 线 / 饼等图表')" @click="emit('addChart')">
        <span class="text-base">▮▮</span>{{ $t('图表') }}
      </button>
      <button class="add-btn" :title="$t('按字段分组的汇总表，如前 10 的店铺')" @click="emit('add', 'table')">
        <span class="text-base">▦</span>{{ $t('表格') }}
      </button>
      <button class="add-btn" :title="$t('数据集明细：放在选中的单元格处，内部滚动、可排序筛选，公式可按表名引用')" @click="emit('add', 'datatable')">
        <span class="text-base">☷</span>{{ $t('数据表') }}
      </button>
    </div>
    <div class="text-xs text-gray-400 leading-5">
      {{ $t('切片器筛选本页所有部件（数据集里有同名字段即可，可跨数据集）；数据表放在选中的单元格处；拖动部件调整位置，拖边角调整大小') }}
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

.add-btn {
  @apply flex flex-col items-center gap-0.5 rounded border border-[#e5e7eb] border-solid bg-white py-1.5 text-xs text-gray-600 cursor-pointer hover:border-[#217346];
}
</style>
