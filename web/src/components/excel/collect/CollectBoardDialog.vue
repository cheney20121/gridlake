<script setup lang="ts">
import type { Collect, CollectDetail } from '@/api/collect'
import { useIntervalFn } from '@vueuse/core'
import { computed, ref, watch } from 'vue'
import { collectDetailApi, deleteCollectPeriodApi, openCollectPeriodApi } from '@/api/collect'
import { t } from '@/i18n'

/** 收集看板：选一期，看每个单位谁填了、谁没填；开启新一期；复制未填名单去催 */
const props = defineProps<{
  visible: boolean
  collect: Collect | null
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 期次或提交状态变了，列表里的进度要刷新 */
  (e: 'changed'): void
}>()

const detail = ref<CollectDetail | null>(null)
const period = ref('')
const filter = ref<'all' | 'pending' | 'submitted'>('all')
const loading = ref(false)

async function load(p?: string, silent = false) {
  if (!props.collect)
    return
  if (!silent)
    loading.value = true
  try {
    detail.value = await collectDetailApi(props.collect.id, p || undefined)
    period.value = detail.value.period ?? ''
  }
  finally {
    loading.value = false
  }
}

// 有人提交后看板自己更新（30 秒一次，后台静默）
const { pause, resume } = useIntervalFn(() => load(period.value, true), 30000, { immediate: false })
watch(() => props.visible, v => v ? resume() : pause(), { immediate: true })

watch(() => [props.visible, props.collect?.id], ([v]) => {
  if (!v)
    return
  detail.value = null
  period.value = ''
  filter.value = 'all'
  load()
}, { immediate: true })

const current = computed(() => detail.value?.periods.find(p => p.period === period.value) ?? null)
const percent = computed(() => (current.value && current.value.total ? Math.round((current.value.submitted / current.value.total) * 100) : 0))
const rows = computed(() => (detail.value?.units ?? []).filter(u => filter.value === 'all' || u.status === filter.value))
const pendingUnits = computed(() => (detail.value?.units ?? []).filter(u => u.status === 'pending'))

