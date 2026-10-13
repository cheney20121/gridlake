<script setup lang="ts">
import type { LedgerBalanceRow } from '@/packs/ecommerce/api'
import { computed, ref, watch } from 'vue'
import { t } from '@/i18n'
import { getLedgerBalancesApi, saveLedgerBalancesApi } from '@/packs/ecommerce/api'
import { shortEntityName } from '@/packs/ecommerce/shop-ledger/summary-template'
import { formatMoney } from '@/utils/format-money'

const props = defineProps<{
  period: string
  operator: string
  /** 冻结（审批中 / 已锁账）的店铺 */
  frozenShops: Set<string>
  /** 只看某个店铺 */
  shop?: string
}>()

const emit = defineEmits<{
  (e: 'saved'): void
}>()

interface EditRow extends LedgerBalanceRow {
  editOpening: number | null
  editClosing: number | null
  editRemark: string
}

const rows = ref<EditRow[]>([])
const loading = ref(false)
const saving = ref(false)

async function load() {
  if (!props.period)
    return
  loading.value = true
  try {
    const list = await getLedgerBalancesApi(props.period)
    rows.value = list.map(r => ({ ...r, editOpening: r.opening, editClosing: r.closing, editRemark: r.remark ?? '' }))
  }
  finally {
    loading.value = false
  }
}

watch(() => props.period, load, { immediate: true })
defineExpose({ load })

const visibleRows = computed(() => props.shop ? rows.value.filter(r => r.shop === props.shop) : rows.value)

const round2 = (v: number) => Math.round(v * 100) / 100
/** 编辑中的实时勾稽结果（未保存也能看到差异） */
const expectedOf = (r: EditRow) => r.editOpening == null ? null : round2(r.editOpening + r.net)
function diffOf(r: EditRow) {
  const e = expectedOf(r)
  return e == null || r.editClosing == null ? null : round2(r.editClosing - e)
}
const continuityOf = (r: EditRow) => r.editOpening == null || r.prevClosing == null ? null : round2(r.editOpening - r.prevClosing)

function statusOf(r: EditRow) {
  const d = diffOf(r)
  if (d == null)
    return { label: t('未录入'), cls: 'st-missing' }
  return Math.abs(d) < 0.005 ? { label: t('已平'), cls: 'st-ok' } : { label: t('不平'), cls: 'st-bad' }
}

const dirty = (r: EditRow) => r.editOpening !== r.opening || r.editClosing !== r.closing || r.editRemark !== (r.remark ?? '')
const dirtyRows = computed(() => rows.value.filter(dirty))

/** 期初为空时用上月期末带入 */
function fillOpeningFromPrev() {
  let n = 0
  for (const r of visibleRows.value) {
    if (r.editOpening == null && r.prevClosing != null && !props.frozenShops.has(r.shop)) {
      r.editOpening = r.prevClosing
      n++
    }
  }
  ElMessage[n ? 'success' : 'info'](n ? `已带入 ${n} 个账户的上月期末` : '没有可带入的上月期末（需先录入上月余额）')
}

async function save() {
  if (!props.operator.trim()) {
    ElMessage.warning(t('请先在右上角填写操作人'))
    return
  }
  saving.value = true
  try {
    await saveLedgerBalancesApi(props.operator, dirtyRows.value.map(r => ({
      shop: r.shop,
      entity: r.entity,
      account: r.account,
      period: r.period,
      opening: r.editOpening,
      closing: r.editClosing,
      remark: r.editRemark,
    })))
    ElMessage.success(t('余额已保存'))
    await load()
    emit('saved')
  }
  finally {
    saving.value = false
  }
}

const summary = computed(() => {
  const r = { balanced: 0, unbalanced: 0, missing: 0 }
  for (const row of visibleRows.value) {
    const s = statusOf(row).cls
    r[s === 'st-ok' ? 'balanced' : s === 'st-bad' ? 'unbalanced' : 'missing']++
  }
  return r
})
</script>

