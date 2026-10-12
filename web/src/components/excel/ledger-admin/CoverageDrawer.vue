<script setup lang="ts">
import type { LedgerCoverage, LedgerCoverageCell } from '@/api/ledger'
import { computed, ref, watch } from 'vue'
import { getLedgerCoverageApi } from '@/api/ledger'
import { t } from '@/i18n'

const props = defineProps<{
  visible: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 查看某月汇总 */
  (e: 'openPeriod', period: string): void
}>()

const coverage = ref<LedgerCoverage | null>(null)
const loading = ref(false)
const selected = ref<LedgerCoverageCell | null>(null)

watch(() => props.visible, async (visible) => {
  if (!visible)
    return
  loading.value = true
  selected.value = null
  try {
    coverage.value = await getLedgerCoverageApi()
    // 默认选中第一个有问题的格子
    selected.value = coverage.value.cells.find(c => c.status === 'partial' || c.status === 'missing') ?? null
  }
  finally {
    loading.value = false
  }
})

const STATUS = {
  complete: { icon: '✓', label: t('完整'), cls: 'st-complete' },
  partial: { icon: '!', label: t('有缺口'), cls: 'st-partial' },
  missing: { icon: '✕', label: t('未导入'), cls: 'st-missing' },
  not_expected: { icon: '—', label: t('无需数据'), cls: 'st-none' },
} as const

const cellMap = computed(() => new Map((coverage.value?.cells ?? []).map(c => [`${c.shop}\u0001${c.period}`, c])))
const cellOf = (shop: string, period: string) => cellMap.value.get(`${shop}\u0001${period}`)

const counts = computed(() => {
  const r = { complete: 0, partial: 0, missing: 0 }
  for (const c of coverage.value?.cells ?? []) {
    if (c.status in r)
      r[c.status as keyof typeof r]++
  }
  return r
})

const medianOf = (shop: string) => coverage.value?.shops.find(s => s.name === shop)?.medianDaily ?? 0

/** 连续日期压缩为区间：[16,17,18,20] → 16–18 日、20 日 */
function ranges(days: number[]): string {
  const out: string[] = []
  let start = days[0]
  for (let i = 1; i <= days.length; i++) {
    const d = days[i]
    const prev = days[i - 1]!
    if (d !== prev + 1) {
      out.push(start === prev ? t('{0} 日', [prev]) : t('{0}–{1} 日', [start, prev]))
      start = d
    }
  }
  return out.join('、')
}

function dayClass(cell: LedgerCoverageCell, day: number): string {
  if (cell.expectedFrom == null || day < cell.expectedFrom || day > (cell.expectedTo ?? 0))
    return cell.days[day - 1] ? 'bar-ok' : 'bar-none'
  if (cell.missingDays.includes(day))
    return 'bar-missing'
  if (cell.lowDays.includes(day))
    return 'bar-low'
  return 'bar-ok'
}

function barHeight(cell: LedgerCoverageCell, day: number, max: number): string {
  const n = cell.days[day - 1] ?? 0
  if (!n)
    return '100%'
  return `${Math.max(6, Math.round((n / max) * 100))}%`
}

const selectedMax = computed(() => Math.max(1, ...(selected.value?.days ?? [1])))
</script>

