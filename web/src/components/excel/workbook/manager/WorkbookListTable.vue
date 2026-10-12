<script setup lang="ts">
import type { WorkbookMeta } from '@/api/workbook'
import { useLedgerUserStore } from '@/stores/ledger-user'
import WorkbookShare from '../WorkbookShare.vue'

/** 工作簿列表：星标、名称（当前打开的标「当前」）、所有者 / 共享、修改信息、版本与大小，行内重命名 / 复制 / 移动 / 删除 */
defineProps<{
  items: WorkbookMeta[]
  currentId: string | null
  /** 搜索 / 全部 / 星标视图里在名称下显示所在文件夹 */
  showPath: boolean
  emptyText: string
  pathOf: (folderId: string | null) => string
  canEdit: (w: WorkbookMeta) => boolean
  canManage: (w: WorkbookMeta) => boolean
}>()

const emit = defineEmits<{
  (e: 'open', w: WorkbookMeta): void
  (e: 'star', w: WorkbookMeta): void
  (e: 'rename', w: WorkbookMeta): void
  (e: 'duplicate', w: WorkbookMeta): void
  (e: 'move', items: WorkbookMeta[]): void
  (e: 'remove', items: WorkbookMeta[]): void
  (e: 'selection', rows: WorkbookMeta[]): void
}>()

const ledgerUser = useLedgerUserStore()

function sizeText(n: number) {
  if (n >= 1e6)
    return `${(n / 1e6).toFixed(1)} MB`
  return `${Math.max(1, Math.round(n / 1e3))} KB`
}
</script>

<template>
  <el-table
    :data="items"
    size="small"
    height="100%"
    class="flex-1"
    row-key="id"
    :empty-text="emptyText"
    @row-dblclick="(w: WorkbookMeta) => emit('open', w)"
    @selection-change="(rows: WorkbookMeta[]) => emit('selection', rows)"
  >
    <el-table-column type="selection" width="36" />
    <el-table-column width="30">
      <template #default="{ row }">
        <span class="star" :class="{ on: row.starred }" :title="row.starred ? $t('取消星标') : $t('加星标')" @click.stop="emit('star', row)">{{ row.starred ? '★' : '☆' }}</span>
      </template>
    </el-table-column>
    <el-table-column :label="$t('名称')" min-width="150">
      <template #default="{ row }">
        <button class="name-link" @click="emit('open', row)">
          {{ row.name }}
        </button>
        <el-tag v-if="row.id === currentId" size="small" type="success" class="ml-2">
          {{ $t('当前') }}
        </el-tag>
        <div v-if="showPath" class="text-xs text-gray-400">
          {{ pathOf(row.folderId) || $t('我的工作簿') }}
        </div>
      </template>
    </el-table-column>
    <el-table-column v-if="ledgerUser.authRequired" :label="$t('所有者 / 共享')" width="170">
      <template #default="{ row }">
        <div class="flex flex-col gap-0.5">
          <span class="truncate text-gray-600" :title="row.ownerName || $t('没有所有者（仅管理员可管理）')">{{ row.ownerName || '—' }}</span>
          <WorkbookShare :workbook="row" @changed="(m: WorkbookMeta) => Object.assign(row, m)" />
        </div>
      </template>
    </el-table-column>
    <el-table-column :label="$t('最后修改')" min-width="190">
      <template #default="{ row }">
        <span>{{ row.updatedAt }}</span>
        <span class="ml-1 text-gray-400">{{ row.updatedBy }}</span>
      </template>
    </el-table-column>
    <el-table-column :label="$t('版本')" prop="version" width="56" align="right" />
    <el-table-column :label="$t('大小')" width="76" align="right">
      <template #default="{ row }">
        <el-tooltip :content="$t('压缩后存储 {0}', [sizeText(row.storedSize)])" placement="top">
          <span>{{ sizeText(row.size) }}</span>
        </el-tooltip>
      </template>
    </el-table-column>
    <el-table-column width="220" align="right" fixed="right">
      <template #default="{ row }">
        <el-button link size="small" :disabled="!canEdit(row)" :title="canEdit(row) ? '' : $t('只有所有者开放编辑后才能修改')" @click="emit('rename', row)">
          {{ $t('重命名') }}
        </el-button>
        <el-button link size="small" @click="emit('duplicate', row)">
          {{ $t('复制') }}
        </el-button>
        <el-button link size="small" :disabled="!canEdit(row)" @click="emit('move', [row])">
          {{ $t('移动') }}
        </el-button>
        <el-button link size="small" type="danger" :disabled="row.id === currentId || !canManage(row)" :title="canManage(row) ? '' : $t('只有所有者或管理员可以删除')" @click="emit('remove', [row])">
          {{ $t('删除') }}
        </el-button>
      </template>
    </el-table-column>
  </el-table>
</template>

<style scoped lang="scss">
.name-link {
  @apply border-none bg-transparent p-0 text-left text-[13px] text-gray-800 cursor-pointer hover:text-[#107C41] hover:underline;
}

.star {
  @apply cursor-pointer text-base text-gray-300 hover:text-[#E6A23C];

  &.on {
    @apply text-[#E6A23C];
  }
}
</style>
