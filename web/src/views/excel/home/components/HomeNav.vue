<script setup lang="ts">
import type { Component } from 'vue'
import { Clock, Collection, Cpu, DataAnalysis, Star } from '@element-plus/icons-vue'
import { t } from '@/i18n'

/** 首页左侧导航：最近 / 星标 / 数据中心 / 自动化 + 模板库 */
export type HomeSection = 'recent' | 'starred' | 'data' | 'automation'

defineProps<{
  section: HomeSection
  datasetCount: number
  /** 数据连接有同步失败时，自动化旁显示红点 */
  failures: number
}>()

const emit = defineEmits<{
  (e: 'update:section', val: HomeSection): void
  (e: 'templates'): void
}>()

const NAV: { key: HomeSection, label: string, icon: Component }[] = [
  { key: 'recent', label: t('最近'), icon: Clock },
  { key: 'starred', label: t('已加星标'), icon: Star },
  { key: 'data', label: t('数据中心'), icon: DataAnalysis },
  { key: 'automation', label: t('自动化'), icon: Cpu },
]
</script>

<template>
  <nav class="w-52 flex shrink-0 flex-col gap-1 border-r border-[#e8ebe9] border-r-solid bg-white px-3 py-4">
    <button v-for="n in NAV" :key="n.key" type="button" class="nav-item" :class="{ active: section === n.key }" @click="emit('update:section', n.key)">
      <el-icon :size="16">
        <component :is="n.icon" />
      </el-icon>
      <span>{{ n.label }}</span>
      <span v-if="n.key === 'data'" class="count">{{ datasetCount }}</span>
      <span v-if="n.key === 'automation' && failures" class="ml-auto h-1.5 w-1.5 rounded-full bg-[#DC2626]" :title="$t('{0} 个数据连接同步失败', [failures])" />
    </button>
    <div class="mx-1 my-2 border-t border-[#eef0ee] border-t-solid" />
    <button type="button" class="nav-item" @click="emit('templates')">
      <el-icon :size="16">
        <Collection />
      </el-icon>
      <span>{{ $t('模板库') }}</span>
    </button>
  </nav>
</template>

<style scoped lang="scss">
.nav-item {
  @apply relative flex cursor-pointer items-center gap-2.5 rounded-lg border-none bg-transparent px-3 py-2 text-left text-[13px] text-gray-600 transition-colors hover:bg-[#F1F5F2] hover:text-gray-900;

  &.active {
    @apply bg-[#E7F4EC] font-medium text-[#0E5A31];

    &::before {
      content: '';

      @apply absolute left-0 top-2 bottom-2 w-[3px] rounded-r bg-[#107C41];
    }
  }
}

.count {
  @apply ml-auto rounded-full bg-[#f0f2f1] px-1.5 text-[11px] leading-[18px] text-gray-500;
}
</style>
