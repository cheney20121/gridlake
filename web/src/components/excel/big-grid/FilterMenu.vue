<script setup lang="ts">
import type { GridColumn } from './types'
import type { DatasetFilter, DatasetQuery, DistinctResult } from '@/api/dataset'
import { useDebounceFn } from '@vueuse/core'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { getDistinctApi } from '@/api/dataset'
import { t } from '@/i18n'

/** 列头筛选菜单（Excel 自动筛选）：排序、按值勾选、包含文字、数值 / 日期区间 */
const props = defineProps<{
  datasetId: string
  column: GridColumn
  /** 当前全部查询条件（候选值按「其他列的筛选」统计） */
  query: DatasetQuery
  /** 该列当前的筛选条件 */
  current: DatasetFilter | null
  sortDesc: boolean | null
  anchor: DOMRect
  /** 不显示排序（透视表筛选只需要按值筛选） */
  hideSort?: boolean
}>()

const emit = defineEmits<{
  (e: 'apply', filter: DatasetFilter | null): void
  (e: 'sort', desc: boolean | null): void
  (e: 'close'): void
}>()

const numeric = computed(() => props.column.kind === 'money' || props.column.kind === 'number')
const isDate = computed(() => props.column.kind === 'datetime')
const isContribution = computed(() => props.column.key === '__contribution')

const menuRef = ref<HTMLDivElement>()
const style = computed(() => {
  const w = 300
  const left = Math.min(window.innerWidth - w - 8, Math.max(8, props.anchor.right - w))
  return { left: `${left}px`, top: `${props.anchor.bottom + 4}px`, width: `${w}px` }
})

// ── 按值筛选 ──
const search = ref('')
const distinct = ref<DistinctResult | null>(null)
const loading = ref(false)
/** null = 全部（不筛选） */
const checked = ref<Set<string> | null>(props.current?.op === 'in' ? new Set(props.current.values) : null)
const containsText = ref(props.current?.op === 'contains' ? props.current.value : '')
const rangeMin = ref<number | string | undefined>(props.current?.op === 'between' ? (props.current.min ?? undefined) : undefined)
const rangeMax = ref<number | string | undefined>(props.current?.op === 'between' ? (props.current.max ?? undefined) : undefined)
const dateRange = ref<[string, string] | null>(
  props.current?.op === 'between' && isDate.value ? [String(props.current.min ?? ''), String(props.current.max ?? '')] : null,
)

let controller: AbortController | null = null
async function loadDistinct() {
  if (isContribution.value)
    return
  controller?.abort()
  controller = new AbortController()
  loading.value = true
  try {
    distinct.value = await getDistinctApi(props.datasetId, { ...props.query, column: props.column.key, search: search.value || undefined }, controller.signal)
  }
  catch {
    // 取消或失败时保留上次结果
  }
  finally {
    loading.value = false
  }
}
const debouncedLoad = useDebounceFn(loadDistinct, 250)

const truncated = computed(() => !!distinct.value && distinct.value.totalDistinct > distinct.value.values.length)

/**
 * 候选值只返回前 N 个：此时在列表里勾选 / 取消会把「没显示出来的值」一并排除（等于悄悄丢掉大部分数据），所以列表只读，
 * 要求先用上面的搜索把候选缩小到能完整显示
 */
const listLocked = computed(() => truncated.value)

/** 数值 / 时间列的不同值通常很多（金额几乎每行都不同），按值勾选没意义：默认折叠，用户需要时再展开 */
const valuesOpen = ref(false)
const valuesCollapsible = computed(() => (numeric.value || isDate.value) && (distinct.value?.totalDistinct ?? 0) > 30)
const showValues = computed(() => !valuesCollapsible.value || valuesOpen.value)

function isChecked(v: string) {
  return checked.value == null || checked.value.has(v)
}

function toggle(v: string) {
  const set = checked.value ? new Set(checked.value) : new Set(distinct.value?.values.map(x => x.value) ?? [])
  if (set.has(v))
    set.delete(v)
  else
    set.add(v)
  checked.value = set
}

function checkAll(all: boolean) {
  checked.value = all ? null : new Set()
}

const display = (v: string) => (v === '' ? t('（空白）') : v)

function apply() {
  if (numeric.value || isContribution.value) {
    const hasRange = (rangeMin.value != null && rangeMin.value !== '') || (rangeMax.value != null && rangeMax.value !== '')
    if (hasRange) {
      emit('apply', { op: 'between', column: props.column.key, min: rangeMin.value ?? null, max: rangeMax.value ?? null })
      return
    }
  }
  if (isDate.value && dateRange.value) {
    emit('apply', { op: 'between', column: props.column.key, min: dateRange.value[0], max: dateRange.value[1] })
    return
  }
  if (containsText.value.trim()) {
    emit('apply', { op: 'contains', column: props.column.key, value: containsText.value.trim() })
    return
  }
  if (checked.value != null) {
    emit('apply', { op: 'in', column: props.column.key, values: [...checked.value] })
    return
  }
  emit('apply', null)
}

// 点击菜单外部关闭
function onOutside(e: MouseEvent) {
  if (menuRef.value && !menuRef.value.contains(e.target as Node) && !(e.target as HTMLElement).closest('.ec-popper, .el-popper'))
    emit('close')
}
function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape')
    emit('close')
}

onMounted(() => {
  loadDistinct()
  setTimeout(() => document.addEventListener('mousedown', onOutside), 0)
  document.addEventListener('keydown', onKey)
})
onBeforeUnmount(() => {
  controller?.abort()
  document.removeEventListener('mousedown', onOutside)
  document.removeEventListener('keydown', onKey)
})
</script>

