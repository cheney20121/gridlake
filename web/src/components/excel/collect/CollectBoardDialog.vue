<script setup lang="ts">
import type { Collect, CollectDetail, CollectStatus, ReviewAction } from '@/api/collect'
import { useIntervalFn } from '@vueuse/core'
import { computed, reactive, ref, watch } from 'vue'
import { collectDetailApi, deleteCollectPeriodApi, openCollectPeriodApi, remindCollectApi, reviewCollectApi, setCollectDeadlineApi } from '@/api/collect'
import { t } from '@/i18n'

/**
 * 收集看板：选一期，看每个单位谁填了、谁没填；开启新一期；复制未填名单去催；
 * 确认人（或管理员）在这里确认并锁定、退回重填
 */
const props = defineProps<{
  visible: boolean
  collect: Pick<Collect, 'id' | 'name' | 'periodColumn'> | null
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 期次或提交状态变了，列表里的进度要刷新 */
  (e: 'changed'): void
}>()

const detail = ref<CollectDetail | null>(null)
const period = ref('')
const filter = ref<'all' | 'pending' | 'submitted' | 'confirmed'>('all')
const reviewing = ref(false)
const reminding = ref(false)
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
const confirmedPercent = computed(() => (current.value && current.value.total ? Math.round((current.value.confirmed / current.value.total) * 100) : 0))
/** 未提交 = 还没填的 + 被退回还没改好的 */
const isOpen = (s: CollectStatus) => s === 'pending' || s === 'returned'
const rows = computed(() => (detail.value?.units ?? []).filter(u => filter.value === 'all' || (filter.value === 'pending' ? isOpen(u.status) : u.status === filter.value)))
const pendingUnits = computed(() => (detail.value?.units ?? []).filter(u => isOpen(u.status)))
const toConfirm = computed(() => (detail.value?.units ?? []).filter(u => u.status === 'submitted'))

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

/** 开启新一期 / 改截止日期共用的小对话框 */
const form = reactive({ visible: false, mode: 'open' as 'open' | 'deadline', period: '', deadline: '', saving: false })

function openPeriod() {
  Object.assign(form, { visible: true, mode: 'open', period: suggestPeriod(), deadline: '' })
}

function editDeadline() {
  if (current.value)
    Object.assign(form, { visible: true, mode: 'deadline', period: current.value.period, deadline: current.value.deadline ?? '' })
}

async function saveForm() {
  if (!props.collect)
    return
  const period = form.period.trim()
  if (!period)
    return ElMessage.warning(t('请输入期次'))
  form.saving = true
  try {
    if (form.mode === 'open') {
      await openCollectPeriodApi(props.collect.id, period, form.deadline || null)
      ElMessage.success(t('已开启「{0}」，负责人现在可以填报了', [period]))
    }
    else {
      await setCollectDeadlineApi(props.collect.id, period, form.deadline || null)
      ElMessage.success(t('已保存'))
    }
    form.visible = false
    await load(period)
    emit('changed')
  }
  finally {
    form.saving = false
  }
}

/** 离截止日期还有几天（今天 0，过了是负数） */
function leftDays(d: string) {
  const today = new Date()
  today.setHours(0, 0, 0, 0)
  return Math.round((new Date(`${d}T00:00:00`).getTime() - today.getTime()) / 86400000)
}

const deadlineText = computed(() => {
  const c = current.value
  if (!c?.deadline)
    return ''
  const n = leftDays(c.deadline)
  if (n < 0)
    return t('截止 {0}，已逾期 {1} 天，{2} 个单位没交', [c.deadline, -n, c.overdue])
  return n === 0 ? t('截止 {0}（今天）', [c.deadline]) : t('截止 {0}（还剩 {1} 天）', [c.deadline, n])
})

