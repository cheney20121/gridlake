<script setup lang="ts">
import type { AiThread } from '@/api/ai'

/** 对话栏：切换对话、新对话、复制对话链接、删除对话 */
defineProps<{
  threads: AiThread[]
  threadId: string | null
  /** 回答进行中时不能删除当前对话 */
  sending: boolean
}>()

const emit = defineEmits<{
  (e: 'select', id: string): void
  (e: 'new'): void
  (e: 'link'): void
  (e: 'delete'): void
}>()
</script>

<template>
  <div class="flex items-center gap-1 border-b border-b-[#eef0f3] border-b-solid px-3 py-2">
    <el-select :model-value="threadId ?? undefined" size="small" class="min-w-0 flex-1" :placeholder="$t('新对话')" :no-data-text="$t('还没有对话')" @update:model-value="(id: string) => emit('select', id)">
      <el-option v-for="t in threads" :key="t.id" :label="t.title" :value="t.id">
        <span>{{ t.title }}</span>
        <span v-if="t.running" class="ml-2 text-xs text-[#217346]">{{ $t('回答中') }}</span>
        <span v-else class="ml-2 text-xs text-gray-400">{{ t.updatedAt.slice(5, 16) }}</span>
      </el-option>
    </el-select>
    <el-button size="small" :title="$t('新对话')" @click="emit('new')">
      {{ $t('新对话') }}
    </el-button>
    <el-dropdown v-if="threadId" trigger="click" @command="(c: 'link' | 'delete') => c === 'link' ? emit('link') : emit('delete')">
      <el-button size="small" text :title="$t('更多')">
        ⋯
      </el-button>
      <template #dropdown>
        <el-dropdown-menu>
          <el-dropdown-item command="link">
            {{ $t('复制对话链接') }}
          </el-dropdown-item>
          <el-dropdown-item command="delete" :disabled="sending">
            {{ $t('删除对话') }}
          </el-dropdown-item>
        </el-dropdown-menu>
      </template>
    </el-dropdown>
  </div>
</template>
