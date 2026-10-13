<script setup lang="ts">
import type { AnalysisBreakdown, AnalysisRow } from '@/packs/ecommerce/api'
import type { AnalysisSheetPayload } from '@/packs/ecommerce/shop-ledger/analysis-sheet'
import type { DrillTarget } from '@/packs/ecommerce/shop-ledger/ledger-workbook'
import { computed, ref, watch } from 'vue'
import { t as tr } from '@/i18n'
import { getAnalysisBreakdownApi } from '@/packs/ecommerce/api'
import { buildDrillTarget, dimensionLabel, formatPct, rateOf } from '@/packs/ecommerce/shop-ledger/analysis'
import { formatMoney } from '@/utils/format-money'
import AmountLink from './AmountLink.vue'

const props = defineProps<{
  period: string
  shop: string
}>()

const emit = defineEmits<{
  (e: 'drill', target: DrillTarget): void
  (e: 'export', payload: AnalysisSheetPayload): void
}>()

type Dim = 'channel' | 'product' | 'talent'
const DIMS: { by: Dim, label: string }[] = [
  { by: 'channel', label: tr('渠道') },
  { by: 'product', label: tr('商品') },
  { by: 'talent', label: tr('达人') },
]
const TOP = 10

const data = ref<Partial<Record<Dim, AnalysisBreakdown>>>({})
const loading = ref(false)

async function load() {
  if (!props.period)
    return
  loading.value = true
  try {
    const [channel, product, talent] = await Promise.all(DIMS.map(d => getAnalysisBreakdownApi(props.period, d.by, props.shop)))
    data.value = { channel, product, talent }
  }
  finally {
    loading.value = false
  }
}
watch(() => [props.period, props.shop], load, { immediate: true })

const total = computed(() => data.value.channel?.total)

/** 按退款金额排序（退款为负数，越小越多）；只看有退款的 */
function topRows(by: Dim): AnalysisRow[] {
  return [...(data.value[by]?.rows ?? [])].filter(r => r.values.refund < 0).sort((a, b) => a.values.refund - b.values.refund).slice(0, TOP)
}

/** 退款占全部退款的比例 */
function share(row: AnalysisRow) {
  const t = total.value?.values.refund
  return t ? row.values.refund / t : null
}

function drill(by: Dim, row: AnalysisRow) {
  const metric = data.value[by]!.metrics.find(m => m.key === 'refund')!
  const label = DIMS.find(d => d.by === by)!.label
  emit('drill', buildDrillTarget(metric, row.values.refund, { scope: `${label} ${row.name}`, shop: props.shop || undefined, by, key: row.key }))
}

function exportSheet() {
  const rows = DIMS.flatMap(d => topRows(d.by).map(r => [d.label, dimensionLabel(d.by, r), r.values.refund, r.refundCount, rateOf(r, 'refund'), share(r)]))
  emit('export', {
    name: `退款分析_${props.period}`,
    title: `退款分析 ${props.period}${props.shop ? ` · ${props.shop}` : ' · 全部店铺'}（各维度前 ${TOP}）`,
    columns: [
      { title: tr('维度'), width: 70 },
      { title: tr('名称'), width: 300 },
      { title: tr('退款'), kind: 'money' },
      { title: tr('售后单数'), kind: 'int' },
      { title: tr('退款率'), kind: 'percent' },
      { title: tr('占全部退款'), kind: 'percent' },
    ],
    rows,
  })
}
</script>

<template>
  <div v-loading="loading" class="flex flex-col gap-3">
    <div v-if="total" class="flex items-center gap-6 rounded bg-[#F7F8FA] px-4 py-3">
      <div>
        <div class="text-xs text-gray-500">
          {{ $t('退款合计') }}
        </div>
        <div class="text-xl text-gray-900 font-semibold tabular-nums">
          {{ formatMoney(total.values.refund) }}
        </div>
      </div>
      <div>
        <div class="text-xs text-gray-500">
          {{ $t('退款率（退款 / 结算实付）') }}
        </div>
        <div class="text-xl text-gray-900 font-semibold tabular-nums">
          {{ formatPct(rateOf(total, 'refund')) }}
        </div>
      </div>
      <div>
        <div class="text-xs text-gray-500">
          {{ $t('售后单数') }}
        </div>
        <div class="text-xl text-gray-900 font-semibold tabular-nums">
          {{ total.refundCount.toLocaleString() }}
        </div>
      </div>
      <span class="text-xs text-gray-400">{{ $t('退款含结算时扣减与结算后退给用户；按退款金额排序，各维度显示前 {0}', [TOP]) }}</span>
      <el-button size="small" class="ml-auto" @click="exportSheet">
        {{ $t('写入工作表') }}
      </el-button>
    </div>

    <div class="grid grid-cols-3 gap-3">
      <div v-for="d in DIMS" :key="d.by" class="rounded border border-[#e6e6e6] border-solid p-3">
        <div class="mb-2 text-gray-800 font-semibold">
          {{ $t('按{0}', [d.label]) }}
        </div>
        <div v-for="r in topRows(d.by)" :key="r.key" class="py-1.5 text-[13px] leading-5">
          <div class="flex gap-2">
            <span class="min-w-0 flex-1 truncate" :title="dimensionLabel(d.by, r)">{{ dimensionLabel(d.by, r) }}</span>
            <AmountLink :value="r.values.refund" @drill="drill(d.by, r)" />
          </div>
          <!-- 条长 = 占全部退款的比例 -->
          <div class="mt-1 h-1.5 rounded-full bg-[#f0efec]">
            <div class="h-full rounded-full bg-[#d4544f]" :style="{ width: `${Math.min(100, (share(r) ?? 0) * 100)}%` }" />
          </div>
          <div class="mt-0.5 text-xs text-gray-400">
            {{ $t('占 {0} · 退款率', [formatPct(share(r))]) }} <span :class="{ 'text-[#B91C1C] font-medium': (rateOf(r, 'refund') ?? 0) >= 2 * (rateOf(total!, 'refund') ?? 0) }">{{ formatPct(rateOf(r, 'refund')) }}</span>
            {{ $t('· {0} 笔售后', [r.refundCount]) }}
          </div>
        </div>
        <el-empty v-if="!topRows(d.by).length" :image-size="60" :description="$t('无退款')" />
      </div>
    </div>
  </div>
</template>
