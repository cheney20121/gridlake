<script setup lang="ts">
import type { AnalysisBreakdown, AnalysisMetricDef, AnalysisMetricKey, AnalysisRow } from '@/api/ledger'
import type { AnalysisSheetPayload } from '@/features/excel/shop-ledger/analysis-sheet'
import type { DrillTarget } from '@/features/excel/shop-ledger/ledger-workbook'
import { computed, ref, watch } from 'vue'
import { getAnalysisBreakdownApi } from '@/api/ledger'
import { formatMoney } from '@/components/excel/ledger-detail/columns'
import { buildDrillTarget, formatPct, rateOf, WATERFALL_KEYS } from '@/features/excel/shop-ledger/analysis'
import AmountLink from './AmountLink.vue'

const props = defineProps<{
  period: string
  shop: string
}>()

const emit = defineEmits<{
  (e: 'drill', target: DrillTarget): void
  (e: 'export', payload: AnalysisSheetPayload): void
}>()

const data = ref<AnalysisBreakdown | null>(null)
const loading = ref(false)
/** 瀑布图展示的行：null 为合计 */
const selectedKey = ref<string | null>(null)

async function load() {
  if (!props.period)
    return
  loading.value = true
  try {
    data.value = await getAnalysisBreakdownApi(props.period, 'channel', props.shop)
    selectedKey.value = null
  }
  finally {
    loading.value = false
  }
}
watch(() => [props.period, props.shop], load, { immediate: true })

const metricOf = (key: AnalysisMetricKey) => data.value?.metrics.find(m => m.key === key) as AnalysisMetricDef
const selectedRow = computed(() => data.value
  ? (selectedKey.value == null ? data.value.total : data.value.rows.find(r => r.key === selectedKey.value) ?? data.value.total)
  : null)
const selectedName = computed(() => selectedKey.value == null ? '全部渠道' : selectedRow.value?.name ?? '')

function drill(key: AnalysisMetricKey, row: AnalysisRow, isTotal = false) {
  emit('drill', buildDrillTarget(metricOf(key), row.values[key], isTotal
    ? { scope: '全部渠道', shop: props.shop || undefined }
    : { scope: row.name, shop: props.shop || undefined, by: 'channel', key: row.key }))
}

// ── 瀑布图 ──
interface Step {
  key: AnalysisMetricKey | 'net'
  label: string
  value: number
  from: number
  to: number
  total: boolean
}

const steps = computed<Step[]>(() => {
  const row = selectedRow.value
  if (!row || !data.value)
    return []
  let acc = 0
  const out: Step[] = WATERFALL_KEYS.map((key) => {
    const value = row.values[key]
    const step = { key, label: metricOf(key).label, value, from: acc, to: acc + value, total: false }
    acc += value
    return step
  })
  out.push({ key: 'net', label: '资金净额', value: row.values.net, from: 0, to: row.values.net, total: true })
  return out
})

const domain = computed(() => {
  const all = steps.value.flatMap(s => [s.from, s.to, 0])
  const min = Math.min(...all)
  const max = Math.max(...all)
  return { min, span: Math.max(max - min, 1) }
})

/** 柱子在绘图区中的位置（百分比，从下往上） */
function barStyle(s: Step) {
  const { min, span } = domain.value
  const lo = Math.min(s.from, s.to)
  const hi = Math.max(s.from, s.to)
  return { bottom: `${((lo - min) / span) * 100}%`, height: `${Math.max(((hi - lo) / span) * 100, 0.6)}%` }
}

const zeroLine = computed(() => `${((0 - domain.value.min) / domain.value.span) * 100}%`)

function short(v: number) {
  if (Math.abs(v) >= 1e4)
    return `${(v / 1e4).toFixed(Math.abs(v) >= 1e6 ? 0 : 1)}万`
  return v.toFixed(0)
}

// ── 指标卡 ──
const tiles = computed(() => {
  const t = data.value?.total
  if (!t)
    return []
  return [
    { label: '结算实付', value: formatMoney(t.values.gmv), sub: `${t.orders.toLocaleString()} 单` },
    { label: '平台服务费率', value: formatPct(rateOf(t, 'platformFee')), sub: formatMoney(t.values.platformFee) },
    { label: '佣金率', value: formatPct(rateOf(t, 'commission')), sub: formatMoney(t.values.commission) },
    { label: '退款率', value: formatPct(rateOf(t, 'refund')), sub: `${t.refundCount.toLocaleString()} 笔售后` },
  ]
})

const TABLE_KEYS: AnalysisMetricKey[] = ['gmv', 'freight', 'subsidy', 'refund', 'platformFee', 'commission', 'otherFee', 'other', 'net']

function exportSheet() {
  if (!data.value)
    return
  const rows = [...data.value.rows, data.value.total]
  emit('export', {
    name: `渠道分析_${props.period}`,
    title: `渠道资金瀑布 ${props.period}${props.shop ? ` · ${props.shop}` : ' · 全部店铺'}`,
    columns: [
      { title: '渠道' },
      ...TABLE_KEYS.map(k => ({ title: metricOf(k).label, kind: 'money' as const })),
      { title: '结算订单数', kind: 'int' },
      { title: '平台服务费率', kind: 'percent' },
      { title: '佣金率', kind: 'percent' },
      { title: '退款率', kind: 'percent' },
    ],
    rows: rows.map(r => [
      r.name,
      ...TABLE_KEYS.map(k => r.values[k]),
      r.orders,
      rateOf(r, 'platformFee'),
      rateOf(r, 'commission'),
      rateOf(r, 'refund'),
    ]),
  })
}
</script>

