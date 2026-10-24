<script setup lang="ts">
import type { ColumnDetail, ColumnProfile, DatasetFilter, DatasetProfile, DatasetQuery, ProfileBucket } from '@/api/dataset'
import { isCancel } from 'axios'
import { computed, ref, watch } from 'vue'
import { profileColumnApi, profileDatasetApi } from '@/api/dataset'
import { formatBool, isNumericKind, kindLabel } from '@/features/excel/dataset/column-kinds'
import { t } from '@/i18n'
import { errorText } from '@/utils/busy-hint'
import { formatMoney } from '@/utils/format-money'

/**
 * 数据概况：按数据表当前的筛选，每列空值多少、有多少种取值、范围与分布（服务端一次扫描，亿级也是几秒）
 * 点开一列看直方图和最常见的取值；点一档 / 一个取值就按它筛选数据表（概况随之重算）
 */
const props = defineProps<{
  visible: boolean
  datasetId: string
  query: DatasetQuery
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'filter', filter: DatasetFilter): void
}>()

const profile = ref<DatasetProfile | null>(null)
const loading = ref(false)
const error = ref('')
const expanded = ref<string | null>(null)
const detail = ref<ColumnDetail | null>(null)
const detailLoading = ref(false)
let controller: AbortController | null = null
let detailController: AbortController | null = null

const queryKey = computed(() => JSON.stringify([props.datasetId, props.query.filters, props.query.keyword, props.query.rule]))

async function load() {
  controller?.abort()
  controller = new AbortController()
  loading.value = true
  error.value = ''
  try {
    profile.value = await profileDatasetApi(props.datasetId, props.query, controller.signal)
    if (expanded.value)
      loadDetail(expanded.value)
  }
  catch (e) {
    if (!isCancel(e))
      error.value = errorText(e)
  }
  finally {
    loading.value = false
  }
}

async function loadDetail(key: string) {
  detailController?.abort()
  detailController = new AbortController()
  detailLoading.value = true
  try {
    detail.value = await profileColumnApi(props.datasetId, props.query, key, detailController.signal)
  }
  catch (e) {
    if (!isCancel(e))
      ElMessage.error(errorText(e))
  }
  finally {
    detailLoading.value = false
  }
}

watch(() => [props.visible, queryKey.value], () => {
  if (props.visible)
    load()
}, { immediate: true })

watch(() => props.datasetId, () => {
  expanded.value = null
  detail.value = null
})

function toggle(c: ColumnProfile) {
  if (expanded.value === c.key) {
    expanded.value = null
    detail.value = null
    return
  }
  expanded.value = c.key
  detail.value = null
  loadDetail(c.key)
}

// ── 显示 ──

function num(v: unknown, kind: string): string {
  if (v == null || v === '')
    return '—'
  if (typeof v === 'boolean')
    return formatBool(v)
  const n = typeof v === 'number' ? v : Number(v)
  if (!Number.isFinite(n))
    return String(v)
  if (kind === 'money')
    return formatMoney(n)
  return n.toLocaleString(undefined, { maximumFractionDigits: Math.abs(n) >= 100 ? 0 : 2 })
}

function pct(part: number, whole: number): string {
  if (!whole || !part)
    return '0%'
  const p = (part / whole) * 100
  return p > 0 && p < 0.1 ? '<0.1%' : `${p.toFixed(p < 10 ? 1 : 0)}%`
}

/** 一列的要点：一行字 */
function summary(c: ColumnProfile): string {
  if (!c.nonNull)
    return t('全是空值')
  if (isNumericKind(c.kind))
    return t('{0} ~ {1} · 平均 {2} · 中位数约 {3} · 合计 {4}', [num(c.min, c.kind), num(c.max, c.kind), num(c.mean, c.kind), num(c.median, c.kind), num(c.sum, c.kind)])
  if (c.kind === 'datetime')
    return t('{0} ~ {1}', [String(c.min ?? ''), String(c.max ?? '')])
  if (c.kind === 'boolean')
    return t('是 {0}（{1}）· 否 {2}', [(c.trues ?? 0).toLocaleString(), pct(c.trues ?? 0, c.nonNull), (c.nonNull - (c.trues ?? 0)).toLocaleString()])
  return c.minLen === c.maxLen ? t('长度都是 {0}', [c.minLen ?? 0]) : t('长度 {0} ~ {1}（平均 {2}）', [c.minLen ?? 0, c.maxLen ?? 0, (c.avgLen ?? 0).toFixed(1)])
}

