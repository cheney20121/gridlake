<script setup lang="ts">
import type { MyTask } from '@/api/collect'
import { ref, watch } from 'vue'
import { myCollectsApi } from '@/api/collect'

/** 我的填报：我负责的单位在最近几期里填了没有，以及怎么填 */
const props = defineProps<{ visible: boolean }>()

const emit = defineEmits<{ (e: 'update:visible', val: boolean): void }>()

const tasks = ref<MyTask[]>([])
const loading = ref(false)

watch(() => props.visible, async (v) => {
  if (!v)
    return
  loading.value = true
  try {
    tasks.value = await myCollectsApi()
  }
  finally {
    loading.value = false
  }
}, { immediate: true })
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('我的填报')"
    width="760px"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div v-loading="loading" class="min-h-32">
      <div v-if="!tasks.length && !loading" class="text-[13px] text-gray-500">
        {{ $t('没有指派给你的填报任务') }}
      </div>

      <div v-for="t in tasks" :key="t.id" class="card">
        <div class="flex items-center gap-2">
          <span class="text-gray-900 font-medium">{{ t.name }}</span>
          <el-tag v-if="t.pending" size="small" type="danger">
            {{ $t('{0} 项待填', [t.pending]) }}
          </el-tag>
          <el-tag v-else size="small" type="success">
            {{ $t('都已提交') }}
          </el-tag>
        </div>
        <div class="mt-1 text-xs text-gray-500 leading-5">
          {{ $t('填到数据集「{0}」：打开任意工作簿 → 工具栏「填报」→ 在表格里放一个填报区，列里要包含「{1}」和「{2}」，「{1}」填你负责的单位，「{2}」填下面的期次。', [t.datasetName ?? '—', t.unitColumn, t.periodColumn]) }}
        </div>
        <el-table :data="t.units.flatMap(u => u.periods.map(p => ({ unit: u.unit, ...p })))" size="small" class="mt-2" max-height="260" :empty-text="$t('管理员还没有开启期次')">
          <el-table-column :label="t.unitColumn" prop="unit" min-width="140" show-overflow-tooltip />
          <el-table-column :label="t.periodColumn" prop="period" width="110" />
          <el-table-column :label="$t('状态')" width="90">
            <template #default="{ row }">
              <el-tag v-if="row.status === 'submitted'" size="small" type="success">
                {{ $t('已提交') }}
              </el-tag>
              <el-tag v-else size="small" type="danger">
                {{ $t('待填') }}
              </el-tag>
            </template>
          </el-table-column>
          <el-table-column :label="$t('提交时间')" width="160">
            <template #default="{ row }">
              {{ row.submittedAt ?? '—' }}
            </template>
          </el-table-column>
          <el-table-column :label="$t('已填行数')" width="90" align="right" prop="rows" />
        </el-table>
      </div>
    </div>
  </el-dialog>
</template>

<style scoped lang="scss">
.card {
  @apply mb-3 rounded-md border border-[#e5e7eb] border-solid px-3 py-2.5 text-[13px];
}
</style>
