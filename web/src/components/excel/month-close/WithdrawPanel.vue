<script setup lang="ts">
import type { LedgerBankBatch, LedgerBankImportResult, LedgerBankTxn, LedgerWithdrawRow, LedgerWithdrawStatus, LedgerWithdrawView } from '@/api/ledger'
import { computed, reactive, ref, watch } from 'vue'
import {
  autoMatchWithdrawalsApi,
  deleteLedgerBankBatchApi,
  getLedgerBankBatchesApi,
  getLedgerWithdrawalsApi,
  getWithdrawCandidatesApi,
  importLedgerBankApi,
  matchWithdrawalApi,
} from '@/api/ledger'
import { formatMoney } from '@/components/excel/ledger-detail/columns'
import { WITHDRAW_STATUS } from '@/features/excel/shop-ledger/month-close'
import { shortEntityName } from '@/features/excel/shop-ledger/summary-template'

const props = defineProps<{
  period: string
  operator: string
  frozenShops: Set<string>
  shop?: string
}>()

const emit = defineEmits<{
  (e: 'changed'): void
}>()

const view = ref<LedgerWithdrawView | null>(null)
const batches = ref<LedgerBankBatch[]>([])
const loading = ref(false)
const statusFilter = ref<LedgerWithdrawStatus | ''>('')

async function load() {
  if (!props.period)
    return
  loading.value = true
  try {
    ;[view.value, batches.value] = await Promise.all([getLedgerWithdrawalsApi(props.period), getLedgerBankBatchesApi()])
  }
  finally {
    loading.value = false
  }
}

watch(() => props.period, load, { immediate: true })
defineExpose({ load })

const rows = computed(() => (view.value?.rows ?? [])
  .filter(r => !props.shop || r.shop === props.shop)
  .filter(r => !statusFilter.value || r.status === statusFilter.value))

const STAT_CARDS = [
  { key: 'matched', label: '已到账', count: 'matched', amount: 'matchedAmount', cls: 'c-ok' },
  { key: 'in_transit', label: '在途（到账期内）', count: 'inTransit', amount: 'inTransitAmount', cls: 'c-warn' },
  { key: 'no_bank', label: '缺银行流水', count: 'noBank', amount: 'noBankAmount', cls: 'c-info' },
  { key: 'overdue', label: '逾期未到账', count: 'overdue', amount: 'overdueAmount', cls: 'c-bad' },
] as const

/** 本月涉及的主体（导入银行流水时选择） */
const entities = computed(() => [...new Set([...(view.value?.rows ?? []).map(r => r.entity), ...(view.value?.banks ?? []).map(b => b.entity)])])

/** 确认框：取消时返回 false（ElMessageBox 取消会 reject） */
function confirmBox(message: string, title: string, confirmButtonText = '确定') {
  return ElMessageBox.confirm(message, title, { type: 'warning', confirmButtonText }).then(() => true, () => false)
}

function requireOperator() {
  if (props.operator.trim())
    return true
  ElMessage.warning('请先在右上角填写操作人')
  return false
}

// ── 自动配对 ──
const matching = ref(false)
async function autoMatch() {
  if (!requireOperator())
    return
  matching.value = true
  try {
    const r = await autoMatchWithdrawalsApi(props.period, props.operator)
    ElMessage.success(`自动配对 ${r.matched} 笔，剩余 ${r.unmatched} 笔未配对`)
    await load()
    emit('changed')
  }
  finally {
    matching.value = false
  }
}

// ── 导入银行流水 ──
const bankDialog = reactive({
  visible: false,
  entity: '',
  accountNo: '',
  file: null as File | null,
  uploading: false,
  result: null as LedgerBankImportResult | null,
})

function openBankDialog() {
  Object.assign(bankDialog, { visible: true, entity: entities.value[0] ?? '', accountNo: '', file: null, result: null })
}

function onFileChange(e: Event) {
  bankDialog.file = (e.target as HTMLInputElement).files?.[0] ?? null
  bankDialog.result = null
}

const MAPPING_LABEL: Record<string, string> = {
  date: '日期',
  time: '时间',
  inflow: '收入',
  outflow: '支出',
  amount: '金额',
  direction: '借贷标志',
  counterparty: '对方户名',
  summary: '摘要',
  serialNo: '流水号',
  accountNo: '本方账号',
}

async function uploadBank() {
  if (!bankDialog.file || !bankDialog.entity)
    return
  bankDialog.uploading = true
  try {
    bankDialog.result = await importLedgerBankApi(bankDialog.file, bankDialog.entity, bankDialog.accountNo)
    await load()
  }
  finally {
    bankDialog.uploading = false
  }
}

