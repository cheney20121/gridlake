<script setup lang="ts">
import type { ReconGroup, ReconGroupItem, ReconMethod } from '@/api/recon'
import { onBeforeUnmount, ref, watch } from 'vue'
import { getReconGroupsApi, unmatchReconApi } from '@/api/recon'
import { t } from '@/i18n'
import { errorText } from '@/utils/busy-hint'
import { formatMoney, METHOD_LABEL } from './recon-pure'

/** 已勾对的组：看每一组由哪些行组成、怎么勾上的、差额与原因，可取消 */
const props = defineProps<{
  reconId: string
  labels: { left: string, right: string }
  reload: number
}>()

const emit = defineEmits<{
  (e: 'changed'): void
}>()

const PAGE = 30
const page = ref(1)
const method = ref<ReconMethod | ''>('')
const withDiff = ref(false)
const broken = ref(false)
const keyword = ref('')
const data = ref<{ total: number, groups: ReconGroup[] }>({ total: 0, groups: [] })
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
    const r = await getReconGroupsApi(props.reconId, {
      method: method.value || undefined,
      withDiff: withDiff.value || undefined,
      broken: broken.value || undefined,
      q: keyword.value.trim() || undefined,
      offset: (page.value - 1) * PAGE,
      limit: PAGE,
    }, c.signal)
    if (c.signal.aborted)
      return
    data.value = r
    if (!r.groups.length && r.total > 0 && page.value > 1)
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

watch([() => props.reconId, () => props.reload, page, method, withDiff, broken], load, { immediate: true })
watch([method, withDiff, broken], () => {
  page.value = 1
})
watch(keyword, () => {
  clearTimeout(timer)
  timer = setTimeout(() => {
    page.value = 1
    load()
  }, 300)
})
onBeforeUnmount(() => {
  ctl?.abort()
  clearTimeout(timer)
})

async function undo(g: ReconGroup) {
  const ok = await ElMessageBox.confirm(t('取消勾对组 {0}？组里的 {1} 笔会回到未勾对。', [g.gno, g.items.length]), t('取消勾对'), { type: 'warning', confirmButtonText: t('取消勾对') }).then(() => true, () => false)
  if (!ok)
    return
  await unmatchReconApi(props.reconId, { groups: [g.gno] })
  ElMessage.success(t('已取消，这些行回到未勾对'))
  emit('changed')
}

const sideItems = (g: ReconGroup, side: 0 | 1) => g.items.filter(i => i.side === side)
const sideTotal = (items: ReconGroupItem[]) => items.reduce((s, i) => s + Math.round(i.amount * 100), 0) / 100
</script>

<template>
  <div class="min-h-0 flex flex-col">
    <div class="flex flex-wrap items-center gap-3 pb-2 text-[13px]">
      <el-select v-model="method" size="small" clearable :placeholder="$t('全部方式')" class="!w-32">
        <el-option v-for="(label, k) in METHOD_LABEL" :key="k" :label="label" :value="k" />
      </el-select>
      <el-checkbox v-model="withDiff" size="small">
        {{ $t('只看有差额的') }}
      </el-checkbox>
      <el-checkbox v-model="broken" size="small">
        {{ $t('只看已失效的') }}
      </el-checkbox>
      <el-input v-model="keyword" size="small" clearable :placeholder="$t('搜索对账键 / 摘要 / 备注')" class="!w-56" />
      <span class="ml-auto text-xs text-gray-500">{{ $t('共 {0} 组', [data.total.toLocaleString()]) }}</span>
    </div>

    <div v-if="error" class="flex flex-col items-center gap-2 py-10 text-sm text-[#B91C1C]">
      <span>{{ error }}</span>
      <el-button size="small" @click="load">
        {{ $t('重试') }}
      </el-button>
    </div>
    <div v-else v-loading="loading" class="min-h-0 flex-1 overflow-y-auto pr-1">
      <div v-for="g in data.groups" :key="g.gno" class="group-card" :class="{ bad: g.broken }">
        <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
          <b class="text-sm text-gray-800">#{{ g.gno }}</b>
          <el-tag size="small" :type="g.method === 'manual' ? 'warning' : 'success'" effect="plain">
            {{ METHOD_LABEL[g.method] }}
          </el-tag>
          <el-tag v-if="g.broken" size="small" type="danger">
            {{ $t('已失效：有的行被改过或删了') }}
          </el-tag>
          <span v-if="g.diff" class="text-[#B45309]">{{ $t('差额 {0}', [formatMoney(g.diff)]) }}</span>
          <span v-if="g.note" class="text-gray-600">{{ $t('备注：{0}', [g.note]) }}</span>
          <span class="ml-auto text-gray-400">{{ g.by ?? '' }} {{ g.at ?? '' }}</span>
          <el-button link size="small" type="danger" @click="undo(g)">
            {{ $t('取消勾对') }}
          </el-button>
        </div>
        <div class="mt-1.5 grid grid-cols-2 gap-3">
          <div v-for="s in [0, 1] as const" :key="s" class="min-w-0">
            <div class="mb-0.5 text-[11px] text-gray-400">
              {{ $t('{0} · {1} 笔 · 合计', [s === 0 ? labels.left : labels.right, sideItems(g, s).length]) }} <span class="tabular-nums">{{ formatMoney(sideTotal(sideItems(g, s))) }}</span>
            </div>
            <div v-for="(i, n) in sideItems(g, s)" :key="n" class="item-line" :class="{ gone: i.missing }">
              <span class="w-20 shrink-0 text-gray-500 tabular-nums">{{ i.date ?? '—' }}</span>
              <span class="w-24 shrink-0 text-right tabular-nums" :class="i.amount < 0 ? 'text-[#B91C1C]' : ''">{{ formatMoney(i.amount) }}</span>
              <span class="min-w-0 flex-1 truncate text-gray-600" :title="`${i.corr ?? ''} ${i.memo ?? ''}`">{{ i.missing ? $t('（这一行已被改过或删除）') : `${i.corr ?? ''} ${i.memo ?? ''}` }}</span>
            </div>
          </div>
        </div>
      </div>
      <div v-if="!data.groups.length && !loading" class="py-12 text-center text-sm text-gray-400">
        {{ method || withDiff || broken || keyword ? $t('没有符合条件的勾对组') : $t('还没有勾对。点「自动勾对」，或在「未勾对」里手工选择两边的行勾对') }}
      </div>
    </div>

    <div v-if="data.total > PAGE" class="flex items-center justify-between border-t border-[#eef0ef] border-t-solid pt-2">
      <el-pagination v-model:current-page="page" small layout="prev, pager, next" :page-size="PAGE" :total="data.total" :pager-count="7" />
      <span class="text-xs text-gray-400">{{ $t('每页 {0} 组', [PAGE]) }}</span>
    </div>
  </div>
</template>

<style scoped lang="scss">
.group-card {
  @apply mb-2 border border-[#e5e7eb] rounded-lg border-solid bg-white px-3 py-2;

  &.bad {
    @apply border-[#F3C7C7] bg-[#FFF8F8];
  }
}

.item-line {
  @apply flex items-center gap-2 py-0.5 text-xs;

  &.gone {
    @apply text-gray-400 line-through;
  }
}
</style>
