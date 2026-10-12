<script setup lang="ts">
import type { WorkbookMeta } from '@/api/workbook'
import { Search } from '@element-plus/icons-vue'
import WorkbookShare from '@/components/excel/workbook/WorkbookShare.vue'

/** 工作簿列表（最近 / 已加星标共用）：搜索、星标、所有者与共享、修改时间、大小 */
defineProps<{
  items: WorkbookMeta[]
  loading: boolean
  /** 开启登录后才显示所有者 / 共享列 */
  authRequired: boolean
  title: string
  emptyText: string
}>()

const emit = defineEmits<{
  (e: 'open', id: string): void
  (e: 'star', w: WorkbookMeta): void
  (e: 'changed', w: WorkbookMeta, meta: WorkbookMeta): void
}>()

const keyword = defineModel<string>('keyword', { required: true })

function relativeTime(iso: string) {
  const diff = (Date.now() - new Date(iso).getTime()) / 1000
  if (diff < 60)
    return '刚刚'
  if (diff < 3600)
    return `${Math.floor(diff / 60)} 分钟前`
  if (diff < 86400)
    return `${Math.floor(diff / 3600)} 小时前`
  if (diff < 86400 * 7)
    return `${Math.floor(diff / 86400)} 天前`
  return iso.slice(0, 10)
}

function size(bytes: number) {
  if (bytes >= 1024 * 1024)
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${Math.max(1, Math.round(bytes / 1024))} KB`
}
</script>

<template>
  <div>
    <div class="mb-3 flex items-center gap-3">
      <h2 class="section-title">
        {{ title }}
      </h2>
      <span class="pill">{{ $t('{0} 个', [items.length]) }}</span>
      <div class="flex-1" />
      <el-input v-model="keyword" clearable :placeholder="$t('搜索工作簿')" :prefix-icon="Search" class="!w-60" />
    </div>

    <div v-loading="loading" class="table-card">
      <div class="list-row head">
        <span class="w-6" />
        <span class="min-w-40 flex-1">{{ $t('名称') }}</span>
        <span v-if="authRequired" class="w-36">{{ $t('所有者 / 共享') }}</span>
        <span class="hidden w-28 lg:block">{{ $t('修改人') }}</span>
        <span class="w-28">{{ $t('修改时间') }}</span>
        <span class="hidden w-20 text-right lg:block">{{ $t('大小') }}</span>
      </div>
      <div v-for="w in items" :key="w.id" class="list-row group" @click="emit('open', w.id)">
        <button type="button" class="star w-6" :class="{ on: w.starred }" :title="w.starred ? $t('取消星标') : $t('加星标')" @click.stop="emit('star', w)">
          {{ w.starred ? '★' : '☆' }}
        </button>
        <span class="min-w-40 flex flex-1 items-center gap-2.5">
          <span class="file-tile">
            <svg viewBox="0 0 16 16" width="14" height="14"><path d="M2.5 2.5h11v11h-11z" fill="none" stroke="#fff" stroke-width="1.2" /><path d="M2.5 6h11M2.5 9.5h11M6.5 2.5v11" stroke="#fff" stroke-width="1" /></svg>
          </span>
          <span class="truncate font-medium group-hover:text-[#0E5A31]">{{ w.name }}</span>
        </span>
        <span v-if="authRequired" class="w-36 flex flex-col gap-0.5" @click.stop>
          <span class="truncate text-gray-500" :title="w.ownerName || $t('没有所有者（仅管理员可管理）')">{{ w.ownerName || '—' }}</span>
          <WorkbookShare :workbook="w" @changed="(m: WorkbookMeta) => emit('changed', w, m)" />
        </span>
        <span class="hidden w-28 truncate text-gray-500 lg:block">{{ w.updatedBy || w.createdBy || '—' }}</span>
        <span class="w-28 text-gray-500" :title="w.updatedAt">{{ relativeTime(w.updatedAt) }}</span>
        <span class="hidden w-20 text-right text-gray-400 tabular-nums lg:block">{{ size(w.size) }}</span>
      </div>
      <div v-if="!loading && !items.length" class="empty">
        <svg viewBox="0 0 48 48" width="44" height="44" fill="none"><rect x="8" y="8" width="32" height="32" rx="6" fill="#EEF5F0" /><path d="M8 18h32M8 28h32M20 8v32" stroke="#C5DFCF" stroke-width="2" /></svg>
        <span>{{ emptyText }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.section-title {
  @apply m-0 text-base font-semibold text-gray-800;
}

.pill {
  @apply rounded-full bg-[#eef1ef] px-2 text-xs leading-5 text-gray-500;
}

.table-card {
  @apply overflow-hidden border border-[#e8ebe9] rounded-xl border-solid bg-white;

  box-shadow: 0 1px 3px rgb(16 24 20 / 4%);
}

.list-row {
  @apply flex cursor-pointer items-center gap-3 border-b border-[#f0f2f1] border-b-solid px-4 py-3 transition-colors last:border-b-none hover:bg-[#F6FAF7];

  &.head {
    @apply cursor-default bg-[#FAFBFA] py-2 text-xs text-gray-400 hover:bg-[#FAFBFA];
  }
}

.file-tile {
  @apply h-6 w-6 flex shrink-0 items-center justify-center rounded-md bg-[#107C41];
}

.star {
  @apply cursor-pointer border-none bg-transparent p-0 text-[15px] text-gray-300 transition-colors hover:text-[#E0A800];

  &.on {
    @apply text-[#E0A800];
  }
}

.empty {
  @apply flex flex-col items-center gap-2 py-12 text-gray-400;
}
</style>
