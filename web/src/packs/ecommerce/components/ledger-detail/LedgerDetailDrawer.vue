<script setup lang="ts">
import type { Column, SortBy } from 'element-plus'
import type { VNode } from 'vue'
import type { DetailColumnDef, FacetKey } from './columns'
import type { DetailRow } from './useDetailWindow'
import type { LedgerDetailFilter, LedgerDetailSort, LedgerDetailSummary } from '@/packs/ecommerce/api'
import type { DrillTarget } from '@/packs/ecommerce/shop-ledger/ledger-workbook'
import { useDebounce, useLocalStorage } from '@vueuse/core'
import { isCancel } from 'axios'
import { TableV2SortOrder } from 'element-plus'
import { computed, h, nextTick, reactive, ref, watch } from 'vue'
import { t as tr } from '@/i18n'
import { exportLedgerDetailsApi, getLedgerDetailSummaryApi } from '@/packs/ecommerce/api'
import { formatMoney } from '@/utils/format-money'
import { CONTRIBUTION_COLUMN, CONTRIBUTION_KEY, DETAIL_COLUMNS, FACETS, SEARCH_KEYS } from './columns'
import FacetPanel from './FacetPanel.vue'
import RowInspector from './RowInspector.vue'
import { useDetailWindow } from './useDetailWindow'

const props = defineProps<{
  visible: boolean
  period: string
  /** 下钻上下文；null 表示查看全部明细 */
  target: DrillTarget | null
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
}>()

// ── 筛选状态 ──────────────────────────────────────────
const selected = reactive<Record<FacetKey, string[]>>({ scenes: [], directions: [], billingTypes: [], entities: [], shops: [] })
const keyword = ref('')
const debouncedKeyword = useDebounce(keyword, 300)
const timeRange = ref<[string, string] | null>(null)
const amountMin = ref<number>()
const amountMax = ref<number>()
const useRule = ref(true)
const columnMode = useLocalStorage<'common' | 'all'>('ledger-detail-columns', 'common')
const sortBy = ref<SortBy>()

const summary = ref<LedgerDetailSummary | null>(null)
const loading = ref(false)
const exporting = ref(false)
const selectedRow = ref<DetailRow | null>(null)
const win = useDetailWindow()

const ruleActive = computed(() => !!props.target?.rule && useRule.value)

// 关闭规则后「贡献金额」列不存在，按它排序会被后端拒绝
watch(ruleActive, (active) => {
  if (!active && sortBy.value?.key === CONTRIBUTION_KEY)
    sortBy.value = undefined
})

const requestFilter = computed<LedgerDetailFilter>(() => {
  const f: LedgerDetailFilter = { period: props.period }
  if (props.target?.filter.shop)
    f.shop = props.target.filter.shop
  if (props.target?.filter.entity)
    f.entity = props.target.filter.entity
  // 经营分析下钻：达人 / 商品 / 下单月份为固定上下文（"" 表示该字段为空，也要传）
  for (const key of ['talentId', 'productId', 'orderMonth'] as const) {
    const v = props.target?.filter[key]
    if (v != null)
      f[key] = v
  }
  for (const { key } of FACETS) {
    if (selected[key].length)
      f[key] = [...selected[key]]
  }
  if (debouncedKeyword.value.trim())
    f.keyword = debouncedKeyword.value.trim()
  if (timeRange.value) {
    f.timeFrom = timeRange.value[0]
    f.timeTo = timeRange.value[1]
  }
  if (amountMin.value != null)
    f.amountMin = amountMin.value
  if (amountMax.value != null)
    f.amountMax = amountMax.value
  if (ruleActive.value)
    f.rule = props.target!.rule
  return f
})

const requestSort = computed<LedgerDetailSort | undefined>(() =>
  sortBy.value ? { column: String(sortBy.value.key), desc: sortBy.value.order === TableV2SortOrder.DESC } : undefined)

