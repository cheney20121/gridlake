<script setup lang="ts">
import type { PivotDisplay } from '@/features/excel/pivot/pivot-sheet'
import { SORT_OPTIONS } from './constants'

/** 透视表选项：排序、前 N 项、小计 / 总计、版式（表格 / 紧凑）、别名 */
defineProps<{
  /** 行区域里的字段个数（紧凑版式 ≥ 2 时才有折叠 / 展开） */
  rowsCount: number
  aliasCount: number
}>()

const emit = defineEmits<{
  (e: 'collapseAll', collapse: boolean): void
  (e: 'clearAliases'): void
}>()

const options = defineModel<{ sort: string, top: number | undefined, subtotals: boolean, grandTotals: boolean }>('options', { required: true })
const display = defineModel<Required<PivotDisplay>>('display', { required: true })
</script>

<template>
  <div class="section">
    <div class="section-title">
      选项
    </div>
    <div class="grid grid-cols-[56px_1fr] items-center gap-x-2 gap-y-1.5 text-xs text-gray-600">
      <span>排序</span>
      <el-select v-model="options.sort" size="small">
        <el-option v-for="o in SORT_OPTIONS" :key="o.value" :label="o.label" :value="o.value" />
      </el-select>
      <span>前 N 项</span>
      <el-input-number v-model="options.top" size="small" :min="1" :max="10000" :controls="false" placeholder="不限（按第一个行字段、第一个值）" />
      <span>汇总</span>
      <div class="flex gap-3">
        <el-checkbox v-model="options.subtotals" size="small">
          小计
        </el-checkbox>
        <el-checkbox v-model="options.grandTotals" size="small">
          总计
        </el-checkbox>
      </div>
      <span>版式</span>
      <el-radio-group v-model="display.layout" size="small">
        <el-radio-button value="tabular" title="每个行字段一列，适合复制、再加工">
          表格
        </el-radio-button>
        <el-radio-button value="compact" title="一列标签按层级缩进，分组可折叠展开，适合阅读和汇报">
          紧凑
        </el-radio-button>
      </el-radio-group>
      <template v-if="display.layout === 'tabular'">
        <span />
        <el-checkbox v-model="display.repeatLabels" size="small" title="每一行都显示上级标签">
          重复显示上级标签
        </el-checkbox>
      </template>
      <template v-else-if="rowsCount >= 2">
        <span />
        <div class="flex items-center gap-3">
          <el-button link size="small" type="primary" @click="emit('collapseAll', true)">
            全部折叠
          </el-button>
          <el-button link size="small" type="primary" :disabled="!display.collapsed.length" @click="emit('collapseAll', false)">
            全部展开
          </el-button>
        </div>
      </template>
      <template v-if="aliasCount">
        <span>别名</span>
        <div class="flex items-center gap-2">
          <span class="text-gray-500">已改 {{ aliasCount }} 处</span>
          <el-button link size="small" type="primary" @click="emit('clearAliases')">
            恢复原名
          </el-button>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped lang="scss">
.section {
  @apply flex flex-col gap-1;
}

.section-title {
  @apply flex items-center justify-between text-xs font-semibold text-gray-600;
}
</style>
