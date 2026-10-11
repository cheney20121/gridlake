<script setup lang="ts">
import type { TrashedWorkbook } from '@/api/workbook'

/** 回收站里的工作簿：可恢复或彻底删除 */
defineProps<{
  items: TrashedWorkbook[]
}>()

const emit = defineEmits<{
  (e: 'restore', t: TrashedWorkbook): void
  (e: 'purge', t: TrashedWorkbook): void
}>()
</script>

<template>
  <el-table :data="items" size="small" height="100%" class="flex-1" empty-text="回收站是空的">
    <el-table-column label="名称" min-width="200" prop="name" show-overflow-tooltip />
    <el-table-column label="原位置" min-width="120">
      <template #default="{ row }">
        <span class="text-gray-500">{{ row.folderName ?? '我的工作簿' }}</span>
      </template>
    </el-table-column>
    <el-table-column label="删除时间" min-width="170">
      <template #default="{ row }">
        <span>{{ row.deletedAt }}</span>
        <span class="ml-1 text-gray-400">{{ row.deletedBy }}</span>
      </template>
    </el-table-column>
    <el-table-column label="自动删除于" width="140">
      <template #default="{ row }">
        <span class="text-gray-500">{{ row.purgeAt ?? '—' }}</span>
      </template>
    </el-table-column>
    <el-table-column width="130" align="right">
      <template #default="{ row }">
        <el-button link size="small" type="primary" @click="emit('restore', row)">
          恢复
        </el-button>
        <el-button link size="small" type="danger" @click="emit('purge', row)">
          彻底删除
        </el-button>
      </template>
    </el-table-column>
  </el-table>
</template>