/** 下钻上下文固定了主体/店铺时，分面里不再显示这两个维度 */
const hiddenFacets = computed<FacetKey[]>(() => [
  ...(props.target?.filter.entity ? ['entities' as const] : []),
  ...(props.target?.filter.shop ? ['shops' as const] : []),
])

// ── 加载 ──────────────────────────────────────────────
let summaryController: AbortController | null = null

async function refresh() {
  if (!props.visible || !props.period)
    return
  summaryController?.abort()
  const controller = new AbortController()
  summaryController = controller
  loading.value = true
  selectedRow.value = null
  const filter = requestFilter.value
  try {
    const result = await getLedgerDetailSummaryApi(filter, controller.signal)
    if (controller !== summaryController)
      return
    summary.value = result
    win.reset(filter, requestSort.value, result.totalCount)
    win.ensureRange(0, 60)
  }
  catch (e) {
    if (!isCancel(e))
      console.warn('[ledger] 明细汇总加载失败:', e)
  }
  finally {
    if (controller === summaryController)
      loading.value = false
  }
}

/** 打开时按下钻上下文初始化筛选 */
function initFromTarget() {
  for (const { key } of FACETS)
    selected[key] = []
  selected.scenes = [...(props.target?.filter.scenes ?? [])]
  selected.directions = [...(props.target?.filter.directions ?? [])]
  selected.billingTypes = [...(props.target?.filter.billingTypes ?? [])]
  keyword.value = ''
  timeRange.value = null
  amountMin.value = undefined
  amountMax.value = undefined
  useRule.value = true
  sortBy.value = undefined
  summary.value = null
}

/** 打开时初始化筛选会改动 requestFilter，此时跳过监听触发的刷新，只在初始化完成后刷新一次 */
let opening = false

watch(() => props.visible, async (visible) => {
  if (!visible)
    return
  opening = true
  initFromTarget()
  await nextTick()
  opening = false
  refresh()
})
watch(requestFilter, () => {
  if (!opening)
    refresh()
}, { deep: true })
// 只改排序：行数与汇总不变，只需重置窗口
watch(requestSort, () => {
  if (!summary.value)
    return
  selectedRow.value = null
  win.reset(requestFilter.value, requestSort.value, summary.value.totalCount)
  win.ensureRange(0, 60)
})

function toggleFacet(key: FacetKey, value: string) {
  const list = selected[key]
  const i = list.indexOf(value)
  if (i >= 0)
    list.splice(i, 1)
  else
    list.push(value)
}

// ── 核对 ──────────────────────────────────────────────
/** 是否在下钻上下文之外叠加了筛选（此时与汇总值对比仅供参考） */
const hasExtraFilters = computed(() => {
  const t = props.target?.filter
  const same = (a: string[], b: string[] = []) => a.length === b.length && a.every(v => b.includes(v))
  return !!debouncedKeyword.value.trim() || !!timeRange.value || amountMin.value != null || amountMax.value != null
    || !same(selected.billingTypes, t?.billingTypes) || selected.entities.length > 0 || selected.shops.length > 0
    || !same(selected.scenes, t?.scenes) || !same(selected.directions, t?.directions)
})

const check = computed(() => {
  const expected = props.target?.expected
  const s = summary.value
  if (!expected || !s || (expected.kind === 'contribution' && !ruleActive.value))
    return null
  const actual = expected.kind === 'contribution' ? (s.contribution ?? 0) : s.totalCount
  const format = (v: number) => expected.kind === 'count' ? `${v.toLocaleString()} 笔` : formatMoney(v)
  return {
    label: expected.label,
    expected: format(expected.value),
    actual: format(actual),
    actualLabel: expected.kind === 'count' ? '明细笔数' : '明细贡献合计',
    ok: Math.abs(actual - expected.value) < 0.005,
  }
})

const nonZeroSums = computed(() => Object.entries(summary.value?.sums ?? {}).filter(([, v]) => v !== 0))

