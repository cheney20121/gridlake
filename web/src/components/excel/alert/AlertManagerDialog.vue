<script setup lang="ts">
import type { Alert, AlertInput, AlertMetric, AlertRun } from '@/api/alert'
import type { ReportChannel } from '@/api/report'
import { useIntervalFn } from '@vueuse/core'
import { ref, watch } from 'vue'
import { AGG_LABEL, alertRunsApi, deleteAlertApi, evaluateAlertApi, listAlertsApi, OP_LABEL, updateAlertApi, WINDOW_LABEL } from '@/api/alert'
import { t } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'
import AlertEditDialog from './AlertEditDialog.vue'

/** 阈值告警管理：指标超过阈值时推送到群 / 邮箱；新超标才通知，恢复后再超标重新通知 */
const props = defineProps<{ visible: boolean }>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 去添加推送渠道（渠道在「定时推送」里管理） */
  (e: 'addChannel'): void
}>()

const WEEKDAYS = [t('周一'), t('周二'), t('周三'), t('周四'), t('周五'), t('周六'), t('周日')]
const ledgerUser = useLedgerUserStore()

const items = ref<Alert[]>([])
const channels = ref<ReportChannel[]>([])
const smtpConfigured = ref(false)
const loading = ref(false)
const checking = ref<Set<string>>(new Set())
const editVisible = ref(false)
const editing = ref<Alert | null>(null)
const expanded = ref<string | null>(null)
const runs = ref<AlertRun[]>([])
const runsLoading = ref(false)

