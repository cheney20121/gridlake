<script setup lang="ts">
import type { TreeNode, View } from './folder-model'
import { ref } from 'vue'
import { ROOT } from './folder-model'

/** 左侧导航：全部 / 星标 / 文件夹树（可新建、重命名、移动、删除）/ 回收站 */
defineProps<{
  tree: TreeNode[]
  allCount: number
  starredCount: number
  trashCount: number
  isActive: (v: View) => boolean
}>()

const emit = defineEmits<{
  (e: 'go', v: View): void
  (e: 'newFolder', parentId: string | null): void
  (e: 'folderCommand', id: string, cmd: 'new' | 'rename' | 'move' | 'delete'): void
}>()

const treeRef = ref<{ getNode: (key: string) => { expand: () => void, parent?: unknown } | null }>()

/** 展开某个节点（进入文件夹时让侧栏里能看到当前位置） */
function expand(id: string) {
  treeRef.value?.getNode(id)?.expand()
}

defineExpose({ expand })
</script>

<template>
  <aside class="w-56 flex flex-col shrink-0 border-r border-r-[#ebeef5] border-r-solid">
    <el-scrollbar class="flex-1">
      <div class="p-2">
        <div class="nav" :class="{ on: isActive({ kind: 'all' }) }" @click="emit('go', { kind: 'all' })">
          <span class="icon">▤</span><span class="flex-1">全部工作簿</span><span class="count">{{ allCount }}</span>
        </div>
        <div class="nav" :class="{ on: isActive({ kind: 'starred' }) }" @click="emit('go', { kind: 'starred' })">
          <span class="icon text-[#E6A23C]">★</span><span class="flex-1">星标</span><span class="count">{{ starredCount }}</span>
        </div>
        <div class="section">
          <span>文件夹</span>
          <el-button link size="small" title="在根目录新建文件夹" @click="emit('newFolder', null)">
            + 新建
          </el-button>
        </div>
        <el-tree
          ref="treeRef"
          :data="tree"
          node-key="id"
          :default-expanded-keys="[ROOT]"
          :expand-on-click-node="false"
          :highlight-current="false"
          class="folder-tree"
          @node-click="(n: TreeNode) => emit('go', { kind: 'folder', id: n.id })"
        >
          <template #default="{ data }">
            <div class="tree-node" :class="{ on: isActive({ kind: 'folder', id: data.id }) }">
              <span class="icon">{{ data.id === ROOT ? '⌂' : '▸' }}</span>
              <span class="min-w-0 flex-1 truncate" :title="data.label">{{ data.label }}</span>
              <span class="count">{{ data.count || '' }}</span>
              <el-dropdown trigger="click" @command="(c: 'new' | 'rename' | 'move' | 'delete') => emit('folderCommand', data.id, c)" @click.stop>
                <span class="more" @click.stop>⋯</span>
                <template #dropdown>
                  <el-dropdown-menu>
                    <el-dropdown-item command="new">
                      新建子文件夹
                    </el-dropdown-item>
                    <template v-if="data.id !== ROOT">
                      <el-dropdown-item command="rename">
                        重命名
                      </el-dropdown-item>
                      <el-dropdown-item command="move">
                        移动到…
                      </el-dropdown-item>
                      <el-dropdown-item command="delete" divided>
                        <span class="text-[#B91C1C]">删除</span>
                      </el-dropdown-item>
                    </template>
                  </el-dropdown-menu>
                </template>
              </el-dropdown>
            </div>
          </template>
        </el-tree>
      </div>
    </el-scrollbar>
    <div class="border-t border-t-[#ebeef5] border-t-solid p-2">
      <div class="nav" :class="{ on: isActive({ kind: 'trash' }) }" @click="emit('go', { kind: 'trash' })">
        <span class="icon">🗑</span><span class="flex-1">回收站</span><span class="count">{{ trashCount || '' }}</span>
      </div>
    </div>
  </aside>
</template>

<style scoped lang="scss">
.nav {
  @apply flex items-center gap-2 rounded px-2 py-1.5 text-[13px] text-gray-700 cursor-pointer hover:bg-[#F4F4F1];

  &.on {
    @apply bg-[#E7F4EC] text-[#0B5A2E] font-medium;
  }
}

.icon {
  @apply w-4 shrink-0 text-center text-gray-400;
}

.count {
  @apply text-xs text-gray-400 tabular-nums;
}

.section {
  @apply mt-3 mb-1 flex items-center justify-between px-2 text-xs text-gray-400;
}

.folder-tree {
  :deep(.#{$namespace}-tree-node__content) {
    @apply h-8 rounded;
  }
}

.tree-node {
  @apply min-w-0 flex flex-1 items-center gap-1.5 rounded px-1 py-1 pr-2 text-[13px] text-gray-700;

  &.on {
    @apply bg-[#E7F4EC] text-[#0B5A2E] font-medium;
  }

  .more {
    @apply px-1 text-gray-400 opacity-0 cursor-pointer hover:text-gray-700;
  }

  &:hover .more {
    @apply opacity-100;
  }
}
</style>