// ── 表格 ──────────────────────────────────────────────
const visibleDefs = computed<DetailColumnDef[]>(() => [
  ...(ruleActive.value ? [CONTRIBUTION_COLUMN] : []),
  ...DETAIL_COLUMNS.filter(c => columnMode.value === 'all' || c.common),
])

function escapeRegExp(s: string) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/** 关键字命中处高亮（不区分大小写） */
function highlight(text: string): (string | VNode)[] {
  const kw = debouncedKeyword.value.trim()
  if (!kw)
    return [text]
  const parts = text.split(new RegExp(`(${escapeRegExp(kw)})`, 'i'))
  return parts.map((p, i) => (i % 2 === 1 ? h('mark', { class: 'kw-hit' }, p) : p))
}

function renderCell(def: DetailColumnDef, row: DetailRow) {
  if (!row.__loaded)
    return h('span', { class: 'cell-skeleton' })
  const v = row[def.key]
  if (def.kind === 'money') {
    if (v == null)
      return ''
    if (v === 0)
      return h('span', { class: 'text-gray-300' }, '-')
    return h('span', {
      class: ['tabular-nums', Number(v) < 0 && 'text-[#DC2626]', def.key === CONTRIBUTION_KEY && 'font-semibold'],
    }, formatMoney(v))
  }
  if (def.kind === 'direction')
    return h('span', { class: v === '入账' ? 'dir-in' : 'dir-out' }, String(v ?? ''))
  const text = v == null ? '' : String(v)
  return h('span', { class: 'truncate', title: text }, SEARCH_KEYS.has(def.key) ? highlight(text) : text)
}

const tableColumns = computed<Column<DetailRow>[]>(() => visibleDefs.value.map(def => ({
  key: def.key,
  dataKey: def.key,
  title: def.title,
  width: def.width,
  fixed: def.fixed ? 'left' : undefined,
  align: def.kind === 'money' ? 'right' : 'left',
  sortable: true,
  cellRenderer: ({ rowData }: { rowData: DetailRow }) => renderCell(def, rowData),
}) as Column<DetailRow>))

/**
 * 排序方向自行维护：el-table-v2 按 oppositeOrderMap[sortBy.order] 推算下一个方向，
 * 未排序时 sortBy 为空会得到 undefined。同一列点击循环：升序 → 降序 → 取消
 */
function onColumnSort({ key }: SortBy) {
  const prev = sortBy.value
  if (prev?.key !== key)
    sortBy.value = { key, order: TableV2SortOrder.ASC }
  else if (prev.order === TableV2SortOrder.ASC)
    sortBy.value = { key, order: TableV2SortOrder.DESC }
  else
    sortBy.value = undefined
}

function onRowsRendered({ rowVisibleStart, rowVisibleEnd }: { rowVisibleStart: number, rowVisibleEnd: number }) {
  win.ensureRange(rowVisibleStart, rowVisibleEnd)
}

const rowEventHandlers = {
  onClick: ({ rowData }: { rowData: DetailRow }) => {
    if (rowData.__loaded)
      selectedRow.value = rowData
  },
}

function rowClass({ rowData }: { rowData: DetailRow }) {
  return rowData.__index === selectedRow.value?.__index ? 'is-selected' : ''
}

// ── 时间范围：限制在当月 ──────────────────────────────
function disabledDate(date: Date) {
  const [y, m] = props.period.split('-').map(Number)
  return date.getFullYear() !== y || date.getMonth() + 1 !== m
}