async function load(silent = false) {
  if (!silent)
    loading.value = true
  try {
    const r = await listAlertsApi()
    items.value = r.items
    channels.value = r.channels
    smtpConfigured.value = r.smtpConfigured
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, v => v && load(), { immediate: true })

const { pause, resume } = useIntervalFn(() => load(true), 30000, { immediate: false })
watch(() => props.visible, v => v ? resume() : pause(), { immediate: true })

function openCreate() {
  if (!channels.value.length) {
    ElMessage.info(t('先添加一个推送渠道（群机器人或邮箱）'))
    emit('addChannel')
    return
  }
  editing.value = null
  editVisible.value = true
}

function openEdit(a: Alert) {
  editing.value = a
  editVisible.value = true
}

/** 创建人或管理员才能改（没开登录时不区分） */
function canManage(a: Alert) {
  return !ledgerUser.authRequired || ledgerUser.isAdmin || (!!a.createdBy && a.createdBy === ledgerUser.user?.id)
}

function resultText(run: AlertRun) {
  if (!run.results.length)
    return run.message ?? ''
  const failed = run.results.filter(r => !r.ok)
  if (!failed.length)
    return t('已通知 {0} 项到 {1}', [run.notified, run.results.map(r => r.channelName).join('、')])
  return failed.map(r => `${r.channelName}：${r.message}`).join('；')
}

async function checkNow(a: Alert) {
  checking.value.add(a.id)
  try {
    const run = await evaluateAlertApi(a.id)
    if (run.status === 'failed')
      ElMessage.error({ message: t('「{0}」检查失败：{1}', [a.name, resultText(run)]), duration: 6000 })
    else if (run.status === 'partial')
      ElMessage.warning({ message: t('「{0}」部分渠道发送失败：{1}', [a.name, resultText(run)]), duration: 6000 })
    else if (run.notified > 0)
      ElMessage.success(t('「{0}」有 {1} 项超出条件，已通知', [a.name, run.notified]))
    else
      ElMessage.success(run.hits > 0 ? t('「{0}」有 {1} 项超出条件，都已通知过', [a.name, run.hits]) : t('「{0}」一切正常', [a.name]))
    await load(true)
    if (expanded.value === a.id)
      loadRuns(a.id)
  }
  finally {
    checking.value.delete(a.id)
  }
}

async function onSaved(id: string, check: boolean) {
  await load(true)
  const a = items.value.find(x => x.id === id)
  if (check && a)
    await checkNow(a)
}

/** 列表项 → 提交内容（只取规则本身的字段，去掉创建人、最近一次检查等只读信息） */
function toInput(a: Alert, enabled: boolean): AlertInput {
  return {
    name: a.name,
    datasetId: a.datasetId,
    metric: a.metric,
    metricLabel: a.metricLabel,
    groupBy: a.groupBy,
    filters: a.filters,
    window: a.window,
    condition: a.condition,
    guard: a.guard,
    schedule: a.schedule,
    channelIds: a.channelIds,
    repeat: a.repeat,
    notifyRecovery: a.notifyRecovery,
    enabled,
  }
}

async function toggle(a: Alert) {
  await updateAlertApi(a.id, toInput(a, !a.enabled))
  ElMessage.success(a.enabled ? t('已停用「{0}」', [a.name]) : t('已启用「{0}」', [a.name]))
  load(true)
}

async function remove(a: Alert) {
  const ok = await ElMessageBox.confirm(t('删除告警「{0}」及其检查记录？', [a.name]), t('删除告警'), { type: 'warning', confirmButtonText: t('删除') }).then(() => true, () => false)
  if (!ok)
    return
  await deleteAlertApi(a.id)
  ElMessage.success(t('已删除'))
  if (expanded.value === a.id)
    expanded.value = null
  load(true)
}

async function loadRuns(id: string) {
  runsLoading.value = true
  try {
    runs.value = await alertRunsApi(id)
  }
  finally {
    runsLoading.value = false
  }
}

function toggleRuns(a: Alert) {
  if (expanded.value === a.id) {
    expanded.value = null
    return
  }
  expanded.value = a.id
  runs.value = []
  loadRuns(a.id)
}

function metricText(a: Alert) {
  if (a.metricLabel)
    return a.metricLabel
  const part = (p: { column?: string | null, agg: keyof typeof AGG_LABEL }) => `${p.column || t('行数')}（${AGG_LABEL[p.agg]}）`
  const m: AlertMetric = a.metric
  if (m.kind === 'freshness')
    return t('「{0}」最新数据距今', [m.dateColumn])
  return m.kind === 'agg' ? part(m) : `${part(m.numerator)} / ${part(m.denominator)}`
}

function conditionText(a: Alert) {
  const basis = a.condition.basis ?? 'value'
  const percent = a.metric.kind === 'ratio' || basis === 'change_pct'
  const unit = a.metric.kind === 'freshness' ? ` ${t('天')}` : percent ? '%' : ''
  const v = percent ? `${Math.round(a.condition.threshold * 1e6) / 1e4}${unit}` : `${a.condition.threshold}${unit}`
  const subject = basis === 'value' ? metricText(a) : t('{0} 较上期变化', [metricText(a)])
  return `${subject} ${OP_LABEL[a.condition.op]} ${v}`
}

function scheduleText(a: Alert) {
  const s = a.schedule
  switch (s.kind) {
    case 'daily': return t('每天 {0}', [s.at])
    case 'weekly': return t('每{0} {1}', [WEEKDAYS[s.weekday - 1], s.at])
    case 'monthly': return t('每月 {0} 号 {1}', [s.day, s.at])
    case 'after_sync': return t('数据同步后')
    default: return t('仅手动')
  }
}

function channelNames(a: Alert) {
  return a.channelIds.map(id => channels.value.find(c => c.id === id)?.name ?? t('（已删除）')).join('、')
}

const STATUS: Record<AlertRun['status'], { text: string, cls: string }> = {
  running: { text: t('检查中'), cls: 'text-[#1D4ED8]' },
  success: { text: t('成功'), cls: 'text-[#0B5A2E]' },
  partial: { text: t('部分失败'), cls: 'text-[#92400E]' },
  failed: { text: t('失败'), cls: 'text-[#B91C1C]' },
}
const TRIGGER: Record<AlertRun['trigger'], string> = { manual: t('手动'), schedule: t('定时'), after_sync: t('同步后') }
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('阈值告警')"
    width="1040px"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="mb-3 flex items-center gap-3 text-[13px]">
      <span class="flex-1 text-xs text-gray-500 leading-5">
        {{ $t('指标超过阈值时推送到企业微信 / 钉钉 / 飞书群或邮箱。同一个问题只在新出现时通知，恢复后再出现会重新通知。') }}
      </span>
      <el-button type="primary" @click="openCreate">
        {{ $t('+ 新建告警') }}
      </el-button>
    </div>

    <div v-loading="loading" class="min-h-40">
      <div v-if="!items.length && !loading" class="empty">
        <div class="text-[15px] text-gray-800 font-medium">
          {{ $t('还没有告警') }}
        </div>
        <div class="text-xs text-gray-500 leading-5">
          {{ $t('例：每天 9:00 检查近 7 天各店铺的退款率，超过 5%（订单数不少于 30）就发到运营群；库存低于安全线时通知采购') }}
        </div>
        <el-button type="primary" class="mt-2" @click="openCreate">
          {{ $t('新建告警') }}
        </el-button>
      </div>

      <div v-for="a in items" :key="a.id" class="card" :class="{ 'opacity-60': !a.enabled }">
        <div class="flex items-center gap-3">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="text-gray-900 font-medium">{{ a.name }}</span>
              <el-tag v-if="!a.enabled" size="small" type="info">
                {{ $t('已停用') }}
              </el-tag>
              <el-tag v-if="a.firing > 0" size="small" type="danger">
                {{ $t('{0} 项触发中', [a.firing]) }}
              </el-tag>
              <span v-if="!a.datasetName" class="text-xs text-[#B91C1C]">{{ $t('数据集已删除') }}</span>
            </div>
            <div class="mt-0.5 flex flex-wrap items-center gap-x-3 text-xs text-gray-500">
              <span>{{ a.datasetName }}</span>
              <span>{{ conditionText(a) }}</span>
              <span v-if="a.groupBy">{{ $t('按「{0}」分组', [a.groupBy]) }}</span>
              <span v-if="a.window.preset !== 'none'">{{ WINDOW_LABEL[a.window.preset] }}</span>
            </div>
            <div class="mt-0.5 flex flex-wrap items-center gap-x-3 text-xs text-gray-500">
              <span>{{ scheduleText(a) }}</span>
              <span>→ {{ channelNames(a) }}</span>
              <span v-if="a.nextRun">{{ $t('下次 {0}', [a.nextRun.slice(5, 16)]) }}</span>
              <span v-if="a.createdByName">{{ $t('创建人 {0}', [a.createdByName]) }}</span>
            </div>
          </div>
          <div class="w-64 text-right text-xs leading-5">
            <template v-if="a.running || checking.has(a.id)">
              <span class="text-[#1D4ED8]">{{ $t('检查中…') }}</span>
            </template>
            <template v-else-if="a.lastRun">
              <div>
                <span :class="STATUS[a.lastRun.status].cls">{{ STATUS[a.lastRun.status].text }}</span>
                <span class="ml-1 text-gray-400">{{ TRIGGER[a.lastRun.trigger] }} {{ a.lastRun.startedAt.slice(5, 16) }}</span>
              </div>
              <div class="truncate text-gray-500" :title="resultText(a.lastRun)">
                {{ $t('命中 {0} 项', [a.lastRun.hits]) }}<template v-if="resultText(a.lastRun)">
                  · {{ resultText(a.lastRun) }}
                </template>
              </div>
            </template>
            <span v-else class="text-gray-400">{{ $t('尚未检查') }}</span>
          </div>
          <div class="flex shrink-0 items-center">
            <el-button size="small" type="primary" plain :loading="checking.has(a.id)" @click="checkNow(a)">
              {{ $t('立即检查') }}
            </el-button>
            <el-button link size="small" @click="toggleRuns(a)">
              {{ $t('记录') }}
            </el-button>
            <template v-if="canManage(a)">
              <el-button link size="small" @click="openEdit(a)">
                {{ $t('修改') }}
              </el-button>
              <el-button link size="small" @click="toggle(a)">
                {{ a.enabled ? $t('停用') : $t('启用') }}
              </el-button>
              <el-button link size="small" type="danger" :disabled="checking.has(a.id)" @click="remove(a)">
                {{ $t('删除') }}
              </el-button>
            </template>
          </div>
        </div>

        <div v-if="expanded === a.id" v-loading="runsLoading" class="mt-2 border-t border-[#eef0f3] border-t-solid pt-2">
          <el-table :data="runs" size="small" max-height="280" :empty-text="$t('暂无检查记录')">
            <el-table-column :label="$t('时间')" prop="startedAt" width="150" />
            <el-table-column :label="$t('方式')" width="66">
              <template #default="{ row }">
                {{ TRIGGER[row.trigger as AlertRun['trigger']] }}
              </template>
            </el-table-column>
            <el-table-column :label="$t('结果')" width="80">
              <template #default="{ row }">
                <span :class="STATUS[row.status as AlertRun['status']].cls">{{ STATUS[row.status as AlertRun['status']].text }}</span>
              </template>
            </el-table-column>
            <el-table-column :label="$t('命中 / 通知')" width="96">
              <template #default="{ row }">
                {{ row.hits }} / {{ row.notified }}
              </template>
            </el-table-column>
            <el-table-column :label="$t('各渠道')" min-width="360">
              <template #default="{ row }">
                <div v-if="!row.results.length" :class="row.status === 'failed' ? 'text-[#B91C1C]' : 'text-gray-500'">
                  {{ row.message }}
                </div>
                <div v-for="x in (row as AlertRun).results" :key="x.channelId" class="leading-5">
                  <span :class="x.ok ? 'text-[#0B5A2E]' : 'text-[#B91C1C]'">{{ x.ok ? '✓' : '✗' }}</span>
                  {{ x.channelName }}
                  <span v-if="x.message" class="text-[#B91C1C]">：{{ x.message }}</span>
                </div>
                <div v-if="row.results.length && row.message" class="text-[#92400E]">
                  {{ row.message }}
                </div>
              </template>
            </el-table-column>
          </el-table>
        </div>
      </div>
    </div>

    <AlertEditDialog
      v-model:visible="editVisible"
      :alert="editing"
      :channels="channels"
      :smtp-configured="smtpConfigured"
      @saved="onSaved"
      @add-channel="emit('addChannel')"
    />
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