/** 催办没提交的单位（不传 units 就是这一期所有没提交的）：负责人收到站内通知，任务配了渠道的还推送名单 */
async function remind(units?: string[]) {
  if (!props.collect || !current.value)
    return
  const channelHint = detail.value?.collect.remindChannels.length ? t('，名单也会推送到配置的群 / 邮箱') : ''
  const ok = await ElMessageBox.confirm(
    units ? t('催办「{0}」？负责人会收到站内通知{1}。', [units[0], channelHint]) : t('催办这 {0} 个没提交的单位？负责人会收到站内通知{1}。', [pendingUnits.value.length, channelHint]),
    t('催办'),
    { confirmButtonText: t('催办'), cancelButtonText: t('取消') },
  ).then(() => true, () => false)
  if (!ok)
    return
  reminding.value = true
  try {
    const r = await remindCollectApi(props.collect.id, current.value.period, units)
    const parts = [t('已通知 {0} 位负责人', [r.notified])]
    if (r.skipped)
      parts.push(t('{0} 个刚催过，已跳过', [r.skipped]))
    if (r.unassigned)
      parts.push(t('{0} 个没有负责人', [r.unassigned]))
    const failed = r.channels.filter(c => !c.ok)
    if (r.channels.length)
      parts.push(failed.length ? t('推送失败：{0}', [failed.map(c => c.channelName).join('、')]) : t('已推送到 {0} 个渠道', [r.channels.length]))
    const message = parts.join('；')
    failed.length ? ElMessage.warning(message) : ElMessage.success(message)
    await load(current.value.period, true)
  }
  finally {
    reminding.value = false
  }
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

/** 确认并锁定（可以一次多个）/ 退回重填（要写原因）/ 撤销确认 */
async function doReview(action: ReviewAction, units: string[], reason?: string) {
  if (!props.collect || !current.value || !units.length)
    return
  reviewing.value = true
  try {
    await reviewCollectApi(props.collect.id, { period: current.value.period, units, action, reason })
    await load(current.value.period, true)
    emit('changed')
  }
  finally {
    reviewing.value = false
  }
}

async function confirmUnits(units: string[]) {
  const ok = await ElMessageBox.confirm(
    units.length === 1 ? t('确认「{0}」这一期的数据？确认后锁定，负责人不能再改；要改得先退回。', [units[0]]) : t('确认这 {0} 个单位这一期的数据？确认后都会锁定，负责人不能再改；要改得先退回。', [units.length]),
    t('确认并锁定'),
    { confirmButtonText: t('确认'), cancelButtonText: t('取消') },
  ).then(() => true, () => false)
  if (!ok)
    return
  await doReview('confirm', units)
  ElMessage.success(t('已确认 {0} 个单位', [units.length]))
}

async function returnUnit(unit: string) {
  const reason = await ElMessageBox.prompt(t('写明哪里要改，负责人会收到通知：'), t('退回「{0}」重填', [unit]), {
    inputType: 'textarea',
    inputPattern: /\S/,
    inputErrorMessage: t('请写退回原因'),
    inputPlaceholder: t('如：营收口径不对，按含税填'),
    confirmButtonText: t('退回'),
    cancelButtonText: t('取消'),
  }).then(r => r.value.trim(), () => '')
  if (!reason)
    return
  await doReview('return', [unit], reason)
  ElMessage.success(t('已退回，负责人改完重新提交后会回到「待确认」'))
}

async function reopenUnit(unit: string) {
  const ok = await ElMessageBox.confirm(t('撤销「{0}」的确认？撤销后负责人可以再改。', [unit]), t('撤销确认'), { confirmButtonText: t('撤销确认'), cancelButtonText: t('取消') }).then(() => true, () => false)
  if (!ok)
    return
  await doReview('reopen', [unit])
  ElMessage.success(t('已撤销确认'))
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
    width="min(1080px, 96vw)"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div v-loading="loading" class="min-h-48">
      <template v-if="detail">
        <div class="mb-3 flex flex-wrap items-center gap-2 text-[13px]">
          <el-select v-model="period" class="!w-44" :placeholder="$t('还没有期次')" :disabled="!detail.periods.length" @change="load(period)">
            <el-option v-for="p in detail.periods" :key="p.period" :label="`${p.period}（${p.submitted}/${p.total}）`" :value="p.period" />
          </el-select>
          <el-button v-if="detail.canManage" type="primary" @click="openPeriod">
            {{ $t('+ 开启新一期') }}
          </el-button>
          <el-button v-if="detail.canManage && current && !current.submitted" link type="danger" @click="removePeriod">
            {{ $t('删除这一期') }}
          </el-button>
          <el-button link :loading="loading" @click="load(period)">
            {{ $t('刷新') }}
          </el-button>
          <span v-if="deadlineText" class="text-xs" :class="current?.overdue ? 'text-[#B91C1C] font-medium' : 'text-gray-500'">{{ deadlineText }}</span>
          <el-button v-if="detail.canManage && current" link type="primary" @click="editDeadline">
            {{ current.deadline ? $t('改截止日期') : $t('设置截止日期') }}
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
            {{ detail.canManage ? $t('开启一期（如本月）后，负责人才能往数据集里填；提交时填报区要带上单位列和期次列。') : $t('管理员还没有开启期次') }}
          </div>
          <el-button v-if="detail.canManage" type="primary" class="mt-2" @click="openPeriod">
            {{ $t('开启第一期') }}
          </el-button>
        </div>

        <template v-else-if="current">
          <div class="mb-3 flex flex-wrap items-center gap-3">
            <el-progress :percentage="percent" :stroke-width="10" class="w-60" :show-text="false" />
            <span class="text-[13px]">
              <b>{{ current.submitted }}</b> / {{ current.total }} {{ $t('已提交') }}
              <span v-if="current.total - current.submitted" class="ml-2 text-[#B91C1C]">{{ $t('{0} 个未提交', [current.total - current.submitted]) }}</span>
              <span v-else class="ml-2 text-[#0B5A2E]">{{ $t('全部已提交') }}</span>
              <span class="ml-2 text-gray-500">{{ $t('已确认 {0}（{1}%）', [current.confirmed, confirmedPercent]) }}</span>
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
                {{ $t('待确认') }}
              </el-radio-button>
              <el-radio-button value="confirmed">
                {{ $t('已确认') }}
              </el-radio-button>
            </el-radio-group>
            <el-button size="small" type="primary" plain :disabled="!toConfirm.length" :loading="reviewing" @click="confirmUnits(toConfirm.map(u => u.unit))">
              {{ $t('确认全部待确认的（{0}）', [toConfirm.length]) }}
            </el-button>
            <el-button size="small" type="warning" plain :disabled="!pendingUnits.length" :loading="reminding" @click="remind()">
              {{ $t('催办未提交的（{0}）', [pendingUnits.length]) }}
            </el-button>
            <el-button size="small" :disabled="!pendingUnits.length" @click="copyPending">
              {{ $t('复制未提交名单') }}
            </el-button>
          </div>
          <div class="mb-2 text-xs text-gray-500">
            {{ detail.collect.reviewerName ? $t('确认人：{0}（管理员也可以审核）', [detail.collect.reviewerName]) : $t('还没有指定确认人，现在只有管理员能审核；可以在「修改」里指定') }}
          </div>

          <el-table :data="rows" size="small" max-height="440" :empty-text="$t('没有符合的单位')">
            <el-table-column :label="detail.collect.unitColumn" prop="unit" min-width="100" show-overflow-tooltip />
            <el-table-column :label="$t('负责人')" min-width="120" show-overflow-tooltip>
              <template #default="{ row }">
                <span v-if="row.userName">{{ row.userName }}</span>
                <span v-else class="text-gray-400">{{ $t('未指派（管理员代填）') }}</span>
              </template>
            </el-table-column>
            <el-table-column :label="$t('状态')" width="125">
              <template #default="{ row }">
                <el-tag v-if="row.status === 'confirmed'" size="small" type="success">
                  {{ $t('已确认') }}
                </el-tag>
                <el-tag v-else-if="row.status === 'submitted'" size="small" type="warning">
                  {{ $t('待确认') }}
                </el-tag>
                <el-tag v-else-if="row.status === 'returned'" size="small" type="danger" effect="dark">
                  {{ $t('已退回') }}
                </el-tag>
                <el-tag v-else size="small" type="danger">
                  {{ $t('未提交') }}
                </el-tag>
                <el-tag v-if="row.overdue" size="small" type="danger" effect="dark" class="ml-1">
                  {{ $t('逾期') }}
                </el-tag>
              </template>
            </el-table-column>
            <el-table-column :label="$t('提交时间')" prop="submittedAt" width="150">
              <template #default="{ row }">
                {{ row.submittedAt ?? '—' }}
              </template>
            </el-table-column>
            <el-table-column :label="$t('提交人')" width="80" show-overflow-tooltip>
              <template #default="{ row }">
                {{ row.submittedByName || '—' }}
              </template>
            </el-table-column>
            <el-table-column :label="$t('已填行数')" width="80" align="right">
              <template #default="{ row }">
                {{ row.rows }}
              </template>
            </el-table-column>
            <el-table-column :label="$t('审核')" min-width="150" show-overflow-tooltip>
              <template #default="{ row }">
                <span v-if="row.status === 'returned'" class="text-[#B91C1C]">{{ row.returnReason }}（{{ row.reviewedByName }} {{ row.reviewedAt.slice(5, 16) }}）</span>
                <span v-else-if="row.status === 'confirmed'" class="text-gray-500">{{ $t('{0} 于 {1} 确认，已锁定', [row.reviewedByName, row.reviewedAt.slice(5, 16)]) }}</span>
                <span v-else-if="row.remindedAt" class="text-gray-400">{{ $t('上次催办 {0}', [row.remindedAt.slice(5, 16)]) }}</span>
                <span v-else class="text-gray-300">—</span>
              </template>
            </el-table-column>
            <el-table-column width="130" align="center" fixed="right">
              <template #default="{ row }">
                <template v-if="row.status === 'submitted'">
                  <el-button link type="primary" size="small" :disabled="reviewing" @click="confirmUnits([row.unit])">
                    {{ $t('确认') }}
                  </el-button>
                  <el-button link type="danger" size="small" :disabled="reviewing" @click="returnUnit(row.unit)">
                    {{ $t('退回') }}
                  </el-button>
                </template>
                <template v-else-if="(row.status === 'pending' || row.status === 'returned') && row.userId">
                  <el-button link type="warning" size="small" :disabled="reminding" @click="remind([row.unit])">
                    {{ $t('催办') }}
                  </el-button>
                </template>
                <template v-else-if="row.status === 'confirmed'">
                  <el-button link size="small" :disabled="reviewing" @click="reopenUnit(row.unit)">
                    {{ $t('撤销确认') }}
                  </el-button>
                  <el-button link type="danger" size="small" :disabled="reviewing" @click="returnUnit(row.unit)">
                    {{ $t('退回') }}
                  </el-button>
                </template>
              </template>
            </el-table-column>
          </el-table>
        </template>
      </template>
    </div>

    <el-dialog v-model="form.visible" :title="form.mode === 'open' ? $t('开启新一期') : $t('截止日期 · {0}', [form.period])" width="420px" append-to-body :close-on-click-modal="false">
      <el-form label-width="84px" @submit.prevent>
        <el-form-item v-if="form.mode === 'open'" :label="$t('期次')" required>
          <el-input v-model="form.period" maxlength="20" :placeholder="$t('如 2026-10、2026-W42')" />
          <div class="hint">
            {{ $t('填报时要填在「{0}」列里', [collect?.periodColumn]) }}
          </div>
        </el-form-item>
        <el-form-item :label="$t('截止日期')">
          <el-date-picker v-model="form.deadline" type="date" value-format="YYYY-MM-DD" format="YYYY-MM-DD" clearable :placeholder="$t('不设置')" class="!w-full" />
          <div class="hint">
            {{ $t('当天 24 点前都算按时，过了还没提交的显示「逾期」；只是提示，不拦截提交。设了之后会自动催办：截止前一天、当天，逾期后每天一次（最多 7 天，每天 9 点以后）。') }}
          </div>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="form.visible = false">
          {{ $t('取消') }}
        </el-button>
        <el-button type="primary" :loading="form.saving" @click="saveForm">
          {{ form.mode === 'open' ? $t('开启') : $t('保存') }}
        </el-button>
      </template>
    </el-dialog>
  </el-dialog>
</template>

<style scoped lang="scss">
.hint {
  @apply mt-1 w-full text-xs text-gray-500 leading-5;
}

.empty {
  @apply flex flex-col items-center gap-2 rounded-md border border-dashed border-[#c8c8c8] px-6 py-10 text-center;
}
</style>
