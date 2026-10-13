<script setup lang="ts">
import type { Collect } from '@/api/collect'
import { ref, watch } from 'vue'
import { deleteCollectApi, listCollectsApi } from '@/api/collect'
import { t } from '@/i18n'
import CollectBoardDialog from './CollectBoardDialog.vue'
import CollectEditDialog from './CollectEditDialog.vue'

/** 收集任务管理（管理员）：让很多人往同一个数据集里各填各的，看谁填了、谁没填 */
const props = defineProps<{ visible: boolean }>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 任务或期次变了（首页的「我的填报」要刷新） */
  (e: 'changed'): void
}>()

const items = ref<Collect[]>([])
const loading = ref(false)
const editVisible = ref(false)
const editing = ref<Collect | null>(null)
const boardVisible = ref(false)
const boardFor = ref<Collect | null>(null)

async function load(silent = false) {
  if (!silent)
    loading.value = true
  try {
    items.value = await listCollectsApi()
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, v => v && load(), { immediate: true })

function openCreate() {
  editing.value = null
  editVisible.value = true
}

function openEdit(c: Collect) {
  editing.value = c
  editVisible.value = true
}

function openBoard(c: Collect) {
  boardFor.value = c
  boardVisible.value = true
}

/** 保存后：新建的直接打开看板，下一步就是开启第一期 */
async function onSaved(id: string) {
  const isNew = !editing.value
  await load(true)
  emit('changed')
  const c = items.value.find(x => x.id === id)
  if (isNew && c)
    openBoard(c)
}

async function onBoardChanged() {
  await load(true)
  emit('changed')
}

async function remove(c: Collect) {
  const ok = await ElMessageBox.confirm(t('删除收集任务「{0}」？期次和提交记录会删除，负责人的数据范围会收回；数据集和里面已填的数据保留。', [c.name]), t('删除收集任务'), { type: 'warning', confirmButtonText: t('删除') }).then(() => true, () => false)
  if (!ok)
    return
  await deleteCollectApi(c.id)
  ElMessage.success(t('已删除'))
  await load(true)
  emit('changed')
}

function percent(c: Collect) {
  return c.unitCount ? Math.round((c.latestSubmitted / c.unitCount) * 100) : 0
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('收集任务')"
    width="960px"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="mb-3 flex items-center gap-3 text-[13px]">
      <span class="flex-1 text-xs text-gray-500 leading-5">
        {{ $t('让很多人往同一个数据集里各填各的：每个人只能看到、只能填自己负责的单位；汇总不用另做，透视表和仪表盘直接引用这个数据集。') }}
      </span>
      <el-button type="primary" @click="openCreate">
        {{ $t('+ 新建收集任务') }}
      </el-button>
    </div>

    <div v-loading="loading" class="min-h-40">
      <div v-if="!items.length && !loading" class="empty">
        <div class="text-[15px] text-gray-800 font-medium">
          {{ $t('还没有收集任务') }}
        </div>
        <div class="text-xs text-gray-500 leading-5">
          {{ $t('例：30 家店铺每月报一次经营数据——建一个任务、列出店铺和负责人、每月开启一期，看板上随时能看谁没填') }}
        </div>
        <el-button type="primary" class="mt-2" @click="openCreate">
          {{ $t('新建收集任务') }}
        </el-button>
      </div>

      <div v-for="c in items" :key="c.id" class="card">
        <div class="flex items-center gap-3">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="text-gray-900 font-medium">{{ c.name }}</span>
              <span v-if="!c.datasetName" class="text-xs text-[#B91C1C]">{{ $t('数据集已删除') }}</span>
            </div>
            <div class="mt-0.5 flex flex-wrap items-center gap-x-3 text-xs text-gray-500">
              <span>{{ $t('数据集：{0}', [c.datasetName ?? '—']) }}</span>
              <span>{{ $t('{0} 个单位，{1} 个已指派负责人', [c.unitCount, c.assignedCount]) }}</span>
              <span>{{ $t('已开启 {0} 期', [c.periodCount]) }}</span>
              <span v-if="c.createdByName">{{ $t('创建人 {0}', [c.createdByName]) }}</span>
            </div>
          </div>
          <div class="w-60 text-xs">
            <template v-if="c.latestPeriod">
              <div class="mb-1 flex justify-between">
                <span>{{ $t('最近一期 {0}', [c.latestPeriod]) }}</span>
                <span><b>{{ c.latestSubmitted }}</b> / {{ c.unitCount }}</span>
              </div>
              <el-progress :percentage="percent(c)" :stroke-width="8" :show-text="false" />
            </template>
            <span v-else class="text-gray-400">{{ $t('还没有开启期次') }}</span>
          </div>
          <div class="flex shrink-0 items-center">
            <el-button size="small" type="primary" plain @click="openBoard(c)">
              {{ $t('看板') }}
            </el-button>
            <el-button link size="small" @click="openEdit(c)">
              {{ $t('修改') }}
            </el-button>
            <el-button link size="small" type="danger" @click="remove(c)">
              {{ $t('删除') }}
            </el-button>
          </div>
        </div>
      </div>
    </div>

    <CollectEditDialog v-model:visible="editVisible" :collect="editing" @saved="onSaved" />
    <CollectBoardDialog v-model:visible="boardVisible" :collect="boardFor" @changed="onBoardChanged" />
  </el-dialog>
</template>

<style scoped lang="scss">
.card {
  @apply mb-2 rounded-md border border-[#e5e7eb] border-solid px-3 py-2.5 text-[13px];
}

.empty {
  @apply flex flex-col items-center gap-2 rounded-md border border-dashed border-[#c8c8c8] px-6 py-10 text-center;
}
</style>