/** 建议的下一期：当前月（已经有了就下个月） */
function suggestPeriod() {
  const have = new Set(detail.value?.periods.map(p => p.period))
  const d = new Date()
  for (let i = 0; i < 24; i++) {
    const s = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`
    if (!have.has(s))
      return s
    d.setMonth(d.getMonth() + 1)
  }
  return ''
}

async function openPeriod() {
  if (!props.collect)
    return
  const value = await ElMessageBox.prompt(t('期次的名称（填报时要填在「{0}」列里），如 2026-10、2026-W42', [props.collect.periodColumn]), t('开启新一期'), { inputValue: suggestPeriod(), inputPattern: /\S/, inputErrorMessage: t('请输入期次') })
    .then(r => r.value.trim(), () => '')
  if (!value)
    return
  await openCollectPeriodApi(props.collect.id, value)
  ElMessage.success(t('已开启「{0}」，负责人现在可以填报了', [value]))
  await load(value)
  emit('changed')
}

async function removePeriod() {
  if (!props.collect || !current.value)
    return
  const p = current.value.period
  const ok = await ElMessageBox.confirm(t('删除「{0}」这一期？只能删还没有人提交过的期次。', [p]), t('删除期次'), { type: 'warning', confirmButtonText: t('删除') }).then(() => true, () => false)
  if (!ok)
    return
  await deleteCollectPeriodApi(props.collect.id, p)
  ElMessage.success(t('已删除'))
  await load()
  emit('changed')
}

async function copyPending() {
  const text = pendingUnits.value.map(u => (u.userName ? `${u.unit}（${u.userName}）` : u.unit)).join('\n')
  if (!text)
    return ElMessage.info(t('都已提交'))
  try {
    await navigator.clipboard.writeText(text)
    ElMessage.success(t('已复制 {0} 个未提交的单位', [pendingUnits.value.length]))
  }
  catch {
    await ElMessageBox.alert(text.replace(/\n/g, '、'), t('未提交的单位'))
  }
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('收集看板 · {0}', [collect?.name])"
    width="920px"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div v-loading="loading" class="min-h-48">
      <template v-if="detail">
        <div class="mb-3 flex flex-wrap items-center gap-2 text-[13px]">
          <el-select v-model="period" class="!w-44" :placeholder="$t('还没有期次')" :disabled="!detail.periods.length" @change="load(period)">
            <el-option v-for="p in detail.periods" :key="p.period" :label="`${p.period}（${p.submitted}/${p.total}）`" :value="p.period" />
          </el-select>
          <el-button type="primary" @click="openPeriod">
            {{ $t('+ 开启新一期') }}
          </el-button>
          <el-button v-if="current && !current.submitted" link type="danger" @click="removePeriod">
            {{ $t('删除这一期') }}
          </el-button>
          <el-button link :loading="loading" @click="load(period)">
            {{ $t('刷新') }}
          </el-button>
          <span class="flex-1" />
          <span class="text-xs text-gray-500">
            {{ $t('数据在数据集「{0}」里，单位列「{1}」、期次列「{2}」', [detail.collect.datasetName ?? $t('（已删除）'), detail.collect.unitColumn, detail.collect.periodColumn]) }}
          </span>
        </div>

        <div v-if="!detail.periods.length" class="empty">
          <div class="text-[15px] text-gray-800 font-medium">
            {{ $t('还没有开启任何一期') }}
          </div>
          <div class="text-xs text-gray-500 leading-5">
            {{ $t('开启一期（如本月）后，负责人才能往数据集里填；提交时填报区要带上单位列和期次列。') }}
          </div>
          <el-button type="primary" class="mt-2" @click="openPeriod">
            {{ $t('开启第一期') }}
          </el-button>
        </div>

        <template v-else-if="current">
          <div class="mb-3 flex items-center gap-3">
            <el-progress :percentage="percent" :stroke-width="10" class="w-72" :show-text="false" />
            <span class="text-[13px]">
              <b>{{ current.submitted }}</b> / {{ current.total }} {{ $t('已提交') }}
              <span v-if="current.total - current.submitted" class="ml-2 text-[#B91C1C]">{{ $t('{0} 个未提交', [current.total - current.submitted]) }}</span>
              <span v-else class="ml-2 text-[#0B5A2E]">{{ $t('全部已提交') }}</span>
            </span>
            <span class="flex-1" />
            <el-radio-group v-model="filter" size="small">
              <el-radio-button value="all">
                {{ $t('全部') }}
              </el-radio-button>
              <el-radio-button value="pending">
                {{ $t('未提交') }}
              </el-radio-button>
              <el-radio-button value="submitted">
                {{ $t('已提交') }}
              </el-radio-button>
            </el-radio-group>
            <el-button size="small" :disabled="!pendingUnits.length" @click="copyPending">
              {{ $t('复制未提交名单') }}
            </el-button>
          </div>

          <el-table :data="rows" size="small" max-height="440" :empty-text="$t('没有符合的单位')">
            <el-table-column :label="detail.collect.unitColumn" prop="unit" min-width="160" show-overflow-tooltip />
            <el-table-column :label="$t('负责人')" min-width="170" show-overflow-tooltip>
              <template #default="{ row }">
                <span v-if="row.userName">{{ row.userName }}</span>
                <span v-else class="text-gray-400">{{ $t('未指派（管理员代填）') }}</span>
              </template>
            </el-table-column>
            <el-table-column :label="$t('状态')" width="90">
              <template #default="{ row }">
                <el-tag v-if="row.status === 'submitted'" size="small" type="success">
                  {{ $t('已提交') }}
                </el-tag>
                <el-tag v-else size="small" type="danger">
                  {{ $t('未提交') }}
                </el-tag>
              </template>
            </el-table-column>
            <el-table-column :label="$t('提交时间')" prop="submittedAt" width="160">
              <template #default="{ row }">
                {{ row.submittedAt ?? '—' }}
              </template>
            </el-table-column>
            <el-table-column :label="$t('提交人')" width="110" show-overflow-tooltip>
              <template #default="{ row }">
                {{ row.submittedByName || '—' }}
              </template>
            </el-table-column>
            <el-table-column :label="$t('已填行数')" width="90" align="right">
              <template #default="{ row }">
                {{ row.rows }}
              </template>
            </el-table-column>
          </el-table>
        </template>
      </template>
    </div>
  </el-dialog>
</template>

<style scoped lang="scss">
.empty {
  @apply flex flex-col items-center gap-2 rounded-md border border-dashed border-[#c8c8c8] px-6 py-10 text-center;
}
</style>
