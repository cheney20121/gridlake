<script setup lang="ts">
import type { DatasetInfo } from '@/api/dataset'
import { t } from '@/i18n'

/** 数据中心里的数据集列表：类型、所有者、行列数，可查看使用情况 */
defineProps<{
  items: DatasetInfo[]
  authRequired: boolean
  restricted: boolean
}>()

const emit = defineEmits<{
  (e: 'open', id: string): void
  (e: 'usage', d: DatasetInfo): void
}>()

const KIND_LABEL: Record<DatasetInfo['kind'], string> = { builtin: '内置', table: '数据表', view: '关联视图', summary: t('汇总'), recon: '对账', external: t('外部文件') }
const KIND_TONE: Record<DatasetInfo['kind'], string> = { builtin: 'slate', table: 'green', view: 'blue', summary: 'blue', recon: 'green', external: 'slate' }
</script>

<template>
  <div>
    <div class="mb-3 flex items-center gap-3">
      <h2 class="m-0 text-base text-gray-800 font-semibold">
        {{ $t('数据集') }}
      </h2>
      <span class="rounded-full bg-[#eef1ef] px-2 text-xs leading-5 text-gray-500">{{ $t('{0} 个', [items.length]) }}</span>
    </div>
    <div class="table-card">
      <div class="list-row head">
        <span class="flex-1">{{ $t('名称') }}</span>
        <span class="w-24">{{ $t('类型') }}</span>
        <span v-if="authRequired" class="w-24">{{ $t('所有者') }}</span>
        <span class="w-24 text-right">{{ $t('行数') }}</span>
        <span class="w-16 text-right">{{ $t('列数') }}</span>
        <span class="w-28" />
      </div>
      <div v-for="d in items" :key="d.id" class="list-row group" @click="emit('open', d.id)">
        <span class="min-w-0 flex-1 truncate font-medium group-hover:text-[#0E5A31]">{{ d.name }}</span>
        <span class="w-24"><span class="kind" :class="KIND_TONE[d.kind]">{{ KIND_LABEL[d.kind] }}</span></span>
        <span v-if="authRequired" class="w-24 truncate text-gray-500" :title="d.ownerName || $t('内置或启用登录前创建，仅管理员可修改')">{{ d.ownerName || '—' }}</span>
        <span class="w-24 text-right text-gray-600 tabular-nums">{{ d.rowCount.toLocaleString() }}</span>
        <span class="w-16 text-right text-gray-500 tabular-nums">{{ d.columns.length }}</span>
        <span class="w-28 text-right">
          <el-button v-if="!restricted" link size="small" type="primary" @click.stop="emit('usage', d)">{{ $t('使用情况') }}</el-button>
        </span>
      </div>
      <div v-if="!items.length" class="empty">
        <svg viewBox="0 0 48 48" width="44" height="44" fill="none"><ellipse cx="24" cy="13" rx="14" ry="5" fill="#EEF5F0" stroke="#C5DFCF" stroke-width="2" /><path d="M10 13v22c0 2.8 6.3 5 14 5s14-2.2 14-5V13" stroke="#C5DFCF" stroke-width="2" /><path d="M10 24c0 2.8 6.3 5 14 5s14-2.2 14-5" stroke="#C5DFCF" stroke-width="2" /></svg>
        <span>{{ $t('还没有数据集，先导入一份数据') }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
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

.kind {
  @apply rounded px-1.5 py-0.5 text-xs;

  &.green {
    @apply bg-[#E7F4EC] text-[#0E5A31];
  }

  &.blue {
    @apply bg-[#E8F0FC] text-[#2f64b8];
  }

  &.slate {
    @apply bg-[#EEF0F3] text-[#5b6472];
  }
}

.empty {
  @apply flex flex-col items-center gap-2 py-12 text-gray-400;
}
</style>