<template>
  <el-drawer
    :model-value="visible"
    :title="$t('月结完整性')"
    size="78%"
    @update:model-value="emit('update:visible', $event)"
  >
    <div v-loading="loading" class="h-full flex flex-col gap-4">
      <div class="flex items-center gap-4 text-[13px]">
        <span class="legend st-complete">{{ $t('✓ 完整 {0}', [counts.complete]) }}</span>
        <span class="legend st-partial">{{ $t('! 有缺口 {0}', [counts.partial]) }}</span>
        <span class="legend st-missing">{{ $t('✕ 未导入 {0}', [counts.missing]) }}</span>
        <span class="text-xs text-gray-400">
          {{ $t('按天检查：应有流水的日期（店铺首笔流水之后、昨天及以前、关店之前）无流水为缺口；某天笔数低于日中位数 30% 视为疑似导出截断') }}
        </span>
      </div>

      <!-- 店铺 × 月份 -->
      <el-table v-if="coverage" :data="coverage.shops" row-key="name" max-height="46vh">
        <el-table-column :label="$t('店铺')" width="200" fixed>
          <template #default="{ row }">
            <div class="font-medium text-gray-800">
              {{ row.name }}
              <el-tag v-if="row.status === 'closed'" size="small" type="info" class="ml-1">
                {{ $t('已关店') }}
              </el-tag>
            </div>
            <div class="text-xs text-gray-400">
              {{ $t('日均约 {0} 笔', [Math.round(row.medianDaily).toLocaleString()]) }}
            </div>
          </template>
        </el-table-column>
        <el-table-column v-for="period in coverage.periods" :key="period" :label="period" min-width="150" align="center">
          <template #default="{ row }">
            <button
              v-if="cellOf(row.name, period)"
              class="cell"
              :class="[STATUS[cellOf(row.name, period)!.status].cls, { active: selected === cellOf(row.name, period) }]"
              @click="selected = cellOf(row.name, period)!"
            >
              <span class="font-semibold">{{ STATUS[cellOf(row.name, period)!.status].icon }}</span>
              <span>{{ cellOf(row.name, period)!.rowCount.toLocaleString() }}</span>
              <span v-if="cellOf(row.name, period)!.missingDays.length" class="text-[11px]">{{ $t('缺 {0} 天', [cellOf(row.name, period)!.missingDays.length]) }}</span>
            </button>
          </template>
        </el-table-column>
      </el-table>

      <!-- 选中格子的每日明细 -->
      <div v-if="selected" class="rounded border border-solid border-[#e6e6e6] p-3">
        <div class="mb-2 flex items-center gap-3">
          <span class="font-semibold text-gray-800">{{ selected.shop }} · {{ selected.period }}</span>
          <span class="legend" :class="STATUS[selected.status].cls">{{ STATUS[selected.status].label }}</span>
          <span class="text-xs text-gray-500">{{ $t('{0} 条流水 · 日中位数 {1}', [selected.rowCount.toLocaleString(), Math.round(medianOf(selected.shop)).toLocaleString()]) }}</span>
          <el-button size="small" class="ml-auto" @click="emit('openPeriod', selected.period); emit('update:visible', false)">
            {{ $t('查看 {0} 汇总', [selected.period]) }}
          </el-button>
        </div>
        <div class="flex h-28 items-end gap-[3px]">
          <div
            v-for="(n, i) in selected.days"
            :key="i"
            class="h-full flex flex-1 flex-col items-center"
            :title="$t('{0} 日：{1} 笔', [i + 1, n.toLocaleString()])"
          >
            <!-- 柱子区域需要确定高度，百分比高度才能生效 -->
            <div class="w-full flex flex-1 items-end">
              <div class="bar" :class="dayClass(selected, i + 1)" :style="{ height: barHeight(selected, i + 1, selectedMax) }" />
            </div>
            <span class="mt-0.5 text-[10px] text-gray-400">{{ i + 1 }}</span>
          </div>
        </div>
        <div class="mt-2 text-[13px] leading-6">
          <div v-if="selected.missingDays.length" class="text-[#B91C1C]">
            {{ $t('缺少流水：{0}，请补导 {1} 这些日期的资金流水（合并导入会自动去重）', [ranges(selected.missingDays), selected.period]) }}
          </div>
          <div v-if="selected.lowDays.length" class="text-[#92400E]">
            {{ $t('笔数异常偏低：{0}，可能是导出被截断，建议重新导出这些日期核对', [ranges(selected.lowDays)]) }}
          </div>
          <div v-if="selected.status === 'complete'" class="text-[#0B5A2E]">
            {{ $t('该月流水完整') }}
          </div>
          <div v-if="selected.status === 'not_expected'" class="text-gray-500">
            {{ $t('该月在店铺开店前或关店后，无需数据') }}
          </div>
        </div>
      </div>
    </div>
  </el-drawer>
</template>

<style scoped lang="scss">
.legend {
  @apply rounded px-2 py-0.5 text-xs font-medium;
}

.cell {
  @apply w-full flex flex-col items-center gap-0.5 rounded border border-solid border-transparent py-1.5 text-xs leading-4;

  &.active {
    @apply border-[#107C41];
  }
}

.st-complete {
  @apply bg-[#E7F4EC] text-[#0B5A2E];
}

.st-partial {
  @apply bg-[#FEF3C7] text-[#92400E];
}

.st-missing {
  @apply bg-[#FEE2E2] text-[#B91C1C];
}

.st-none {
  @apply bg-gray-50 text-gray-400;
}

.bar {
  @apply w-full rounded-t-sm;
}

.bar-ok {
  @apply bg-[#6CC08B];
}

.bar-low {
  @apply bg-[#F59E0B];
}

.bar-missing {
  @apply bg-[#FECACA];
}

.bar-none {
  @apply bg-gray-100;
}
</style>
