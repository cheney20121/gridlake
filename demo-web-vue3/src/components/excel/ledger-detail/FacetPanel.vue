<script setup lang="ts">
import type { FacetKey } from './columns'
import type { LedgerFacetItem } from '@/api/ledger'
import { reactive } from 'vue'
import { FACETS, formatMoney } from './columns'

const props = defineProps<{
  facets: Record<string, LedgerFacetItem[]>
  selected: Record<FacetKey, string[]>
  /** 由下钻上下文固定、不需要再筛选的维度 */
  hidden?: FacetKey[]
}>()

const emit = defineEmits<{
  (e: 'toggle', key: FacetKey, value: string): void
  (e: 'clear', key: FacetKey): void
}>()

const COLLAPSED_SIZE = 8
const expanded = reactive<Record<string, boolean>>({})
const search = reactive<Record<string, string>>({})

function visibleItems(dim: string, key: FacetKey): LedgerFacetItem[] {
  const keyword = (search[dim] ?? '').trim()
  let items = props.facets[dim] ?? []
  if (keyword)
    items = items.filter(i => (i.value || '（空）').includes(keyword))
  // 已选中的项即使排在后面也始终显示
  if (!expanded[dim] && items.length > COLLAPSED_SIZE) {
    const chosen = new Set(props.selected[key])
    return items.filter((item, i) => i < COLLAPSED_SIZE || chosen.has(item.value))
  }
  return items
}
</script>

<template>
  <div class="h-full overflow-y-auto pr-1">
    <template v-for="{ dim, key } in FACETS" :key="dim">
      <section v-if="!hidden?.includes(key) && (facets[dim]?.length ?? 0) > 0" class="mb-4">
        <div class="mb-1 flex items-center justify-between">
          <span class="facet-title">{{ dim === '商户主体名称' ? '商户主体' : dim }}</span>
          <button v-if="selected[key].length" class="text-xs text-[#107C41]" @click="emit('clear', key)">
            清除（{{ selected[key].length }}）
          </button>
        </div>
        <el-input
          v-if="(facets[dim]?.length ?? 0) > 12"
          v-model="search[dim]"
          size="small"
          placeholder="搜索"
          clearable
          class="mb-1"
        />
        <ul class="m-0 list-none p-0">
          <li
            v-for="item in visibleItems(dim, key)"
            :key="item.value"
            class="facet-item"
            :class="{ active: selected[key].includes(item.value) }"
            :title="`${item.value || '（空）'}\n${item.count.toLocaleString()} 笔，净额 ${formatMoney(item.net)}`"
            @click="emit('toggle', key, item.value)"
          >
            <span class="min-w-0 flex-1 truncate">{{ item.value || '（空）' }}</span>
            <span class="shrink-0 text-right">
              <span class="block text-[11px] text-gray-400">{{ item.count.toLocaleString() }} 笔</span>
              <span class="block text-[11px]" :class="item.net < 0 ? 'text-[#DC2626]' : 'text-[#107C41]'">{{ formatMoney(item.net) }}</span>
            </span>
          </li>
        </ul>
        <button
          v-if="!search[dim] && (facets[dim]?.length ?? 0) > COLLAPSED_SIZE"
          class="mt-1 text-xs text-gray-500 hover:text-[#107C41]"
          @click="expanded[dim] = !expanded[dim]"
        >
          {{ expanded[dim] ? '收起' : `展开全部 ${facets[dim]?.length} 项` }}
        </button>
      </section>
    </template>
  </div>
</template>

<style scoped lang="scss">
.facet-title {
  @apply text-xs font-semibold text-gray-500;
}

.facet-item {
  @apply flex cursor-pointer items-center gap-2 rounded px-2 py-1 text-[13px] text-gray-700;

  &:hover {
    @apply bg-gray-100;
  }

  &.active {
    @apply bg-[#E7F4EC] text-[#0B5A2E] font-medium;
  }
}
</style>