/** 值得注意的情况 */
function badges(c: ColumnProfile, rows: number): { text: string, type: 'danger' | 'warning' | 'success' | 'info' }[] {
  const out: { text: string, type: 'danger' | 'warning' | 'success' | 'info' }[] = []
  if (c.invalid)
    out.push({ text: t('{0} 个认不出的日期', [c.invalid.toLocaleString()]), type: 'danger' })
  if (c.blanks)
    out.push({ text: t('{0} 个只有空白', [c.blanks.toLocaleString()]), type: 'warning' })
  if (c.negatives)
    out.push({ text: t('{0} 个负数', [c.negatives.toLocaleString()]), type: 'info' })
  if (rows > 1 && c.distinctExact && c.distinct === rows)
    out.push({ text: t('每行都不同'), type: 'success' })
  else if (rows > 1 && c.nonNull > 0 && c.distinct === 1)
    out.push({ text: t('只有一个值'), type: 'info' })
  return out
}

function distinctText(c: ColumnProfile): string {
  return c.distinctExact ? c.distinct.toLocaleString() : t('约 {0}', [c.distinct.toLocaleString()])
}

const maxBucket = computed(() => Math.max(1, ...(detail.value?.histogram ?? []).map(b => b.count)))
const maxTop = computed(() => Math.max(1, ...(detail.value?.top ?? []).map(v => v.count)))

const GRAIN: Record<string, string> = { bin: t('按区间'), value: t('按取值'), day: t('按日'), month: t('按月'), year: t('按年') }

function bucketFilter(d: ColumnDetail, b: ProfileBucket): DatasetFilter | null {
  if (b.prefix != null)
    return { op: 'prefix', column: d.key, value: b.prefix }
  if (b.from != null && b.to != null)
    return { op: 'between', column: d.key, min: b.from, max: b.to }
  return null
}

function pickBucket(d: ColumnDetail, b: ProfileBucket) {
  const f = bucketFilter(d, b)
  if (f && b.count)
    emit('filter', f)
}

function pickValue(d: ColumnDetail, value: string) {
  emit('filter', { op: 'in', column: d.key, values: [value] })
}

function valueText(d: ColumnDetail, v: string): string {
  if (d.kind === 'boolean')
    return formatBool(v)
  return isNumericKind(d.kind) ? num(v, d.kind) : v
}
</script>