async function removeBatch(b: LedgerBankBatch) {
  const ok = await confirmBox(
    `删除「${b.fileName ?? b.batchId}」的 ${b.rowCount} 条银行流水${b.matched ? `，并取消其中 ${b.matched} 笔已配对的提现` : ''}？`,
    '删除银行流水',
    '删除',
  )
  if (!ok)
    return
  await deleteLedgerBankBatchApi(b.batchId)
  ElMessage.success('已删除')
  await load()
  emit('changed')
}

// ── 手工配对 ──
const matchDialog = reactive({
  visible: false,
  row: null as LedgerWithdrawRow | null,
  candidates: [] as LedgerBankTxn[],
  loading: false,
})

async function openMatch(row: LedgerWithdrawRow) {
  Object.assign(matchDialog, { visible: true, row, candidates: [], loading: true })
  try {
    matchDialog.candidates = await getWithdrawCandidatesApi(row.shop, row.period, row.serialNo)
  }
  finally {
    matchDialog.loading = false
  }
}

async function pick(txn: LedgerBankTxn) {
  const row = matchDialog.row
  if (!row || !requireOperator())
    return
  const diff = Math.round((txn.amount - row.amount) * 100) / 100
  if (diff !== 0 && !await confirmBox(
    `到账 ${formatMoney(txn.amount)} 与提现 ${formatMoney(row.amount)} 相差 ${formatMoney(diff)}（如手续费），确认配对？`,
    '金额不一致',
    '确认配对',
  )) {
    return
  }
  await matchWithdrawalApi({ shop: row.shop, period: row.period, serialNo: row.serialNo, txnId: txn.txnId, operator: props.operator })
  ElMessage.success('已配对')
  matchDialog.visible = false
  await load()
  emit('changed')
}

async function unmatch(row: LedgerWithdrawRow) {
  if (!requireOperator())
    return
  if (!await confirmBox(`取消提现 ${formatMoney(row.amount)}（${row.time}）与银行流水的配对？`, '取消配对'))
    return
  await matchWithdrawalApi({ shop: row.shop, period: row.period, serialNo: row.serialNo, txnId: null, operator: props.operator })
  await load()
  emit('changed')
}

const lagText = (r: LedgerWithdrawRow) => r.lagDays == null ? '' : r.lagDays === 0 ? '当天到账' : `T+${r.lagDays}`
</script>

