<script setup lang="ts">
import type { AnalysisBreakdown, AnalysisMetricDef, AnalysisMetricKey, AnalysisRow } from '@/api/ledger'
import type { AnalysisSheetPayload } from '@/features/excel/shop-ledger/analysis-sheet'
import type { DrillTarget } from '@/features/excel/shop-ledger/ledger-workbook'
import { computed, ref, watch } from 'vue'
import { getAnalysisBreakdownApi } from '@/api/ledger'
import { buildDrillTarget, dimensionLabel, formatPct, rateOf } from '@/features/excel/shop-ledger/analysis'
import { t } from '@/i18n'
import AmountLink from './AmountLink.vue'

const props = defineProps<{
  period: string
  shop: string
  by: 'talent' | 'product'
}>()

const emit = defineEmits<{
  (e: 'drill', target: DrillTarget): void
  (e: 'export', payload: AnalysisSheetPayload): void
}>()

const DIM_LABEL = { talent: '达人', product: '商品' } as const
const PAGE_SIZE = 50

const data = ref<AnalysisBreakdown | null>(null)
const loading = ref(false)
const keyword = ref('')
const flagFilter = ref<'' | Flag>('')
const sort = ref<{ key: SortKey, desc: boolean }>({ key: 'gmv', desc: true })
const page = ref(1)

async function load() {
  if (!props.period)
    return
  loading.value = true
  try {
    data.value = await getAnalysisBreakdownApi(props.period, props.by, props.shop)
    page.value = 1
  }
  finally {
    loading.value = false
  }
}
watch(() => [props.period, props.shop, props.by], load, { immediate: true })

const metricOf = (key: AnalysisMetricKey) => data.value?.metrics.find(m => m.key === key) as AnalysisMetricDef

// ── 异常标记 ──
type Flag = 'highCommission' | 'highRefund' | 'negative'
const FLAG_META: Record<Flag, { label: string, cls: string, hint: string }> = {
  highCommission: { label: t('高佣'), cls: 'flag-warn', hint: t('佣金率 ≥ 35%，且高于整体佣金率 1.5 倍') },
  highRefund: { label: t('高退款'), cls: 'flag-bad', hint: t('退款率 ≥ 整体退款率 2 倍（结算实付 ≥ 1,000）') },
  negative: { label: t('负贡献'), cls: 'flag-bad', hint: t('资金净额为负：扣除佣金、费用、退款后倒贴') },
}

const baseline = computed(() => data.value
  ? { commission: rateOf(data.value.total, 'commission') ?? 0, refund: rateOf(data.value.total, 'refund') ?? 0 }
  : { commission: 0, refund: 0 })

function flagsOf(row: AnalysisRow): Flag[] {
  if (!row.key)
    return []
  const out: Flag[] = []
  const cr = rateOf(row, 'commission')
  const rr = rateOf(row, 'refund')
  if (cr != null && cr >= 0.35 && cr >= baseline.value.commission * 1.5)
    out.push('highCommission')
  if (rr != null && row.values.gmv >= 1000 && rr >= baseline.value.refund * 2)
    out.push('highRefund')
  if (row.values.gmv > 0 && row.values.net < 0)
    out.push('negative')
  return out
}

const flagCounts = computed(() => {
  const c: Record<Flag, number> = { highCommission: 0, highRefund: 0, negative: 0 }
  for (const r of data.value?.rows ?? []) {
    for (const f of flagsOf(r))
      c[f]++
  }
  return c
})

// ── 排序 / 筛选 / 分页 ──
type SortKey = AnalysisMetricKey | 'orders' | 'commissionRate' | 'refundRate' | 'netRate'
function sortValue(row: AnalysisRow, key: SortKey): number {
  switch (key) {
    case 'orders': return row.orders
    case 'commissionRate': return rateOf(row, 'commission') ?? -Infinity
    case 'refundRate': return rateOf(row, 'refund') ?? -Infinity
    case 'netRate': return rateOf(row, 'net') ?? -Infinity
    default: return row.values[key]
  }
}

const filtered = computed(() => {
  const kw = keyword.value.trim().toLowerCase()
  const rows = (data.value?.rows ?? []).filter(r =>
    (!kw || r.name.toLowerCase().includes(kw) || r.key.includes(kw))
    && (!flagFilter.value || flagsOf(r).includes(flagFilter.value)))
  const { key, desc } = sort.value
  return [...rows].sort((a, b) => (sortValue(a, key) - sortValue(b, key)) * (desc ? -1 : 1))
})
const paged = computed(() => filtered.value.slice((page.value - 1) * PAGE_SIZE, page.value * PAGE_SIZE))
watch([keyword, flagFilter, sort], () => {
  page.value = 1
}, { deep: true })

function onSortChange({ prop, order }: { prop: string | null, order: 'ascending' | 'descending' | null }) {
  sort.value = prop && order ? { key: prop as SortKey, desc: order === 'descending' } : { key: 'gmv', desc: true }
}

function drill(key: AnalysisMetricKey, row: AnalysisRow) {
  emit('drill', buildDrillTarget(metricOf(key), row.values[key], {
    scope: `${DIM_LABEL[props.by]} ${row.name}`,
    shop: props.shop || undefined,
    by: props.by,
    key: row.key,
  }))
}