<template>
  <div v-loading="loading" class="flex flex-col gap-4">
    <div class="grid grid-cols-4 gap-3">
      <div v-for="t in tiles" :key="t.label" class="tile">
        <div class="text-xs text-gray-500">
          {{ t.label }}
        </div>
        <div class="text-xl text-gray-900 font-semibold tabular-nums">
          {{ t.value }}
        </div>
        <div class="text-xs text-gray-400 tabular-nums">
          {{ t.sub }}
        </div>
      </div>
    </div>

    <div v-if="data" class="rounded border border-[#e6e6e6] border-solid p-3">
      <div class="mb-2 flex items-center gap-3">
        <span class="text-gray-800 font-semibold">资金瀑布 · {{ selectedName }}</span>
        <span class="legend"><i class="dot bg-[#3b7dd8]" />增加</span>
        <span class="legend"><i class="dot bg-[#d4544f]" />扣减</span>
        <span class="legend"><i class="dot bg-[#5f5e5a]" />资金净额</span>
        <span class="text-xs text-gray-400">点击下表的渠道行切换；点击柱子查看构成明细</span>
      </div>
      <div class="relative h-56 flex gap-2 border-b border-[#d9d9d6] border-b-solid">
        <div class="pointer-events-none absolute left-0 right-0 border-t border-[#b4b2a9] border-t-dashed" :style="{ bottom: zeroLine }" />
        <div v-for="s in steps" :key="s.key" class="relative flex-1">
          <el-tooltip placement="top" :content="`${s.label}：${formatMoney(s.value)}${selectedRow && s.key !== 'gmv' && s.key !== 'net' && selectedRow.values.gmv ? `（占结算实付 ${formatPct(s.value / selectedRow.values.gmv)}）` : ''}`">
            <button
              class="bar"
              :class="s.total ? 'bg-[#5f5e5a]' : s.value >= 0 ? 'bg-[#3b7dd8]' : 'bg-[#d4544f]'"
              :style="barStyle(s)"
              @click="selectedRow && drill(s.key, selectedRow, selectedKey == null)"
            >
              <span class="bar-label">{{ s.value ? short(s.value) : '' }}</span>
            </button>
          </el-tooltip>
        </div>
      </div>
      <div class="mt-1 flex gap-2">
        <div v-for="s in steps" :key="s.key" class="flex-1 text-center text-xs text-gray-500">
          {{ s.label }}
        </div>
      </div>
    </div>

    <div class="flex items-center justify-between">
      <span class="text-xs text-gray-400">
        各项之和 = 资金净额；「其他收支」含运费险、小额打款、提现、千川充值等非订单收支。点击金额可下钻明细，明细贡献合计与此一致
      </span>
      <el-button size="small" @click="exportSheet">
        写入工作表
      </el-button>
    </div>
    <el-table
      v-if="data"
      :data="[...data.rows, data.total]"
      size="small"
      :row-class-name="({ row }: { row: AnalysisRow }) => (row === data!.total ? 'total-row' : (selectedKey === row.key ? 'selected-row' : ''))"
      @row-click="(row: AnalysisRow) => selectedKey = row === data!.total ? null : row.key"
    >
      <el-table-column label="渠道" prop="name" width="110" fixed />
      <el-table-column v-for="k in TABLE_KEYS" :key="k" :label="metricOf(k).label" align="right" min-width="118">
        <template #header>
          <el-tooltip :content="metricOf(k).hint" placement="top">
            <span>{{ metricOf(k).label }}</span>
          </el-tooltip>
        </template>
        <template #default="{ row }">
          <AmountLink :value="row.values[k]" @drill="drill(k, row, row === data.total)" />
        </template>
      </el-table-column>
      <el-table-column label="订单数" align="right" width="90">
        <template #default="{ row }">
          {{ row.orders.toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column label="平台费率" align="right" width="84">
        <template #default="{ row }">
          {{ formatPct(rateOf(row, 'platformFee')) }}
        </template>
      </el-table-column>
      <el-table-column label="佣金率" align="right" width="80">
        <template #default="{ row }">
          {{ formatPct(rateOf(row, 'commission')) }}
        </template>
      </el-table-column>
      <el-table-column label="退款率" align="right" width="80">
        <template #default="{ row }">
          {{ formatPct(rateOf(row, 'refund')) }}
        </template>
      </el-table-column>
    </el-table>
  </div>
</template>

<style scoped lang="scss">
.tile {
  @apply rounded bg-[#F7F8FA] px-4 py-3;
}

.legend {
  @apply inline-flex items-center gap-1 text-xs text-gray-600;
}

.dot {
  @apply inline-block h-2.5 w-2.5 rounded-sm;
}

.bar {
  @apply absolute left-[18%] right-[18%] rounded-[4px] cursor-pointer;
}

.bar-label {
  @apply absolute left-1/2 -top-4 -translate-x-1/2 whitespace-nowrap text-[11px] text-gray-700 tabular-nums;
}

:deep(.total-row) {
  @apply font-semibold bg-[#F7F8FA];
}

:deep(.selected-row) {
  @apply bg-[#E7F4EC];
}

:deep(.#{$namespace}-table__row) {
  cursor: pointer;
}
</style>