<template>
  <div ref="menuRef" class="menu" :style="style">
    <div v-if="!hideSort" class="flex flex-col gap-0.5 border-b border-[#eee] border-b-solid pb-1.5">
      <button class="item" :class="{ active: sortDesc === false }" @click="emit('sort', false)">
        ↑ {{ numeric ? $t('升序（从小到大）') : $t('升序（A → Z）') }}
      </button>
      <button class="item" :class="{ active: sortDesc === true }" @click="emit('sort', true)">
        ↓ {{ numeric ? $t('降序（从大到小）') : $t('降序（Z → A）') }}
      </button>
      <button v-if="sortDesc != null" class="item text-gray-500" @click="emit('sort', null)">
        {{ $t('清除排序') }}
      </button>
    </div>

    <div class="mt-2 flex flex-col gap-2">
      <!-- 数值区间 -->
      <template v-if="numeric || isContribution">
        <div class="text-xs text-gray-500">
          {{ $t('数值区间{0}', [distinct?.min != null ? $t('（当前 {0} ~ {1}）', [distinct.min, distinct.max]) : '']) }}
        </div>
        <div class="flex items-center gap-1.5">
          <el-input-number v-model="rangeMin as number" size="small" :controls="false" :placeholder="$t('最小')" class="!w-full" />
          <span class="text-gray-400">~</span>
          <el-input-number v-model="rangeMax as number" size="small" :controls="false" :placeholder="$t('最大')" class="!w-full" />
        </div>
      </template>

      <!-- 日期区间 -->
      <template v-if="isDate">
        <div class="text-xs text-gray-500">
          {{ $t('时间区间') }}
        </div>
        <el-date-picker
          v-model="dateRange"
          type="datetimerange"
          size="small"
          value-format="YYYY-MM-DD HH:mm:ss"
          :start-placeholder="$t('开始')"
          :end-placeholder="$t('结束')"
          class="!w-full"
        />
      </template>

      <!-- 按值勾选 -->
      <template v-if="!isContribution">
        <el-input v-model="containsText" size="small" clearable :placeholder="$t('包含文字（优先于勾选）')" @keydown.enter="apply" />
        <button v-if="valuesCollapsible" class="text-left text-xs text-[#217346]" @click="valuesOpen = !valuesOpen">
          {{ $t('{0} 按值勾选（共 {1} 个不同的值）', [valuesOpen ? '▾' : '▸', distinct!.totalDistinct.toLocaleString()]) }}
        </button>
        <template v-if="showValues">
          <el-input v-model="search" size="small" clearable :placeholder="$t('搜索候选值')" @input="debouncedLoad" />
          <div class="flex items-center gap-3 text-xs">
            <button class="text-[#217346] disabled:text-gray-300" :disabled="listLocked" @click="checkAll(true)">
              {{ $t('全选') }}
            </button>
            <button class="text-[#217346] disabled:text-gray-300" :disabled="listLocked" @click="checkAll(false)">
              {{ $t('清空') }}
            </button>
            <span v-if="distinct && !valuesCollapsible" class="ml-auto text-gray-400">{{ $t('共 {0} 个值', [distinct.totalDistinct.toLocaleString()]) }}</span>
          </div>
          <div v-loading="loading" class="values" :class="{ locked: listLocked }">
            <label v-for="v in distinct?.values ?? []" :key="v.value" class="value-row" :class="{ 'cursor-not-allowed': listLocked }">
              <input type="checkbox" :checked="isChecked(v.value)" :disabled="listLocked" @change="toggle(v.value)">
              <span class="min-w-0 flex-1 truncate" :title="display(v.value)">{{ display(v.value) }}</span>
              <span class="text-gray-400 tabular-nums">{{ v.count.toLocaleString() }}</span>
            </label>
          </div>
          <div v-if="truncated" class="rounded bg-[#FEF3C7] px-2 py-1.5 text-xs text-[#92400E] leading-5">
            {{ $t('值太多（共 {0} 个），只列出前 {1} 个，暂不能勾选。请在上面输入文字搜索，缩小后再勾选；或直接用「包含文字」{2}筛选', [distinct!.totalDistinct.toLocaleString(), distinct!.values.length, numeric || isDate ? $t('、区间') : '']) }}
          </div>
        </template>
      </template>
    </div>

    <div class="mt-3 flex justify-end gap-2">
      <el-button size="small" @click="emit('apply', null)">
        {{ $t('清除筛选') }}
      </el-button>
      <el-button size="small" type="primary" @click="apply">
        {{ $t('确定') }}
      </el-button>
    </div>
  </div>
</template>

<style scoped lang="scss">
.menu {
  @apply fixed z-[3000] rounded-md bg-white p-2.5 text-[13px] shadow-lg;
  border: 1px solid #e4e4e4;
}

.item {
  @apply rounded border-none bg-transparent px-2 py-1 text-left hover:bg-[#F2F2F2];

  &.active {
    @apply bg-[#E7F4EC] text-[#0E5A31];
  }
}

.values {
  @apply max-h-56 min-h-16 overflow-auto rounded border border-[#eee] border-solid py-1;

  &.locked {
    @apply bg-[#FAFAFA] text-gray-400;
  }
}

.value-row {
  @apply flex cursor-pointer items-center gap-2 px-2 py-0.5 hover:bg-[#F7F7F7];
}
</style>
