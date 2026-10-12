<script setup lang="ts">
import type { DatasetInfo } from '@/api/dataset'
import type { ReconInfo, ReconItem, ReconSideSummary, ReconSummary } from '@/api/recon'
import { computed, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { autoMatchReconApi, getReconApi, getReconSummaryApi, matchReconApi, unmatchReconApi } from '@/api/recon'
import { t } from '@/i18n'
import { errorText } from '@/utils/busy-hint'
import { autoConfirmText, formatMoney, summarizeSelection } from './recon-pure'
import ReconGroupList from './ReconGroupList.vue'
import ReconItemPane from './ReconItemPane.vue'

/**
 * 对账工作台：截至某日的未达账项、两侧未勾对的行（选中后勾对，一对一 / 一对多 / 多对多）、已勾对的组。
 * 自动勾对先试算再确认；手工勾对差额超过容差时必须写明原因
 */
const props = defineProps<{ dataset: DatasetInfo }>()

const emit = defineEmits<{
  (e: 'edit', id: string): void
  /** 看结果数据集的全部明细（透视、图表、导出都在那里） */
  (e: 'detail'): void
  /** 勾对有变化：结果数据集的数据变了（公式、图表需要重算） */
  (e: 'changed'): void
}>()

const id = computed(() => props.dataset.id)
const info = ref<ReconInfo | null>(null)
const summary = ref<ReconSummary | null>(null)
const error = ref('')
const asOf = ref('')
const tab = ref<'open' | 'matched'>('open')
const reload = ref(0)
const busy = ref('')

const selL = reactive(new Map<string, number>())
const selR = reactive(new Map<string, number>())
const amountL = ref<number | null>(null)
const amountR = ref<number | null>(null)

const labels = computed(() => ({ left: info.value?.def.left.label ?? 'A 方', right: info.value?.def.right.label ?? 'B 方' }))
const tolerance = computed(() => info.value?.def.tolerance ?? 0)
const sel = computed(() => summarizeSelection([...selL.values()], [...selR.values()], tolerance.value))

let ctl: AbortController | null = null
async function loadSummary() {
  ctl?.abort()
  const c = new AbortController()
  ctl = c
  try {
    const r = await getReconSummaryApi(id.value, asOf.value || undefined, c.signal)
    if (!c.signal.aborted)
      summary.value = r
  }
  catch (e) {
    if (!c.signal.aborted)
      error.value = errorText(e)
  }
}

async function init() {
  error.value = ''
  info.value = null
  summary.value = null
  selL.clear()
  selR.clear()
  amountL.value = amountR.value = null
  tab.value = 'open'
  try {
    info.value = await getReconApi(id.value)
  }
  catch (e) {
    error.value = errorText(e)
    return
  }
  await loadSummary()
}

watch(id, init, { immediate: true })
watch(asOf, loadSummary)
onBeforeUnmount(() => ctl?.abort())

/** 勾对有变化之后：清空选择、刷新列表与汇总，并通知上层刷新数据集（公式、图表重算） */
async function refresh() {
  selL.clear()
  selR.clear()
  reload.value++
  await loadSummary()
  emit('changed')
}

function toggle(map: Map<string, number>, item: ReconItem, checked: boolean) {
  if (checked)
    map.set(item.key, item.amount)
  else
    map.delete(item.key)
}

function togglePage(map: Map<string, number>, items: ReconItem[], checked: boolean) {
  for (const i of items) toggle(map, i, checked)
}

/** 选了一行之后，一键到另一边找同金额的 */
function findCounterpart() {
  const only = [...selL.values()]
  if (only.length === 1 && !selR.size)
    amountR.value = only[0] ?? null
  else if (selR.size === 1 && !selL.size)
    amountL.value = [...selR.values()][0] ?? null
}
const canFind = computed(() => (selL.size === 1 && !selR.size) || (selR.size === 1 && !selL.size))

async function match() {
  const s = sel.value
  if (!s.ready)
    return
  let note: string | undefined
  if (!s.balanced) {
    const text = `${labels.value.left} 合计 ${formatMoney(s.leftTotal)}，${labels.value.right} 合计 ${formatMoney(s.rightTotal)}，相差 ${formatMoney(s.diff)}`
      + `${tolerance.value ? `，超过了容差 ${formatMoney(tolerance.value)}` : ''}。仍要勾对的话，请写明差额的原因（如银行手续费、汇率差）：`
    note = await ElMessageBox.prompt(text, t('合计不相等'), {
      confirmButtonText: t('确认勾对'),
      inputPlaceholder: t('差额原因（必填）'),
      inputValidator: v => !!v?.trim() || t('请写明原因'),
    }).then(r => r.value.trim(), () => undefined)
    if (!note)
      return
  }
  busy.value = '正在勾对…'
  try {
    await matchReconApi(id.value, { left: [...selL.keys()], right: [...selR.keys()], note, force: !s.balanced })
    ElMessage.success(`已勾对 ${s.leftCount + s.rightCount} 笔${s.balanced ? '' : `（差额 ${formatMoney(s.diff)} 已记录）`}`)
    await refresh()
  }
  catch {
    // 行已被别人勾对或数据变了：服务端说明了原因，刷新列表让选择与现状一致
    await refresh()
  }
  finally {
    busy.value = ''
  }
}

async function autoMatch() {
  busy.value = '正在试算…'
  try {
    const dry = await autoMatchReconApi(id.value, true)
    busy.value = ''
    const text = autoConfirmText(dry, labels.value)
    if (!dry.groups && !dry.clearedBroken) {
      ElMessageBox.alert(text, t('自动勾对'), { confirmButtonText: t('知道了') })
      return
    }
    const ok = await ElMessageBox.confirm(text, t('自动勾对'), { confirmButtonText: dry.groups ? t('执行勾对') : t('清理'), dangerouslyUseHTMLString: false, customClass: 'pre-wrap-box' }).then(() => true, () => false)
    if (!ok)
      return
    busy.value = '正在勾对…'
    const r = await autoMatchReconApi(id.value, false)
    ElMessage.success(t('已自动勾对 {0} 组（{1} 笔）', [r.groups.toLocaleString(), r.matchedRows.toLocaleString()]))
    await refresh()
  }
  finally {
    busy.value = ''
  }
}

async function unmatchAll(mode: 'auto' | 'all') {
  const what = mode === 'auto' ? '全部自动勾对的结果' : '全部勾对（含手工勾对）'
  const ok = await ElMessageBox.confirm(t('取消{0}？这些行会回到未勾对，可以重新勾对。', [what]), t('取消勾对'), { type: 'warning', confirmButtonText: t('取消勾对'), confirmButtonClass: 'ec-button--danger' }).then(() => true, () => false)
  if (!ok)
    return
  busy.value = '正在取消…'
  try {
    const r = await unmatchReconApi(id.value, { mode })
    ElMessage.success(t('已取消 {0} 组', [r.groups.toLocaleString()]))
    await refresh()
  }
  finally {
    busy.value = ''
  }
}

const net = (s: ReconSideSummary) => Math.round((s.openIn.total + s.openOut.total) * 100) / 100
const g = computed(() => summary.value?.groups)
</script>

<template>
  <div class="h-full flex flex-col gap-3 overflow-hidden bg-[#F6F8F7] p-3 text-[13px]">
    <!-- 标题与操作 -->
    <div class="flex flex-wrap items-center gap-x-3 gap-y-2">
      <span class="text-base text-gray-800 font-semibold">{{ dataset.name }}</span>
      <span class="rounded bg-[#E7F4EC] px-1.5 py-0.5 text-xs text-[#0E5A31]">{{ $t('对账') }}</span>
      <span v-if="info" class="text-xs text-gray-500">
        {{ $t('{0} ⇄ {1}', [labels.left, labels.right]) }}&emsp;{{ $t('金额容差 {0}{1}{2}', [formatMoney(tolerance), info.def.days != null ? $t('，日期窗口 {0} 天', [info.def.days]) : '', info.def.groupByKey ? $t('，按键汇总') : '']) }}
      </span>
      <div class="ml-auto flex flex-wrap items-center gap-2">
        <el-date-picker v-model="asOf" type="date" value-format="YYYY-MM-DD" size="small" clearable :placeholder="$t('截至日期（空 = 全部）')" class="!w-44" :disabled-date="(d: Date) => d.getTime() > Date.now() + 366 * 864e5" />
        <el-button size="small" type="primary" :disabled="!info || !!busy" @click="autoMatch">
          {{ $t('自动勾对') }}
        </el-button>
        <el-dropdown trigger="click" :disabled="!info || !g?.total" @command="unmatchAll">
          <el-button size="small" :disabled="!info || !g?.total">
            {{ $t('取消勾对 ▾') }}
          </el-button>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item command="auto" :disabled="!g?.total || g.total === g.manual">
                {{ $t('取消全部自动勾对的') }}
              </el-dropdown-item>
              <el-dropdown-item command="all">
                <span class="text-[#B91C1C]">{{ $t('取消全部勾对（含手工）') }}</span>
              </el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
        <el-button size="small" :disabled="!info" :title="$t('修改数据集、列对应、容差与日期窗口')" @click="emit('edit', id)">
          {{ $t('对账设置') }}
        </el-button>
        <el-button size="small" :title="$t('结果数据集的全部明细：透视、图表、导出')" @click="emit('detail')">
          {{ $t('结果明细') }}
        </el-button>
      </div>
    </div>

    <div v-if="error" class="flex flex-1 flex-col items-center justify-center gap-2 text-sm text-[#B91C1C]">
      <span>{{ error }}</span>
      <el-button size="small" @click="init">
        {{ $t('重试') }}
      </el-button>
    </div>

    <template v-else-if="info">
      <!-- 汇总：两侧的未达账项与勾对情况 -->
      <div v-if="summary" class="grid grid-cols-[1fr_1fr_minmax(220px,0.8fr)] gap-3">
        <div v-for="(s, i) in [summary.left, summary.right]" :key="i" class="card">
          <div class="flex items-baseline gap-2">
            <b class="text-sm text-gray-800">{{ s.label }}</b>
            <span class="text-xs text-gray-500 tabular-nums">{{ $t('{0} 笔，合计 {1}', [s.all.rows.toLocaleString(), formatMoney(s.all.total)]) }}</span>
          </div>
          <div class="mt-1 text-xs text-gray-500">
            {{ $t('已勾对') }} <span class="tabular-nums">{{ s.matched.rows.toLocaleString() }}</span> {{ $t('笔') }}
            <template v-if="s.minDate">
              &emsp;{{ $t('日期 {0} ~ {1}', [s.minDate, s.maxDate]) }}
            </template>
          </div>
          <div class="mt-1.5 grid grid-cols-[auto_1fr_1fr] items-baseline gap-x-3 text-xs">
            <span class="text-gray-500">{{ $t('未达账项{0}', [summary.asOf ? $t('（截至 {0}）', [summary.asOf]) : '']) }}</span>
            <span class="text-right tabular-nums">{{ $t('增加 {0} 笔', [s.openIn.rows.toLocaleString()]) }}&emsp;<b>{{ formatMoney(s.openIn.total) }}</b></span>
            <span class="text-right tabular-nums">{{ $t('减少 {0} 笔', [s.openOut.rows.toLocaleString()]) }}&emsp;<b class="text-[#B91C1C]">{{ formatMoney(s.openOut.total) }}</b></span>
          </div>
          <div class="mt-0.5 text-right text-xs text-gray-500 tabular-nums">
            {{ $t('净额') }} <b :class="net(s) < 0 ? 'text-[#B91C1C]' : ''">{{ formatMoney(net(s)) }}</b>
            <span v-if="summary.asOf && s.undated" class="ml-2 text-[#B45309]" :title="$t('没有日期的行不在截至日范围内')">{{ $t('{0} 笔没有日期', [s.undated.toLocaleString()]) }}</span>
          </div>
        </div>
        <div class="card">
          <div class="flex items-baseline gap-2">
            <b class="text-sm text-gray-800">{{ $t('勾对') }}</b>
            <span class="text-xs text-gray-500 tabular-nums">{{ $t('{0} 组', [g?.total.toLocaleString()]) }}</span>
          </div>
          <div class="mt-1 text-xs text-gray-500 leading-5 tabular-nums">
            {{ $t('自动 {0}', [g?.exact]) }}&emsp;{{ $t('容差 {0}', [g?.tolerance]) }}&emsp;{{ $t('汇总 {0}', [g?.sum]) }}&emsp;{{ $t('手工 {0}', [g?.manual]) }}
          </div>
          <div v-if="g?.withDiff" class="mt-0.5 text-xs text-[#B45309] tabular-nums">
            {{ $t('{0} 组有差额，合计 {1}', [g.withDiff, formatMoney(g.diffTotal)]) }}
          </div>
          <div v-if="g?.broken" class="mt-1 cursor-pointer text-xs text-[#B91C1C]" @click="tab = 'matched'">
            {{ $t('⚠ {0} 组已失效（行被改过或删了），点「自动勾对」清理', [g.broken]) }}
          </div>
          <div class="mt-1 text-[11px] text-gray-400 leading-4">
            {{ $t('未达账项 = 截至日未勾对的 + 已勾对但对方日期晚于截至日的') }}
          </div>
        </div>
      </div>

      <el-tabs v-model="tab" class="recon-tabs min-h-0 flex flex-1 flex-col">
        <el-tab-pane name="open" :label="`未勾对${summary ? `（${labels.left} ${summary.left.all.rows - summary.left.matched.rows}&emsp;${labels.right} ${summary.right.all.rows - summary.right.matched.rows}）` : ''}`" class="h-full">
          <div class="h-full flex flex-col gap-2">
            <div class="grid grid-cols-2 min-h-0 flex-1 gap-3">
              <ReconItemPane
                v-model:amount="amountL"
                :recon-id="id"
                side="left"
                :label="labels.left"
                :selected="selL"
                :reload="reload"
                @toggle="(i: ReconItem, c: boolean) => toggle(selL, i, c)"
                @toggle-page="(items: ReconItem[], c: boolean) => togglePage(selL, items, c)"
              />
              <ReconItemPane
                v-model:amount="amountR"
                :recon-id="id"
                side="right"
                :label="labels.right"
                :selected="selR"
                :reload="reload"
                @toggle="(i: ReconItem, c: boolean) => toggle(selR, i, c)"
                @toggle-page="(items: ReconItem[], c: boolean) => togglePage(selR, items, c)"
              />
            </div>
            <!-- 勾对条：选了什么、差多少 -->
            <div class="flex flex-wrap items-center gap-x-5 gap-y-1 border border-[#e5e7eb] rounded-lg border-solid bg-white px-3 py-2">
              <span class="text-gray-600">{{ $t('{0} 已选', [labels.left]) }} <b class="tabular-nums">{{ sel.leftCount }}</b> {{ $t('笔') }}&emsp;<b class="tabular-nums">{{ formatMoney(sel.leftTotal) }}</b></span>
              <span class="text-gray-600">{{ $t('{0} 已选', [labels.right]) }} <b class="tabular-nums">{{ sel.rightCount }}</b> {{ $t('笔') }}&emsp;<b class="tabular-nums">{{ formatMoney(sel.rightTotal) }}</b></span>
              <span v-if="sel.ready" class="tabular-nums" :class="sel.balanced ? 'text-[#0B5A2E]' : 'text-[#B91C1C]'">
                {{ sel.balanced ? $t('✓ 合计相等') : $t('差额 {0}', [formatMoney(sel.diff)]) }}
              </span>
              <span v-else-if="sel.leftCount || sel.rightCount" class="text-xs text-gray-400">{{ $t('两边各选至少一笔才能勾对；只在一边的是未达账项，不用勾对') }}</span>
              <span v-else class="text-xs text-gray-400">{{ $t('点行选择；一笔可以对多笔') }}</span>
              <el-button v-if="canFind" link size="small" type="primary" @click="findCounterpart">
                {{ $t('找金额相同的') }}
              </el-button>
              <el-button v-if="sel.leftCount || sel.rightCount" link size="small" @click="selL.clear(); selR.clear()">
                {{ $t('清空选择') }}
              </el-button>
              <el-button class="ml-auto" size="small" type="primary" :disabled="!sel.ready || !!busy" :loading="busy === '正在勾对…'" @click="match">
                {{ $t('勾对{0}', [sel.ready && !sel.balanced ? $t('（有差额）') : '']) }}
              </el-button>
            </div>
          </div>
        </el-tab-pane>
        <el-tab-pane name="matched" :label="`已勾对${g ? `（${g.total.toLocaleString()} 组）` : ''}`" class="h-full">
          <ReconGroupList :recon-id="id" :labels="labels" :reload="reload" @changed="refresh" />
        </el-tab-pane>
      </el-tabs>
    </template>
    <div v-else class="flex flex-1 items-center justify-center text-gray-400">
      {{ $t('加载中…') }}
    </div>

    <div v-if="busy && busy !== '正在勾对…'" class="absolute inset-0 z-10 flex items-center justify-center bg-white/60 text-sm text-gray-600">
      <span class="i-svg-spinners-90-ring-with-bg mr-2 text-xl text-[#107C41]" />{{ busy }}
    </div>
  </div>
</template>

<style scoped lang="scss">
.card {
  @apply border border-[#e5e7eb] rounded-lg border-solid bg-white px-3 py-2;
}

.recon-tabs {
  :deep(.#{$namespace}-tabs__content) {
    @apply min-h-0 flex-1 overflow-hidden;
  }

  :deep(.#{$namespace}-tab-pane) {
    @apply h-full;
  }

  :deep(.#{$namespace}-tabs__header) {
    @apply mb-2;
  }
}
</style>

<style lang="scss">
.pre-wrap-box .#{$namespace}-message-box__message p {
  white-space: pre-wrap;
}
</style>