// ── 导出 ──────────────────────────────────────────────
async function exportCsv() {
  const count = summary.value?.totalCount ?? 0
  if (!count)
    return
  if (count > 300_000) {
    try {
      await ElMessageBox.confirm(tr('将导出 {0} 行，文件较大，确定继续？', [count.toLocaleString()]), tr('导出明细'), { type: 'warning' })
    }
    catch {
      return
    }
  }
  exporting.value = true
  try {
    const res = await exportLedgerDetailsApi(requestFilter.value, requestSort.value)
    const disposition = String(res.headers['content-disposition'] ?? '')
    const name = decodeURIComponent(disposition.replace(/^.*filename=/, '')) || `资金流水明细_${props.period}.csv`
    const a = document.createElement('a')
    a.href = URL.createObjectURL(res.data)
    a.download = name
    a.click()
    URL.revokeObjectURL(a.href)
  }
  finally {
    exporting.value = false
  }
}
</script>

<template>
  <el-drawer
    :model-value="visible"
    size="94%"
    :with-header="false"
    destroy-on-close
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="h-full flex flex-col gap-2">
      <!-- 标题 -->
      <div class="flex items-center gap-3">
        <span class="text-base font-semibold text-gray-900">{{ $t('流水明细 · {0}', [period]) }}</span>
        <el-tag v-if="target" type="success" effect="light" round>
          {{ target.title }}
        </el-tag>
        <el-switch
          v-if="target?.rule"
          v-model="useRule"
          size="small"
          :active-text="$t('只看对该项有贡献的流水')"
        />
        <div class="flex-1" />
        <el-button text @click="emit('update:visible', false)">
          {{ $t('关闭') }}
        </el-button>
      </div>

      <!-- 与汇总对比 -->
      <div
        v-if="check"
        class="flex items-center gap-4 rounded px-3 py-2 text-[13px]"
        :class="check.ok ? 'bg-[#E7F4EC] text-[#0B5A2E]' : 'bg-[#FEE2E2] text-[#991B1B]'"
      >
        <span>{{ check.label }}：<b class="tabular-nums">{{ check.expected }}</b></span>
        <span>{{ check.actualLabel }}：<b class="tabular-nums">{{ check.actual }}</b></span>
        <b>{{ check.ok ? $t('✓ 一致') : $t('✗ 不一致') }}</b>
        <span v-if="hasExtraFilters" class="text-xs opacity-75">{{ $t('已叠加筛选，对比仅供参考') }}</span>
      </div>

      <!-- 筛选工具栏 -->
      <div class="flex flex-wrap items-center gap-2">
        <el-input
          v-model="keyword"
          clearable
          class="w-80"
          :placeholder="$t('搜索订单号 / 流水号 / 售后编号 / 商品 / 达人 / 备注')"
        />
        <el-date-picker
          v-model="timeRange"
          type="datetimerange"
          value-format="YYYY-MM-DD HH:mm:ss"
          :start-placeholder="$t('动账开始时间')"
          :end-placeholder="$t('结束时间')"
          :disabled-date="disabledDate"
          :default-time="[new Date(2000, 0, 1, 0, 0, 0), new Date(2000, 0, 1, 23, 59, 59)]"
          class="!w-90"
        />
        <div class="flex items-center gap-1">
          <el-input-number v-model="amountMin" :controls="false" :placeholder="$t('金额 ≥')" class="!w-24" />
          <span class="text-gray-400">~</span>
          <el-input-number v-model="amountMax" :controls="false" :placeholder="$t('金额 ≤')" class="!w-24" />
        </div>
        <div class="flex-1" />
        <el-radio-group v-model="columnMode" size="small">
          <el-radio-button value="common">
            {{ $t('常用列') }}
          </el-radio-button>
          <el-radio-button value="all">
            {{ $t('全部列') }}
          </el-radio-button>
        </el-radio-group>
        <el-button size="small" :loading="exporting" :disabled="!summary?.totalCount" @click="exportCsv">
          {{ $t('导出 CSV') }}
        </el-button>
      </div>

      <!-- 统计 -->
      <div class="flex items-center gap-5 border-0 border-y border-solid border-[#f0f0f0] py-1.5 text-[13px] text-gray-600">
        <span>{{ $t('共') }} <b class="text-gray-900 tabular-nums">{{ (summary?.totalCount ?? 0).toLocaleString() }}</b> {{ $t('笔') }}</span>
        <span>{{ $t('入账') }} <b class="text-[#107C41] tabular-nums">{{ formatMoney(summary?.inAmount ?? 0) }}</b></span>
        <span>{{ $t('出账') }} <b class="text-[#DC2626] tabular-nums">{{ formatMoney(summary?.outAmount ?? 0) }}</b></span>
        <span>{{ $t('净额') }} <b class="text-gray-900 tabular-nums">{{ formatMoney(summary?.netAmount ?? 0) }}</b></span>
        <span v-if="ruleActive && summary?.contribution != null">{{ $t('贡献合计') }} <b class="text-gray-900 tabular-nums">{{ formatMoney(summary.contribution) }}</b></span>
        <el-popover trigger="click" placement="bottom-start" :width="300">
          <template #reference>
            <button class="text-[#107C41] hover:underline">
              {{ $t('各金额列合计') }}
            </button>
          </template>
          <div class="max-h-80 overflow-y-auto text-[12px]">
            <div v-for="[name, v] in nonZeroSums" :key="name" class="flex justify-between py-0.5">
              <span class="text-gray-500">{{ name }}</span>
              <span class="tabular-nums" :class="{ 'text-[#DC2626]': v < 0 }">{{ formatMoney(v) }}</span>
            </div>
            <div v-if="!nonZeroSums.length" class="text-gray-400">
              {{ $t('无') }}
            </div>
          </div>
        </el-popover>
        <span v-if="loading || win.pending.value" class="text-xs text-gray-400">{{ $t('加载中…') }}</span>
      </div>

      <!-- 主体：分面 | 表格 | 行详情 -->
      <div class="min-h-0 flex flex-1 gap-3">
        <div class="w-60 shrink-0">
          <FacetPanel
            :facets="summary?.facets ?? {}"
            :selected="selected"
            :hidden="hiddenFacets"
            @toggle="toggleFacet"
            @clear="key => (selected[key] = [])"
          />
        </div>
        <div class="relative min-w-0 flex-1">
          <el-auto-resizer>
            <template #default="{ height, width }">
              <el-table-v2
                class="ledger-detail-grid"
                :columns="tableColumns"
                :data="win.rows.value"
                :width="width"
                :height="height"
                :row-height="32"
                :header-height="36"
                row-key="__index"
                fixed
                :sort-by="sortBy ?? { key: '', order: TableV2SortOrder.ASC }"
                :row-class="rowClass"
                :row-event-handlers="rowEventHandlers"
                :on-rows-rendered="onRowsRendered"
                @column-sort="onColumnSort"
              />
            </template>
          </el-auto-resizer>
          <el-empty
            v-if="summary && !summary.totalCount && !loading"
            :description="$t('没有符合条件的流水')"
            class="absolute inset-0 bg-white/80"
          />
        </div>
        <div v-if="selectedRow" class="w-80 shrink-0">
          <RowInspector :row="selectedRow" :with-contribution="ruleActive" @close="selectedRow = null" />
        </div>
      </div>
    </div>
  </el-drawer>
</template>

<style scoped lang="scss">
.ledger-detail-grid {
  font-size: 12px;

  :deep(.#{$namespace}-table-v2__header-cell) {
    @apply bg-[#F7F8FA] font-semibold text-gray-600;
  }

  :deep(.#{$namespace}-table-v2__row) {
    cursor: pointer;

    &:hover {
      @apply bg-[#F5FAF7];
    }

    &.is-selected {
      @apply bg-[#E7F4EC];
    }
  }

  :deep(.cell-skeleton) {
    @apply block h-3 w-4/5 rounded bg-gray-100;
  }

  :deep(.kw-hit) {
    @apply rounded-sm bg-[#FEF08A] px-0 text-inherit;
  }

  :deep(.dir-in) {
    @apply text-[#107C41];
  }

  :deep(.dir-out) {
    @apply text-[#DC2626];
  }
}
</style>