const MONEY_COLS: { key: AnalysisMetricKey, width: number }[] = [
  { key: 'gmv', width: 120 },
  { key: 'commission', width: 116 },
  { key: 'refund', width: 108 },
  { key: 'platformFee', width: 108 },
  { key: 'net', width: 120 },
]

function exportSheet() {
  if (!data.value)
    return
  emit('export', {
    name: `${DIM_LABEL[props.by]}排行_${props.period}`,
    title: `${DIM_LABEL[props.by]}排行 ${props.period}${props.shop ? ` · ${props.shop}` : ' · 全部店铺'}${flagFilter.value ? ` · ${FLAG_META[flagFilter.value].label}` : ''}`,
    columns: [
      { title: DIM_LABEL[props.by], width: 260 },
      { title: 'ID', width: 170 },
      { title: t('结算订单数'), kind: 'int' },
      ...MONEY_COLS.map(c => ({ title: metricOf(c.key).label, kind: 'money' as const })),
      { title: t('佣金率'), kind: 'percent' },
      { title: t('退款率'), kind: 'percent' },
      { title: t('净回款率'), kind: 'percent' },
      { title: t('标记'), width: 140 },
    ],
    rows: filtered.value.map(r => [
      r.name,
      r.key,
      r.orders,
      ...MONEY_COLS.map(c => r.values[c.key]),
      rateOf(r, 'commission'),
      rateOf(r, 'refund'),
      rateOf(r, 'net'),
      flagsOf(r).map(f => FLAG_META[f].label).join('、'),
    ]),
  })
}
</script>

<template>
  <div v-loading="loading" class="flex flex-col gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <el-input v-model="keyword" size="small" clearable :placeholder="$t('搜索{0}名称 / ID', [DIM_LABEL[by]])" class="!w-56" />
      <el-radio-group v-model="flagFilter" size="small">
        <el-radio-button value="">
          {{ $t('全部 {0}', [data?.rows.length ?? 0]) }}
        </el-radio-button>
        <el-radio-button v-for="(m, f) in FLAG_META" :key="f" :value="f">
          <el-tooltip :content="m.hint" placement="top">
            <span>{{ m.label }} {{ flagCounts[f] }}</span>
          </el-tooltip>
        </el-radio-button>
      </el-radio-group>
      <span v-if="data" class="text-xs text-gray-400">
        {{ $t('整体佣金率 {0} · 整体退款率 {1}', [formatPct(baseline.commission), formatPct(baseline.refund)]) }}
      </span>
      <el-button size="small" class="ml-auto" @click="exportSheet">
        {{ $t('写入工作表') }}
      </el-button>
    </div>

    <el-table :data="paged" size="small" :default-sort="{ prop: 'gmv', order: 'descending' }" @sort-change="onSortChange">
      <el-table-column type="index" :index="(i: number) => (page - 1) * PAGE_SIZE + i + 1" width="56" />
      <el-table-column :label="DIM_LABEL[by]" min-width="220" show-overflow-tooltip>
        <template #default="{ row }">
          {{ dimensionLabel(by, row) }}
          <el-tooltip v-for="f in flagsOf(row)" :key="f" :content="FLAG_META[f].hint" placement="top">
            <span class="flag" :class="FLAG_META[f].cls">{{ FLAG_META[f].label }}</span>
          </el-tooltip>
        </template>
      </el-table-column>
      <el-table-column :label="$t('订单数')" prop="orders" sortable="custom" align="right" width="90">
        <template #default="{ row }">
          {{ row.orders.toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column v-for="c in MONEY_COLS" :key="c.key" :prop="c.key" :label="metricOf(c.key)?.label" sortable="custom" align="right" :width="c.width">
        <template #default="{ row }">
          <AmountLink :value="row.values[c.key]" @drill="drill(c.key, row)" />
        </template>
      </el-table-column>
      <el-table-column :label="$t('佣金率')" prop="commissionRate" sortable="custom" align="right" width="90">
        <template #default="{ row }">
          {{ formatPct(rateOf(row, 'commission')) }}
        </template>
      </el-table-column>
      <el-table-column :label="$t('退款率')" prop="refundRate" sortable="custom" align="right" width="90">
        <template #default="{ row }">
          {{ formatPct(rateOf(row, 'refund')) }}
        </template>
      </el-table-column>
      <el-table-column :label="$t('净回款率')" prop="netRate" sortable="custom" align="right" width="100">
        <template #default="{ row }">
          {{ formatPct(rateOf(row, 'net')) }}
        </template>
      </el-table-column>
    </el-table>
    <el-pagination
      v-if="filtered.length > PAGE_SIZE"
      v-model:current-page="page"
      :page-size="PAGE_SIZE"
      :total="filtered.length"
      layout="total, prev, pager, next"
      size="small"
      class="justify-end"
    />
  </div>
</template>

<style scoped lang="scss">
.flag {
  @apply ml-1 rounded px-1.5 py-px text-[11px] font-medium;
}

.flag-warn {
  @apply bg-[#FEF3C7] text-[#92400E];
}

.flag-bad {
  @apply bg-[#FEE2E2] text-[#B91C1C];
}
</style>
