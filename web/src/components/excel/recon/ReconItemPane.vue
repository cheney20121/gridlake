<script setup lang="ts">
import type { ReconItem } from '@/api/recon'
import { onBeforeUnmount, ref, watch } from 'vue'
import { getReconItemsApi } from '@/api/recon'
import { errorText } from '@/utils/busy-hint'
import { formatMoney } from './recon-pure'

/** 对账工作台的一侧：未勾对的行（搜索、按金额找对象、分页），勾选的行由上层按行标识保存，翻页不丢 */
const props = defineProps<{
  reconId: string
  side: 'left' | 'right'
  label: string
  /** 已勾选：行标识 → 金额 */
  selected: Map<string, number>
  /** 上层让它重新加载的版本号 */
  reload: number
}>()

const emit = defineEmits<{
  (e: 'toggle', item: ReconItem, checked: boolean): void
  (e: 'togglePage', items: ReconItem[], checked: boolean): void
}>()

/** 按金额找对象：只看和它相差不超过容差的行 */
const amount = defineModel<number | null>('amount', { default: null })

const PAGE = 100
const page = ref(1)
const keyword = ref('')
const data = ref<{ total: number, sum: number, rows: ReconItem[] }>({ total: 0, sum: 0, rows: [] })
const loading = ref(false)
const error = ref('')
let ctl: AbortController | null = null
let timer: ReturnType<typeof setTimeout> | undefined

async function load() {
  ctl?.abort()
  const c = new AbortController()
  ctl = c
  loading.value = true
  error.value = ''
  try {
    const r = await getReconItemsApi(props.reconId, {
      side: props.side,
      status: 'open',
      q: keyword.value.trim() || undefined,
      amount: amount.value ?? undefined,
      offset: (page.value - 1) * PAGE,
      limit: PAGE,
    }, c.signal)
    if (c.signal.aborted)
      return
    data.value = r
    // 翻到了最后一页之后的位置（勾对后行变少）：回到最后一页
    if (!r.rows.length && r.total > 0 && page.value > 1)
      page.value = Math.ceil(r.total / PAGE)
  }
  catch (e) {
    if (!c.signal.aborted)
      error.value = errorText(e)
  }
  finally {
    if (ctl === c)
      loading.value = false
  }
}

watch([() => props.reconId, () => props.reload, page, amount], load, { immediate: true })
watch(keyword, () => {
  clearTimeout(timer)
  timer = setTimeout(() => {
    page.value = 1
    load()
  }, 300)
})
watch(amount, () => {
  page.value = 1
})
onBeforeUnmount(() => {
  ctl?.abort()
  clearTimeout(timer)
})

const allOnPage = () => data.value.rows.length > 0 && data.value.rows.every(r => props.selected.has(r.key))
const filtered = () => !!keyword.value.trim() || amount.value != null
</script>

<template>
  <div class="min-h-0 flex flex-col border border-[#e5e7eb] rounded-lg border-solid bg-white">
    <div class="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-[#eef0ef] border-b-solid px-3 py-2">
      <b class="text-sm text-gray-800">{{ label }}</b>
      <span class="text-xs text-gray-500">{{ $t('未勾对') }} <b class="tabular-nums">{{ data.total.toLocaleString() }}</b> {{ $t('笔，合计') }} <b class="tabular-nums" :class="data.sum < 0 ? 'text-[#B91C1C]' : ''">{{ formatMoney(data.sum) }}</b></span>
      <el-input v-model="keyword" size="small" clearable :placeholder="$t('搜索对账键 / 摘要')" class="ml-auto !w-44" />
      <el-input-number v-model="amount" size="small" :controls="false" :precision="2" :placeholder="$t('按金额找')" class="!w-28" :value-on-clear="null" />
    </div>

    <div v-if="error" class="flex flex-1 flex-col items-center justify-center gap-2 py-8 text-sm text-[#B91C1C]">
      <span>{{ error }}</span>
      <el-button size="small" @click="load">
        {{ $t('重试') }}
      </el-button>
    </div>
    <el-table
      v-else
      v-loading="loading"
      :data="data.rows"
      size="small"
      height="100%"
      class="min-h-0 flex-1"
      row-key="key"
      :row-class-name="({ row }: { row: ReconItem }) => selected.has(row.key) ? 'picked' : ''"
      :empty-text="filtered() ? $t('没有符合条件的行') : $t('✓ 没有未勾对的了')"
      @row-click="(row: ReconItem) => emit('toggle', row, !selected.has(row.key))"
    >
      <el-table-column width="40" align="center">
        <template #header>
          <el-checkbox :model-value="allOnPage()" :indeterminate="!allOnPage() && data.rows.some(r => selected.has(r.key))" @change="(v: unknown) => emit('togglePage', data.rows, !!v)" />
        </template>
        <template #default="{ row }">
          <el-checkbox :model-value="selected.has(row.key)" @click.stop @change="(v: unknown) => emit('toggle', row, !!v)" />
        </template>
      </el-table-column>
      <el-table-column :label="$t('日期')" width="96" prop="date">
        <template #default="{ row }">
          <span class="tabular-nums">{{ row.date ?? '—' }}</span>
        </template>
      </el-table-column>
      <el-table-column :label="$t('金额')" width="116" align="right">
        <template #default="{ row }">
          <span class="tabular-nums" :class="row.amount < 0 ? 'text-[#B91C1C]' : ''">{{ formatMoney(row.amount) }}</span>
        </template>
      </el-table-column>
      <el-table-column :label="$t('对账键')" min-width="110" show-overflow-tooltip>
        <template #default="{ row }">
          {{ row.corr ?? '' }}
        </template>
      </el-table-column>
      <el-table-column :label="$t('摘要')" min-width="140" show-overflow-tooltip>
        <template #default="{ row }">
          <el-tag v-if="row.stale" size="small" type="warning" class="mr-1" :title="$t('这一行原来勾对过，但对方的行被改过或删了，勾对已失效')">
            {{ $t('勾对已失效') }}
          </el-tag>{{ row.memo ?? '' }}
        </template>
      </el-table-column>
    </el-table>

    <div v-if="data.total > PAGE" class="flex items-center justify-between border-t border-[#eef0ef] border-t-solid px-3 py-1.5">
      <el-pagination v-model:current-page="page" small layout="prev, pager, next" :page-size="PAGE" :total="data.total" :pager-count="5" />
      <span class="text-xs text-gray-400">{{ $t('每页 {0} 笔', [PAGE]) }}</span>
    </div>
  </div>
</template>

<style scoped lang="scss">
:deep(.picked > td) {
  background: #eaf6ee !important;
}

:deep(.#{$namespace}-table__row) {
  cursor: pointer;
}
</style>
