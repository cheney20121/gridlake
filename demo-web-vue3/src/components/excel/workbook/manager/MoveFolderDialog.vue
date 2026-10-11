<script setup lang="ts">
import type { TreeNode } from './folder-model'
import { ROOT } from './folder-model'

/** 「移动到…」：选一个目标文件夹 */
defineProps<{
  open: boolean
  title?: string
  tree: TreeNode[]
}>()

const emit = defineEmits<{
  (e: 'cancel'): void
  (e: 'confirm'): void
}>()

const target = defineModel<string>('target', { required: true })
</script>

<template>
  <el-dialog :model-value="open" :title="title" width="380px" append-to-body @update:model-value="emit('cancel')">
    <el-scrollbar max-height="320px">
      <el-tree
        v-if="open"
        :data="tree"
        node-key="id"
        default-expand-all
        :expand-on-click-node="false"
        @node-click="(n: TreeNode) => target = n.id"
      >
        <template #default="{ data }">
          <div class="tree-node" :class="{ on: target === data.id }">
            <span class="icon">{{ data.id === ROOT ? '⌂' : '▸' }}</span>
            <span class="truncate">{{ data.label }}</span>
          </div>
        </template>
      </el-tree>
    </el-scrollbar>
    <template #footer>
      <el-button size="small" @click="emit('cancel')">
        取消
      </el-button>
      <el-button size="small" type="primary" @click="emit('confirm')">
        移到这里
      </el-button>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.icon {
  @apply w-4 shrink-0 text-center text-gray-400;
}

.tree-node {
  @apply min-w-0 flex flex-1 items-center gap-1.5 rounded px-1 py-1 pr-2 text-[13px] text-gray-700;

  &.on {
    @apply bg-[#E7F4EC] text-[#0B5A2E] font-medium;
  }
}
</style>