<template>
  <div v-loading="loading" class="h-full flex flex-col gap-3">
    <div class="flex items-center gap-3 text-[13px]">
      <span class="tag st-ok">{{ $t('已平 {0}', [summary.balanced]) }}</span>
      <span class="tag st-bad">{{ $t('不平 {0}', [summary.unbalanced]) }}</span>
      <span class="tag st-missing">{{ $t('未录入 {0}', [summary.missing]) }}</span>
      <span class="text-xs text-gray-400">
        {{ $t('从抖店后台「资金账户」页抄录各主体账户的月初、月末余额。期初 + 入账 − 出账 ≠ 期末，说明流水漏导或多导') }}
      </span>
      <div class="ml-auto flex gap-2">
        <el-button size="small" @click="fillOpeningFromPrev">
          {{ $t('期初带入上月期末') }}
        </el-button>
        <el-button size="small" type="primary" :disabled="!dirtyRows.length" :loading="saving" @click="save">
          {{ $t('保存{0}', [dirtyRows.length ? `（${dirtyRows.length}）` : '']) }}
        </el-button>
      </div>
    </div>

    <el-table
      :data="visibleRows"
      header-row-class-name="global-table-header"
      border
      class="balance-table"
      height="100%"
      :row-key="(r: EditRow) => `${r.shop}|${r.entity}|${r.account}`"
    >
      <el-table-column :label="$t('店铺')" prop="shop" min-width="140" show-overflow-tooltip />
      <el-table-column :label="$t('主体 · 账户')" min-width="150">
        <template #default="{ row }">
          <el-tooltip :content="row.entity" placement="top">
            <span>{{ shortEntityName(row.entity) }} · {{ row.account }}</span>
          </el-tooltip>
        </template>
      </el-table-column>
      <el-table-column :label="$t('笔数')" align="right" width="90">
        <template #default="{ row }">
          {{ row.rowCount.toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column :label="$t('期初余额')" align="right" width="150">
        <template #default="{ row }">
          <el-input-number
            v-model="row.editOpening"
            size="small"
            :precision="2"
            :controls="false"
            :disabled="frozenShops.has(row.shop)"
            class="!w-full"
            :placeholder="$t('录入')"
          />
        </template>
      </el-table-column>
      <el-table-column :label="$t('入账')" align="right" width="130">
        <template #default="{ row }">
          {{ formatMoney(row.inflow) }}
        </template>
      </el-table-column>
      <el-table-column :label="$t('出账')" align="right" width="130">
        <template #default="{ row }">
          {{ formatMoney(row.outflow) }}
        </template>
      </el-table-column>
      <el-table-column :label="$t('应有期末')" align="right" width="130">
        <template #default="{ row }">
          <span class="text-gray-500">{{ formatMoney(expectedOf(row)) }}</span>
        </template>
      </el-table-column>
      <el-table-column :label="$t('期末余额')" align="right" width="150">
        <template #default="{ row }">
          <el-input-number
            v-model="row.editClosing"
            size="small"
            :precision="2"
            :controls="false"
            :disabled="frozenShops.has(row.shop)"
            class="!w-full"
            :placeholder="$t('录入')"
          />
        </template>
      </el-table-column>
      <el-table-column :label="$t('差异')" align="right" width="120">
        <template #default="{ row }">
          <span :class="statusOf(row).cls === 'st-bad' ? 'font-semibold text-[#B91C1C]' : 'text-gray-400'">
            {{ diffOf(row) == null ? '' : formatMoney(diffOf(row)) }}
          </span>
        </template>
      </el-table-column>
      <el-table-column :label="$t('状态')" align="center" width="80">
        <template #default="{ row }">
          <span class="tag" :class="statusOf(row).cls">{{ statusOf(row).label }}</span>
        </template>
      </el-table-column>
      <el-table-column :label="$t('上月期末 / 衔接')" align="right" width="150">
        <template #default="{ row }">
          <template v-if="row.prevClosing != null">
            <div class="text-gray-500">
              {{ formatMoney(row.prevClosing) }}
            </div>
            <div v-if="continuityOf(row) && Math.abs(continuityOf(row)!) >= 0.005" class="text-xs text-[#92400E]">
              {{ $t('期初差 {0}', [formatMoney(continuityOf(row))]) }}
            </div>
          </template>
          <span v-else class="text-xs text-gray-300">{{ $t('上月未录入') }}</span>
        </template>
      </el-table-column>
      <el-table-column :label="$t('备注')" min-width="160">
        <template #default="{ row }">
          <el-input v-model="row.editRemark" size="small" :disabled="frozenShops.has(row.shop)" :placeholder="$t('差异说明')" />
        </template>
      </el-table-column>
      <el-table-column :label="$t('修改人')" width="150">
        <template #default="{ row }">
          <span v-if="frozenShops.has(row.shop)" class="text-xs text-[#0B5A2E]">{{ $t('🔒 已冻结') }}</span>
          <span v-else class="text-xs text-gray-400">{{ row.updatedBy ? `${row.updatedBy} ${row.updatedAt?.slice(5, 16)}` : '' }}</span>
        </template>
      </el-table-column>
    </el-table>
  </div>
</template>

<style scoped lang="scss">
.tag {
  @apply rounded px-2 py-0.5 text-xs font-medium;
}

.st-ok {
  @apply bg-[#E7F4EC] text-[#0B5A2E];
}

.st-bad {
  @apply bg-[#FEE2E2] text-[#B91C1C];
}

.st-missing {
  @apply bg-gray-100 text-gray-500;
}

// 隐藏 el-table border 属性的可见边框，但保留 resizable 能力
.balance-table {
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
