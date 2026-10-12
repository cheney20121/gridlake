<script setup lang="ts">
import type { DetailColumnDef } from './columns'
import type { DetailRow } from './useDetailWindow'
import { computed } from 'vue'
import { CONTRIBUTION_COLUMN, DETAIL_COLUMNS, formatMoney } from './columns'

const props = defineProps<{
  row: DetailRow
  withContribution: boolean
}>()

const emit = defineEmits<{
  (e: 'close'): void
}>()

/** 基本信息全部展示；金额拆分只展示非零项，方便看出这笔流水由哪些部分构成 */
const groups = computed(() => {
  const cols: DetailColumnDef[] = props.withContribution ? [CONTRIBUTION_COLUMN, ...DETAIL_COLUMNS] : DETAIL_COLUMNS
  const value = (c: DetailColumnDef) => props.row[c.key]
  return [
    { title: '基本信息', items: cols.filter(c => c.kind !== 'money' || c.key === '动账金额' || c === CONTRIBUTION_COLUMN) },
    { title: '金额拆分（非零项）', items: cols.filter(c => c.kind === 'money' && c.key !== '动账金额' && c !== CONTRIBUTION_COLUMN && value(c)) },
  ]
})

function display(c: DetailColumnDef): string {
  const v = props.row[c.key]
  if (v == null || v === '')
    return '—'
  return c.kind === 'money' ? formatMoney(v) : String(v)
}

async function copy(c: DetailColumnDef) {
  const v = props.row[c.key]
  if (v == null || v === '')
    return
  await navigator.clipboard.writeText(String(v))
  ElMessage.success(`已复制${c.title}`)
}
</script>

<template>
  <div class="h-full flex flex-col border-0 border-l border-solid border-[#e6e6e6] pl-3">
    <div class="mb-2 flex items-center justify-between">
      <span class="text-sm font-semibold text-gray-800">第 {{ (row.__index + 1).toLocaleString() }} 行</span>
      <el-button text size="small" @click="emit('close')">
        关闭
      </el-button>
    </div>
    <div class="min-h-0 flex-1 overflow-y-auto">
      <section v-for="g in groups" :key="g.title" class="mb-3">
        <div class="mb-1 text-xs font-semibold text-gray-500">
          {{ g.title }}
        </div>
        <div
          v-for="c in g.items"
          :key="c.key"
          class="flex gap-2 border-0 border-b border-dashed border-[#f0f0f0] py-1 text-[12px]"
        >
          <span class="w-24 shrink-0 text-gray-500">{{ c.title }}</span>
          <span
            class="min-w-0 flex-1 break-all text-gray-800"
            :class="{
              'text-right tabular-nums': c.kind === 'money',
              'text-[#DC2626]': c.kind === 'money' && Number(row[c.key]) < 0,
              'cursor-pointer hover:text-[#107C41]': c.kind === 'id',
              'font-semibold': c === CONTRIBUTION_COLUMN,
            }"
            :title="c.kind === 'id' ? '点击复制' : undefined"
            @click="c.kind === 'id' && copy(c)"
          >{{ display(c) }}</span>
        </div>
      </section>
    </div>
  </div>
</template>
