<script setup lang="ts">
import type { TrashedDataset } from '@/api/dataset'
import { ref, watch } from 'vue'
import { emptyDatasetTrashApi, listDatasetTrashApi, purgeDatasetApi, restoreDatasetApi } from '@/api/dataset'

/** 数据集回收站：恢复、彻底删除、清空 */
const props = defineProps<{
  visible: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 数据集有变化（恢复 / 彻底删除）：页面刷新数据集、公式、图表 */
  (e: 'changed'): void
}>()

const items = ref<TrashedDataset[]>([])
const days = ref(0)
const loading = ref(false)

async function load() {
  loading.value = true
  try {
    const r = await listDatasetTrashApi()
    items.value = r.items
    days.value = r.days
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, (v) => {
  if (v)
    load()
})

async function confirm(message: string, title: string, button: string) {
  return ElMessageBox.confirm(message, title, { type: 'warning', confirmButtonText: button, confirmButtonClass: 'ec-button--danger' }).then(() => true, () => false)
}

async function restore(t: TrashedDataset) {
  const r = await restoreDatasetApi(t.id)
  if (r.renamed)
    ElMessage.warning(`已恢复。原名「${t.name}」已被其他数据集使用，恢复为「${r.name}」，引用它的公式需改用新名称`)
  else
    ElMessage.success(`已恢复「${r.name}」`)
  emit('changed')
  await load()
}

async function purge(t: TrashedDataset) {
  const dependents = items.value.filter(x => (x.kind === 'view' || x.kind === 'recon') && x.blocked?.includes(`「${t.name}」`))
  const extra = dependents.length ? `回收站里基于它的关联视图「${dependents.map(x => x.name).join('」「')}」也会一并删除。` : ''
  if (!(await confirm(`彻底删除「${t.name}」${t.rowCount != null ? `及其 ${t.rowCount.toLocaleString()} 行数据` : ''}，删除后不可恢复。${extra}`, '彻底删除', '彻底删除')))
    return
  const names = await purgeDatasetApi(t.id)
  ElMessage.success(names.length > 1 ? `已彻底删除：${names.join('、')}` : '已彻底删除')
  emit('changed')
  await load()
}

async function emptyAll() {
  if (!(await confirm(`彻底删除回收站里的 ${items.value.length} 个数据集及其数据，删除后不可恢复。`, '清空回收站', '清空')))
    return
  const n = await emptyDatasetTrashApi()
  ElMessage.success(`已彻底删除 ${n} 个数据集`)
  emit('changed')
  await load()
}
</script>

<template>
  <el-dialog :model-value="visible" title="数据集回收站" width="820px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div class="mb-3 flex items-center gap-2 text-xs text-gray-500">
      <span>{{ days ? `删除的数据集保留 ${days} 天后自动彻底删除；在此之前数据不会丢失，可随时恢复` : '删除的数据集会一直保留，直到彻底删除' }}</span>
      <el-button class="ml-auto" size="small" type="danger" plain :disabled="!items.length" @click="emptyAll">
        清空回收站
      </el-button>
    </div>
    <el-table v-loading="loading" :data="items" size="small" max-height="420" empty-text="回收站是空的">
      <el-table-column label="名称" min-width="180">
        <template #default="{ row }">
          <span>{{ row.name }}</span>
          <el-tag v-if="row.kind === 'view' || row.kind === 'recon'" size="small" type="info" class="ml-2">
            {{ row.kind === 'recon' ? '对账' : '关联视图' }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column label="行数" width="100" align="right">
        <template #default="{ row }">
          <span class="tabular-nums">{{ row.rowCount != null ? row.rowCount.toLocaleString() : '—' }}</span>
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
      <el-table-column width="140" align="right">
        <template #default="{ row }">
          <el-tooltip :disabled="!row.blocked" :content="row.blocked ?? ''" placement="top">
            <span>
              <el-button link size="small" type="primary" :disabled="!!row.blocked" @click="restore(row)">
                恢复
              </el-button>
            </span>
          </el-tooltip>
          <el-button link size="small" type="danger" class="ml-3" @click="purge(row)">
            彻底删除
          </el-button>
        </template>
      </el-table-column>
    </el-table>
  </el-dialog>
</template>
