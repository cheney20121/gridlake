<script setup lang="ts">
import type { MyTask } from '@/api/collect'
import { ref, watch } from 'vue'
import { myCollectsApi, openCollectWorkbookApi } from '@/api/collect'
import CollectBoardDialog from './CollectBoardDialog.vue'

/** 我的填报：我负责的单位在最近几期里填了没有（被退回的要改）、怎么填；我是确认人的任务可以进去确认 */
const props = defineProps<{ visible: boolean }>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 打开生成好的填报工作簿 */
  (e: 'open', workbookId: string): void
  /** 确认 / 退回之后，首页的待办数要刷新 */
  (e: 'changed'): void
}>()

const tasks = ref<MyTask[]>([])
const loading = ref(false)
/** 正在生成工作簿的（任务、单位、期次） */
const opening = ref('')
const boardFor = ref<MyTask | null>(null)
const boardVisible = ref(false)

function openBoard(t: MyTask) {
  boardFor.value = t
  boardVisible.value = true
}

async function load() {
  loading.value = true
  try {
    tasks.value = await myCollectsApi()
  }
  finally {
    loading.value = false
  }
}

async function onBoardChanged() {
  await load()
  emit('changed')
}

/** 一键生成（或打开已有的）这个单位这一期的填报工作簿：填报区已放好，单位和期次自动带上，只填其他列 */
async function fill(taskId: string, unit: string, period: string) {
  opening.value = `${taskId}|${unit}|${period}`
  try {
    const r = await openCollectWorkbookApi(taskId, unit, period)
    emit('update:visible', false)
    emit('open', r.workbookId)
  }
  finally {
    opening.value = ''
  }
}

watch(() => props.visible, v => v && load(), { immediate: true })
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('我的填报')"
    width="min(860px, 96vw)"
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
          <template v-if="t.units.length">
            <el-tag v-if="t.pending" size="small" type="danger">
              {{ $t('{0} 项待填', [t.pending]) }}
            </el-tag>
            <el-tag v-else size="small" type="success">
              {{ $t('都已提交') }}
            </el-tag>
          </template>
          <el-tag v-if="t.units.length && t.overdue" size="small" type="danger" effect="dark">
            {{ $t('{0} 项已逾期', [t.overdue]) }}
          </el-tag>
          <el-tag v-if="t.reviewer && t.toReview" size="small" type="warning">
            {{ $t('{0} 项待你确认', [t.toReview]) }}
          </el-tag>
          <span class="flex-1" />
          <el-button v-if="t.reviewer" size="small" :type="t.toReview ? 'primary' : 'default'" @click="openBoard(t)">
            {{ $t('去确认') }}
          </el-button>
        </div>
        <div v-if="t.units.length" class="mt-1 text-xs text-gray-500 leading-5">
          {{ $t('点「去填报」打开专属的填报表：「{0}」和「{1}」已经自动带上，只要填其他列；填完在工具栏「填报」里点「提交到数据集」。已经提交过的可以再点开修改，再提交会更新；被退回的按原因改完重新提交；确认人确认后就锁定，不能再改。', [t.unitColumn, t.periodColumn]) }}
          <template v-if="t.reviewerName">
            {{ $t('确认人：{0}', [t.reviewerName]) }}
          </template>
        </div>
        <div v-else class="mt-1 text-xs text-gray-500 leading-5">
          {{ $t('你是这个任务的确认人：负责人提交后，点「去确认」逐个确认并锁定，或者写明原因退回重填。') }}
        </div>
        <el-table v-if="t.units.length" :data="t.units.flatMap(u => u.periods.map(p => ({ unit: u.unit, ...p })))" size="small" class="mt-2" max-height="260" :empty-text="$t('管理员还没有开启期次')">
          <el-table-column :label="t.unitColumn" prop="unit" min-width="140" show-overflow-tooltip />
          <el-table-column :label="t.periodColumn" prop="period" width="110" />
          <el-table-column :label="$t('状态')" width="125">
            <template #default="{ row }">
              <el-tag v-if="row.status === 'confirmed'" size="small" type="success" effect="dark">
                {{ $t('已确认') }}
              </el-tag>
              <el-tag v-else-if="row.status === 'submitted'" size="small" type="success">
                {{ $t('已提交') }}
              </el-tag>
              <el-tag v-else-if="row.status === 'returned'" size="small" type="danger" effect="dark">
                {{ $t('已退回') }}
              </el-tag>
              <el-tag v-else size="small" type="danger">
                {{ $t('待填') }}
              </el-tag>
              <el-tag v-if="row.overdue" size="small" type="danger" effect="dark" class="ml-1">
                {{ $t('逾期') }}
              </el-tag>
            </template>
          </el-table-column>
          <el-table-column :label="$t('截止日期')" width="100">
            <template #default="{ row }">
              <span :class="row.overdue ? 'text-[#B91C1C]' : ''">{{ row.deadline ?? '—' }}</span>
            </template>
          </el-table-column>
          <el-table-column :label="$t('提交时间')" width="160">
            <template #default="{ row }">
              {{ row.submittedAt ?? '—' }}
            </template>
          </el-table-column>
          <el-table-column :label="$t('已填行数')" width="90" align="right" prop="rows" />
          <el-table-column :label="$t('退回原因')" min-width="120" show-overflow-tooltip>
            <template #default="{ row }">
              <span v-if="row.returnReason" class="text-[#B91C1C]">{{ row.returnReason }}</span>
              <span v-else class="text-gray-300">—</span>
            </template>
          </el-table-column>
          <el-table-column width="100" align="center">
            <template #default="{ row }">
              <el-button size="small" :type="row.status === 'pending' || row.status === 'returned' ? 'primary' : 'default'" :loading="opening === `${t.id}|${row.unit}|${row.period}`" @click="fill(t.id, row.unit, row.period)">
                {{ row.status === 'pending' ? $t('去填报') : row.status === 'returned' ? $t('去修改') : row.status === 'confirmed' ? $t('查看') : $t('打开') }}
              </el-button>
            </template>
          </el-table-column>
        </el-table>
      </div>
    </div>

    <CollectBoardDialog v-model:visible="boardVisible" :collect="boardFor" @changed="onBoardChanged" />
  </el-dialog>
</template>

<style scoped lang="scss">
.card {
  @apply mb-3 rounded-md border border-[#e5e7eb] border-solid px-3 py-2.5 text-[13px];
}
</style>
