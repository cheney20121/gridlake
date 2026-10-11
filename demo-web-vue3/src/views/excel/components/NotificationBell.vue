<script setup lang="ts">
import type { NotificationItem } from '@/api/comment'
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { readNotificationsApi } from '@/api/comment'
import { inbox, refreshInbox } from '@/features/excel/comments/state'
import { cellName } from '@/features/excel/writeback/input-table'

/** 顶栏铃铛：别人 @ 了你、回复了你的评论时显示未读数，点一条跳到那个格子；每分钟查一次 */
const emit = defineEmits<{ (e: 'open', n: NotificationItem): void }>()

const visible = ref(false)
let timer: ReturnType<typeof setInterval> | null = null

onMounted(() => {
  refreshInbox()
  timer = setInterval(refreshInbox, 60_000)
})
onBeforeUnmount(() => timer && clearInterval(timer))

async function open(n: NotificationItem) {
  visible.value = false
  if (!n.read) {
    n.read = true
    inbox.value.unread = Math.max(0, inbox.value.unread - 1)
    readNotificationsApi([n.id]).catch(() => {})
  }
  emit('open', n)
}

async function readAll() {
  await readNotificationsApi()
  await refreshInbox()
}

const KIND: Record<string, string> = { mention: '@ 了你', reply: '回复了你的评论' }
</script>

<template>
  <el-popover v-model:visible="visible" trigger="click" placement="bottom-end" :width="360" @show="refreshInbox">
    <template #reference>
      <button type="button" class="bell" title="评论通知" data-bell>
        <svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 2.2a3.8 3.8 0 0 0-3.8 3.8v2.2L3 10.2v.8h10v-.8l-1.2-2V6A3.8 3.8 0 0 0 8 2.2ZM6.5 12.6a1.6 1.6 0 0 0 3 0" /></svg>
        <span v-if="inbox.unread" class="badge" data-unread>{{ inbox.unread > 99 ? '99+' : inbox.unread }}</span>
      </button>
    </template>
    <div class="flex items-center justify-between pb-2 text-[13px]">
      <b>评论通知</b>
      <el-button v-if="inbox.unread" link size="small" type="primary" @click="readAll">
        全部标为已读
      </el-button>
    </div>
    <div v-if="!inbox.items.length" class="py-6 text-center text-xs text-gray-400">
      还没有人 @ 你或回复你的评论
    </div>
    <div class="max-h-[50vh] overflow-y-auto">
      <button v-for="n in inbox.items" :key="n.id" type="button" class="item" :class="{ unread: !n.read }" data-notification @click="open(n)">
        <div class="flex items-center gap-1 text-xs">
          <b>{{ n.fromName }}</b><span class="text-gray-500">{{ KIND[n.kind] ?? n.kind }}</span>
          <span class="ml-auto text-gray-400">{{ n.createdAt.slice(5, 16) }}</span>
        </div>
        <div class="mt-0.5 line-clamp-2 text-[13px]">
          {{ n.text }}
        </div>
        <div class="mt-0.5 text-xs text-gray-400">
          {{ n.workbookName }} · {{ cellName(n.row, n.col) }}
        </div>
      </button>
    </div>
  </el-popover>
</template>

<style scoped lang="scss">
.bell {
  @apply relative flex h-7 w-7 cursor-pointer items-center justify-center border-none rounded-md bg-transparent text-slate-500 hover:bg-[#f0f2f1];
}

.badge {
  @apply absolute right-0 top-0 min-w-4 rounded-full bg-[#E5484D] px-1 text-center text-[10px] text-white leading-4;
}

.item {
  @apply mb-1 w-full cursor-pointer rounded-md border-none bg-transparent p-2 text-left hover:bg-[#f4f6f5];

  &.unread {
    @apply bg-[#F0F7F3];
  }
}
</style>
