<script setup lang="ts">
import type { Component } from 'vue'

/** 首页的入口卡片：图标块 + 标题 + 说明（开始新建、数据中心、自动化共用） */
defineProps<{
  title: string
  desc?: string
  icon: Component
  /** 图标块配色：green / blue / amber / slate / violet */
  tone?: 'green' | 'blue' | 'amber' | 'slate' | 'violet'
  /** 标题后的小提示（如同步失败数） */
  badge?: string
  disabled?: boolean
}>()
</script>

<template>
  <button type="button" class="home-card tool-card" :disabled="disabled">
    <span class="icon-tile" :class="tone ?? 'green'">
      <el-icon :size="18"><component :is="icon" /></el-icon>
    </span>
    <span class="min-w-0 flex flex-col gap-1">
      <span class="font-medium text-gray-800">{{ title }}<span v-if="badge" class="ml-2 text-xs text-[#DC2626] font-normal">{{ badge }}</span></span>
      <span v-if="desc" class="text-xs text-gray-400 leading-snug">{{ desc }}</span>
    </span>
  </button>
</template>

<style scoped lang="scss">
.home-card {
  @apply flex cursor-pointer items-start gap-3 border border-[#e8ebe9] rounded-xl border-solid bg-white px-4 py-3.5 text-left text-[13px] transition-all duration-150;

  &:hover:not(:disabled) {
    @apply border-[#9fd3b4];

    transform: translateY(-2px);
    box-shadow: 0 6px 16px rgb(16 124 65 / 10%);
  }

  &:disabled {
    @apply cursor-wait opacity-70;
  }
}

.icon-tile {
  @apply h-9 w-9 flex shrink-0 items-center justify-center rounded-lg;

  &.green {
    @apply bg-[#E7F4EC] text-[#107C41];
  }

  &.blue {
    @apply bg-[#E8F0FC] text-[#3b7dd8];
  }

  &.amber {
    @apply bg-[#FDF1DC] text-[#C77700];
  }

  &.slate {
    @apply bg-[#EEF0F3] text-[#5b6472];
  }

  &.violet {
    @apply bg-[#F0EBFC] text-[#7a52d1];
  }
}
</style>