<template>
  <!-- 不遮住数据表：点一档筛选后，表格就在旁边变化 -->
  <el-drawer :model-value="visible" :title="$t('数据概况')" size="560px" append-to-body :modal="false" modal-class="pointer-events-none" class="pointer-events-auto" @update:model-value="emit('update:visible', $event)">
    <div class="flex flex-col gap-2 text-[13px]" data-testid="profile-drawer">
      <div class="flex items-center gap-2 text-gray-600">
        <span v-if="profile">{{ $t('按当前筛选 {0} 行 · {1} 列 · 用时 {2} 秒', [profile.rows.toLocaleString(), profile.columns.length, (profile.elapsedMs / 1000).toFixed(1)]) }}</span>
        <span v-else-if="loading">{{ $t('正在逐列统计…') }}</span>
        <el-button link size="small" class="ml-auto" :loading="loading" @click="load">
          {{ $t('重新统计') }}
        </el-button>
      </div>
      <div class="text-xs text-gray-400 leading-5">
        {{ $t('点一列看分布；点直方图的一档或某个取值，数据表就按它筛选。不同值个数很多时是估计值，中位数是近似值。') }}
      </div>
      <div v-if="error" class="rounded bg-[#FEF2F2] px-3 py-2 text-[#B91C1C] leading-5" role="alert">
        {{ error }}
      </div>

      <div v-loading="loading && !profile" class="min-h-40">
        <div v-for="c in profile?.columns ?? []" :key="c.key" class="col" :class="{ open: expanded === c.key }" :data-testid="`profile-col-${c.title}`">
          <div class="head" role="button" @click="toggle(c)">
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-1.5">
                <span class="truncate text-gray-900 font-medium">{{ c.title }}</span>
                <span class="text-xs text-gray-400">{{ kindLabel(c.kind) }}{{ c.computed ? $t(' · 计算列') : '' }}</span>
                <el-tag v-for="b in badges(c, profile!.rows)" :key="b.text" size="small" :type="b.type" effect="plain" disable-transitions>
                  {{ b.text }}
                </el-tag>
              </div>
              <div class="mt-0.5 truncate text-xs text-gray-500" :title="summary(c)">
                {{ summary(c) }}
              </div>
            </div>
            <div class="w-28 shrink-0 text-right text-xs">
              <div class="text-gray-700">
                {{ $t('{0} 种', [distinctText(c)]) }}
              </div>
              <div :class="c.nulls ? 'text-[#B45309]' : 'text-gray-400'">
                {{ $t('空值 {0}', [pct(c.nulls, profile!.rows)]) }}
              </div>
              <div class="nullbar">
                <div :style="{ width: pct(c.nonNull, profile!.rows).replace('<', '') }" />
              </div>
            </div>
          </div>

          <div v-if="expanded === c.key" v-loading="detailLoading" class="detail">
            <template v-if="detail && detail.key === c.key">
              <template v-if="detail.histogram.length">
                <div class="mb-1 text-xs text-gray-500">
                  {{ $t('分布（{0}，点一档按它筛选）', [GRAIN[detail.granularity ?? 'bin']]) }}
                </div>
                <div class="hist" data-testid="profile-histogram">
                  <div
                    v-for="b in detail.histogram" :key="b.label" class="bar" :class="{ empty: !b.count }"
                    :title="`${b.label}：${b.count.toLocaleString()}（${pct(b.count, detail.rows)}）`"
                    @click="pickBucket(detail, b)"
                  >
                    <div :style="{ height: `${Math.max(b.count ? 3 : 0, (b.count / maxBucket) * 100)}%` }" />
                  </div>
                </div>
                <div class="flex justify-between text-[11px] text-gray-400">
                  <span>{{ detail.histogram[0]!.label }}</span>
                  <span>{{ detail.histogram[detail.histogram.length - 1]!.label }}</span>
                </div>
              </template>
              <div class="mb-1 mt-2 text-xs text-gray-500">
                {{ $t('最常见的取值（点一个按它筛选）') }}
              </div>
              <div v-for="v in detail.top" :key="v.value" class="topv" role="button" :data-testid="`profile-value-${v.value}`" @click="pickValue(detail, v.value)">
                <span class="w-40 shrink-0 truncate" :title="v.value">{{ valueText(detail, v.value) }}</span>
                <div class="topbar">
                  <div :style="{ width: `${(v.count / maxTop) * 100}%` }" />
                </div>
                <span class="w-28 shrink-0 text-right text-gray-500 tabular-nums">{{ v.count.toLocaleString() }}（{{ pct(v.count, detail.rows) }}）</span>
              </div>
              <div v-if="detail.others > 0" class="mt-1 text-xs text-gray-400">
                {{ $t('其余 {0} 个值', [detail.others.toLocaleString()]) }}
              </div>
              <div v-if="detail.nulls > 0" class="text-xs text-gray-400">
                {{ $t('空值 {0} 个', [detail.nulls.toLocaleString()]) }}
              </div>
              <div v-if="isNumericKind(c.kind) && c.nonNull" class="mt-2 text-xs text-gray-500 leading-5">
                {{ $t('四分位数约 {0} / {1} / {2}；标准差 {3}；零 {4} 个', [num(c.q1, c.kind), num(c.median, c.kind), num(c.q3, c.kind), num(c.std, c.kind), (c.zeros ?? 0).toLocaleString()]) }}
              </div>
            </template>
          </div>
        </div>
      </div>
    </div>
  </el-drawer>
</template>

<style scoped lang="scss">
.col {
  @apply mb-1.5 rounded-md border border-[#e5e7eb] border-solid;

  &.open {
    @apply border-[#86B49A];
  }
}

.head {
  @apply flex cursor-pointer items-center gap-3 px-3 py-2 hover:bg-[#f9faf9];
}

.nullbar {
  @apply mt-1 h-1 w-full overflow-hidden rounded bg-[#FDE7C7];

  > div {
    @apply h-full bg-[#2F7D4F];
  }
}

.detail {
  @apply min-h-20 border-t border-[#eef0ee] border-t-solid px-3 py-2;
}

.hist {
  @apply flex h-24 items-end gap-[2px];

  .bar {
    @apply h-full flex flex-1 cursor-pointer items-end;

    > div {
      @apply w-full rounded-t-sm bg-[#4C9A6A];
    }

    &:hover > div {
      @apply bg-[#2F7D4F];
    }

    &.empty {
      @apply cursor-default;
    }
  }
}

.topv {
  @apply flex cursor-pointer items-center gap-2 rounded px-1 py-0.5 text-xs hover:bg-[#f3f6f4];
}

.topbar {
  @apply h-2 min-w-0 flex-1 overflow-hidden rounded bg-[#eef0ee];

  > div {
    @apply h-full bg-[#86B49A];
  }
}
</style>
