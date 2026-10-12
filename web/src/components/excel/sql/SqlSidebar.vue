<script setup lang="ts">
import type { DatasetInfo } from '@/api/dataset'
import type { SavedQuery, SqlSchedule } from '@/api/sql'
import { computed, ref } from 'vue'
import { q } from '@/features/excel/sql/builder'
import { KIND_LABEL } from './format'

/** 左侧：可搜索的数据集 / 列（点名称插入到编辑器）与已保存的查询 */
const props = defineProps<{
  datasets: DatasetInfo[]
  queries: SavedQuery[]
  current: SavedQuery | null
}>()

const emit = defineEmits<{
  (e: 'insert', text: string): void
  (e: 'open', sq: SavedQuery): void
  (e: 'new'): void
}>()

/** 参数化查询的实例缩进显示在模板下面；模板已删除的实例照常单独列出 */
const ordered = computed(() => {
  const ids = new Set(props.queries.map(q => q.id))
  const top = props.queries.filter(q => !q.templateId || !ids.has(q.templateId))
  return top.flatMap(q => [{ q, indent: false }, ...props.queries.filter(i => i.templateId === q.id).map(i => ({ q: i, indent: true }))])
})

const keyword = defineModel<string>('keyword', { required: true })
const expanded = ref<string | null>(null)

function scheduleText(sq: SavedQuery) {
  const s: SqlSchedule = sq.schedule
  return s.kind === 'daily' ? `每天 ${s.at} 重算` : s.kind === 'after_sync' ? '同步后重算' : '手动重算'
}
</script>

<template>
  <div class="w-60 flex shrink-0 flex-col gap-2 overflow-hidden text-[13px]">
    <el-input v-model="keyword" size="small" clearable placeholder="搜索数据集 / 列" />
    <div class="min-h-0 flex-1 overflow-y-auto">
      <div class="side-title">
        数据集 <span class="text-gray-400">点名称插入</span>
      </div>
      <div v-for="d in datasets" :key="d.id">
        <div class="item">
          <span class="w-4 shrink-0 cursor-pointer text-center text-gray-400" @click="expanded = expanded === d.id ? null : d.id">{{ expanded === d.id ? '▾' : '▸' }}</span>
          <span class="min-w-0 flex-1 cursor-pointer truncate" :title="`插入 ${q(d.name)}`" @click="emit('insert', q(d.name))">{{ d.name }}</span>
          <span class="shrink-0 text-xs text-gray-400">{{ d.rowCount.toLocaleString() }}</span>
        </div>
        <div v-if="expanded === d.id" class="pb-1 pl-5">
          <div v-for="c in d.columns" :key="c.key" class="col" :title="`插入 ${q(c.title)}`" @click="emit('insert', q(c.title))">
            <span class="min-w-0 flex-1 truncate">{{ c.title }}</span>
            <span class="shrink-0 text-xs text-gray-400">{{ KIND_LABEL[c.kind] ?? c.kind }}</span>
          </div>
        </div>
      </div>
      <div class="side-title mt-3 flex items-center">
        <span class="flex-1">已保存的查询</span>
        <el-button link size="small" @click="emit('new')">
          + 新查询
        </el-button>
      </div>
      <div v-if="!queries.length" class="px-1 text-xs text-gray-400">
        还没有保存的查询
      </div>
      <div v-for="{ q: sq, indent } in ordered" :key="sq.id" class="saved" :class="{ on: current?.id === sq.id, indent }" @click="emit('open', sq)">
        <div class="truncate font-medium">
          <span v-if="sq.params.length && !sq.templateId" class="mr-1 rounded bg-[#E7F4EC] px-1 text-[11px] text-[#0E5A31]" title="参数化查询：可用不同的参数值生成多个数据集">参数</span>{{ sq.name }}
        </div>
        <div class="truncate text-xs text-gray-500">
          <span v-if="sq.datasetName">→ {{ sq.datasetName }} · </span>{{ scheduleText(sq) }}
          <span v-if="sq.lastStatus === 'failed'" class="text-[#B91C1C]"> · 上次失败</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.side-title {
  @apply mb-1 px-1 text-xs text-gray-500 font-medium;
}

.item {
  @apply flex items-center gap-1 rounded px-1 py-1;

  &:hover {
    @apply bg-[#f4f4f1];
  }
}

.col {
  @apply flex cursor-pointer items-center gap-1 rounded px-1 py-0.5 text-xs;

  &:hover {
    @apply bg-[#f4f4f1] text-[#217346];
  }
}

.saved {
  @apply mb-1 cursor-pointer rounded px-2 py-1.5;

  &:hover {
    @apply bg-[#f4f4f1];
  }

  &.on {
    @apply bg-[#E7F4EC];
  }

  &.indent {
    @apply ml-4 border-l-2 border-[#d9e6dd] border-l-solid pl-2;
  }
}
</style>