<template>
  <div v-loading="loading" class="h-full flex flex-col gap-3">
    <!-- 统计 -->
    <div v-if="view" class="flex gap-3">
      <button
        v-for="c in STAT_CARDS"
        :key="c.key"
        class="stat"
        :class="[c.cls, { active: statusFilter === c.key }]"
        @click="statusFilter = statusFilter === c.key ? '' : c.key"
      >
        <div class="text-xs opacity-80">
          {{ c.label }}
        </div>
        <div class="text-lg font-semibold">
          {{ view.stats[c.count] }} 笔
        </div>
        <div class="text-xs">
          {{ formatMoney(view.stats[c.amount]) }}
        </div>
      </button>
      <div class="flex flex-1 flex-col justify-between gap-2">
        <div class="flex justify-end gap-2">
          <el-button size="small" @click="openBankDialog">
            导入银行流水
          </el-button>
          <el-button size="small" type="primary" :loading="matching" @click="autoMatch">
            自动配对
          </el-button>
        </div>
        <div class="text-right text-xs text-gray-400 leading-5">
          按 同主体 + 金额相等 + 提现后 0~{{ view.windowDays }} 天到账 自动配对；超过 {{ view.windowDays }} 天未到账为逾期
          <div v-for="b in view.banks" :key="b.entity">
            {{ shortEntityName(b.entity) }} 银行流水：
            <span v-if="b.rowCount">{{ b.timeFrom?.slice(0, 10) }} ~ {{ b.timeTo?.slice(0, 10) }}（{{ b.rowCount }} 条）</span>
            <span v-else class="text-[#92400E]">未导入</span>
          </div>
        </div>
      </div>
    </div>

    <el-table :data="rows" header-row-class-name="global-table-header" border class="withdraw-table" height="100%">
      <el-table-column label="提现时间" prop="time" width="160" />
      <el-table-column v-if="!shop" label="店铺" prop="shop" min-width="120" show-overflow-tooltip />
      <el-table-column label="主体" width="110">
        <template #default="{ row }">
          <el-tooltip :content="row.entity" placement="top">
            <span>{{ shortEntityName(row.entity) }}</span>
          </el-tooltip>
        </template>
      </el-table-column>
      <el-table-column label="提现金额" align="right" width="130">
        <template #default="{ row }">
          {{ formatMoney(row.amount) }}
        </template>
      </el-table-column>
      <el-table-column label="状态" align="center" width="110">
        <template #default="{ row }">
          <el-tag :type="WITHDRAW_STATUS[row.status as LedgerWithdrawStatus].type" size="small">
            {{ WITHDRAW_STATUS[row.status as LedgerWithdrawStatus].label }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column label="银行到账" min-width="260">
        <template #default="{ row }">
          <div v-if="row.txn" class="leading-5">
            <span>{{ row.txn.time }}</span>
            <span class="ml-2 text-gray-500">{{ lagText(row) }}</span>
            <span v-if="row.diff" class="ml-2 font-semibold text-[#B91C1C]">差 {{ formatMoney(row.diff) }}</span>
            <div class="text-xs text-gray-400">
              {{ row.txn.counterparty }} · {{ row.txn.summary }}
              <span class="ml-1">{{ row.method === 'manual' ? `手工配对 · ${row.matchedBy ?? ''}` : '自动配对' }}</span>
            </div>
          </div>
          <span v-else-if="row.status === 'in_transit'" class="text-xs text-gray-500">预计 {{ row.dueDate }} 前到账</span>
          <span v-else-if="row.status === 'no_bank'" class="text-xs text-gray-500">需导入覆盖到 {{ row.dueDate }} 的银行流水</span>
          <span v-else class="text-xs text-[#B91C1C]">应于 {{ row.dueDate }} 前到账，银行流水中未找到</span>
        </template>
      </el-table-column>
      <el-table-column label="操作" width="110" align="center">
        <template #default="{ row }">
          <span v-if="frozenShops.has(row.shop)" class="text-xs text-[#0B5A2E]">🔒 已冻结</span>
          <el-button v-else-if="row.txn" link type="danger" size="small" @click="unmatch(row)">
            取消配对
          </el-button>
          <el-button v-else link type="primary" size="small" @click="openMatch(row)">
            手工配对
          </el-button>
        </template>
      </el-table-column>
    </el-table>

    <!-- 银行有、平台无 -->
    <div v-if="view?.unmatchedBank.length" class="rounded bg-[#FFFBEB] px-3 py-2 text-[13px]">
      <div class="mb-1 font-medium text-[#92400E]">
        银行收到、未对应任何提现的平台款项 {{ view.unmatchedBank.length }} 笔（可能是平台退款 / 赔付，或提现被拆分到账，请核实）
      </div>
      <div v-for="t in view.unmatchedBank" :key="t.txnId" class="text-xs leading-5 text-gray-700">
        {{ t.time }} · {{ shortEntityName(t.entity) }} · <b>{{ formatMoney(t.amount) }}</b> · {{ t.counterparty }} · {{ t.summary }}
      </div>
    </div>

    <!-- 导入银行流水 -->
    <el-dialog v-model="bankDialog.visible" title="导入银行流水" width="600px" append-to-body>
      <el-form label-width="84px" size="small">
        <el-form-item label="所属主体" required>
          <el-select v-model="bankDialog.entity" class="w-full" filterable allow-create placeholder="银行账户所属的商户主体">
            <el-option v-for="e in entities" :key="e" :label="e" :value="e" />
          </el-select>
        </el-form-item>
        <el-form-item label="银行账号">
          <el-input v-model="bankDialog.accountNo" placeholder="可选；文件中有「本方账号」列时自动读取" />
        </el-form-item>
        <el-form-item label="文件" required>
          <input type="file" accept=".csv,.xls,.xlsx" @change="onFileChange">
          <div class="w-full text-xs text-gray-400">
            支持网银导出的 CSV / XLS / XLSX，自动识别表头（前面的账户信息行会跳过）；同一流水重复导入自动去重
          </div>
        </el-form-item>
      </el-form>
      <div v-if="bankDialog.result" class="rounded bg-[#F7F8FA] p-3 text-[13px] leading-6">
        <div class="font-medium text-[#0B5A2E]">
          新增 {{ bankDialog.result.insertedRows }} 条
          <span v-if="bankDialog.result.skippedRows" class="text-gray-500">，重复跳过 {{ bankDialog.result.skippedRows }} 条</span>
          <span v-if="bankDialog.result.ignoredRows" class="text-gray-500">，忽略 {{ bankDialog.result.ignoredRows }} 行（合计 / 空行）</span>
        </div>
        <div>
          {{ bankDialog.result.timeFrom }} ~ {{ bankDialog.result.timeTo }}，收入 {{ bankDialog.result.inflowCount }} 笔 {{ formatMoney(bankDialog.result.inflowAmount) }}
        </div>
        <div class="text-xs text-gray-500">
          第 {{ bankDialog.result.headerRow }} 行为表头，识别列：
          <template v-for="(v, k) in bankDialog.result.mapping" :key="k">
            <span v-if="v" class="mr-2">{{ MAPPING_LABEL[k] }}=「{{ v }}」</span>
          </template>
        </div>
      </div>
      <template #footer>
        <el-button @click="bankDialog.visible = false">
          {{ bankDialog.result ? '完成' : '取消' }}
        </el-button>
        <el-button
          type="primary"
          :disabled="!bankDialog.file || !bankDialog.entity"
          :loading="bankDialog.uploading"
          @click="uploadBank"
        >
          导入
        </el-button>
      </template>

      <div v-if="batches.length" class="mt-4">
        <div class="mb-1 text-xs text-gray-500">
          已导入批次
        </div>
        <div v-for="b in batches" :key="b.batchId" class="flex items-center gap-2 text-xs leading-6">
          <span class="w-28 truncate">{{ shortEntityName(b.entity) }}</span>
          <span class="flex-1 truncate">{{ b.fileName }}</span>
          <span class="text-gray-400">{{ b.timeFrom.slice(0, 10) }} ~ {{ b.timeTo.slice(0, 10) }} · {{ b.rowCount }} 条 · 已配对 {{ b.matched }}</span>
          <el-button link type="danger" size="small" @click="removeBatch(b)">
            删除
          </el-button>
        </div>
      </div>
    </el-dialog>

    <!-- 手工配对 -->
    <el-dialog v-model="matchDialog.visible" title="手工配对银行到账" width="680px" append-to-body>
      <div v-if="matchDialog.row" class="mb-2 text-[13px]">
        提现：{{ matchDialog.row.time }} · {{ shortEntityName(matchDialog.row.entity) }} · <b>{{ formatMoney(matchDialog.row.amount) }}</b>
      </div>
      <el-table v-loading="matchDialog.loading" :data="matchDialog.candidates" size="small" max-height="360" empty-text="提现前 1 天 ~ 后 10 天内没有未配对的银行收入">
        <el-table-column label="到账时间" prop="time" width="160" />
        <el-table-column label="金额" align="right" width="120">
          <template #default="{ row }">
            <span :class="{ 'text-[#B91C1C]': Math.abs(row.amount - (matchDialog.row?.amount ?? 0)) >= 0.005 }">{{ formatMoney(row.amount) }}</span>
          </template>
        </el-table-column>
        <el-table-column label="对方 / 摘要" min-width="200">
          <template #default="{ row }">
            {{ row.counterparty }} · {{ row.summary }}
          </template>
        </el-table-column>
        <el-table-column width="70" align="center">
          <template #default="{ row }">
            <el-button link type="primary" size="small" @click="pick(row)">
              配对
            </el-button>
          </template>
        </el-table-column>
      </el-table>
    </el-dialog>
  </div>
</template>

<style scoped lang="scss">
.stat {
  @apply w-40 rounded border border-solid border-transparent px-3 py-2 text-left;

  &.active {
    @apply border-current;
  }
}

.c-ok {
  @apply bg-[#E7F4EC] text-[#0B5A2E];
}

.c-warn {
  @apply bg-[#FEF3C7] text-[#92400E];
}

.c-info {
  @apply bg-gray-100 text-gray-600;
}

.c-bad {
  @apply bg-[#FEE2E2] text-[#B91C1C];
}

// 隐藏 el-table border 属性的可见边框，但保留 resizable 能力
.withdraw-table {
  --#{$namespace}-table-border-color: transparent;

  &.#{$namespace}-table--border::before,
  &.#{$namespace}-table--border::after {
    display: none;
  }

  :deep(.#{$namespace}-table__inner-wrapper::after),
  :deep(.#{$namespace}-table__inner-wrapper::before) {
    display: none;
  }

  :deep(.#{$namespace}-table__cell) {
    border-right: none;
  }

  :deep(th.#{$namespace}-table__cell) {
    border-bottom: 1px solid #ebeef5;
  }

  :deep(td.#{$namespace}-table__cell) {
    border-bottom: 1px solid #f0f2f5;
  }

  :deep(thead .#{$namespace}-table__cell:not(:last-child)) {
    position: relative;

    &::after {
      content: '';
      position: absolute;
      top: 30%;
      bottom: 30%;
      right: 0;
      width: 1px;
      background: #cfcfd4;
      pointer-events: none;
    }
  }
}
</style>
