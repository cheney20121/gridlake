<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { CellRef } from '@/features/excel/formula/circular'
import type { DiagnosticsReport, FormulaError, FormulaGroup } from '@/features/excel/formula/diagnostics'
import { computed, ref, watch } from 'vue'
import { datasetFormulaStats } from '@/features/excel/formula/dataset-functions'
import { analyzeFormulas, measureRecalc } from '@/features/excel/formula/diagnostics'
import { functionProblems } from '@/features/excel/formula/function-check'
import { importRangeStats } from '@/features/excel/formula/import-range'
import { cellName } from '@/features/excel/writeback/input-table'
import { t } from '@/i18n'

/**
 * 公式诊断：可能拖慢计算的公式（按引用格子数估算，同形公式合并）、出错的公式、服务端计算与跨工作簿引用的情况；
 * 可实测一次全量重算耗时。点地址定位到单元格
 */
const props = defineProps<{
  visible: boolean
  univerApi: FUniver | null
}>()

const emit = defineEmits<{
  (e: 'update:visible', v: boolean): void
  (e: 'locate', sheetId: string, row: number, col: number): void
}>()

const report = ref<DiagnosticsReport | null>(null)
const recalcMs = ref<number | null | undefined>(undefined)
const measuring = ref(false)
const tab = ref<'slow' | 'errors' | 'cycles' | 'server'>('slow')
const server = ref(datasetFormulaStats())
const imports = ref(importRangeStats())

function scan() {
  if (!props.univerApi)
    return
  report.value = analyzeFormulas(props.univerApi)
  server.value = datasetFormulaStats()
  imports.value = importRangeStats()
}

watch(() => props.visible, (v) => {
  if (v) {
    recalcMs.value = undefined
    scan()
    // 循环引用最该先看到：结果不可信；其次是出错
    if (report.value?.cycles.length)
      tab.value = 'cycles'
    else if (report.value?.errors.length && !report.value.groups.some(g => g.cost >= 100_000))
      tab.value = 'errors'
    else
      tab.value = 'slow'
  }
})

async function measure() {
  if (!props.univerApi)
    return
  measuring.value = true
  try {
    recalcMs.value = await measureRecalc(props.univerApi)
  }
  finally {
    measuring.value = false
  }
  scan()
}

/** 只列值得关注的：开销较大、整列、易失、服务端计算 */
const slow = computed(() => (report.value?.groups ?? []).filter(g => g.cost >= 10_000 || g.wholeColumn || g.volatile.length || g.advice.length).slice(0, 100))
const serverGroups = computed(() => (report.value?.groups ?? []).filter(g => g.server))
const errorKinds = computed(() => {
  const m = new Map<string, number>()
  for (const e of report.value?.errors ?? []) {
    const k = /^#[A-Z/0!?]+/.exec(e.value)?.[0] ?? '平台函数提示'
    m.set(k, (m.get(k) ?? 0) + 1)
  }
  return [...m.entries()]
})

const ERROR_HINT: Record<string, string> = {
  '#REF!': t('引用的格子已被删除'),
  '#NAME?': t('函数名或命名区域写错'),
  '#VALUE!': t('参数类型不对（如文字参与计算）'),
  '#DIV/0!': t('除数为 0 或空白'),
  '#N/A': t('查找不到匹配项'),
  '#SPILL!': t('溢出区域被挡住，清空下方 / 右侧的格子'),
  '#NUM!': t('数值无效'),
}

function hint(e: FormulaError) {
  // #NAME?：先看是不是用了不支持的函数（点名并给出原因 / 替代），其次才是拼错
  if (e.value.startsWith('#NAME?')) {
    const p = functionProblems(e.formula)
    if (p.unsupported.length)
      return t('不支持的函数 {0}：{1}', [p.unsupported[0]!.name, p.unsupported[0]!.reason])
    if (p.unknown.length)
      return t('不认识的函数 {0}（拼错了？）', [p.unknown.join('、')])
  }
  // 平台函数返回的「#说明」本身就是原因，不再重复
  const k = Object.keys(ERROR_HINT).find(x => e.value.startsWith(x))
  return k ? ERROR_HINT[k] : ''
}

function fmt(n: number) {
  if (n >= 100_000_000)
    return `${(n / 100_000_000).toFixed(1)} 亿`
  if (n >= 10_000)
    return `${(n / 10_000).toFixed(n >= 1_000_000 ? 0 : 1)} 万`
  return n.toLocaleString()
}

function locate(g: FormulaGroup | FormulaError) {
  emit('locate', g.sheetId, g.row, g.col)
}

/** 循环引用里的格子：按工作表名找到页签 id 再定位 */
function locateCell(c: CellRef) {
  const s = report.value?.sheets.find(x => x.name === c.sheet)
  if (s)
    emit('locate', s.sheetId, c.row, c.col)
}

function pathText(path: CellRef[], truncated: boolean) {
  const multi = new Set((report.value?.sheets ?? []).filter(s => s.formulas).map(s => s.name)).size > 1
  const items = path.map(c => `${multi ? `${c.sheet}!` : ''}${cellName(c.row, c.col)}`)
  return truncated ? `${items.join(' → ')} → …` : items.join(' → ')
}
</script>

<template>
  <el-dialog :model-value="visible" :title="$t('公式诊断')" width="820px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div v-if="report" class="flex flex-col gap-3 text-[13px]">
      <div class="grid grid-cols-5 gap-2">
        <div class="stat">
          <span class="stat-value">{{ report.formulas.toLocaleString() }}</span>
          <span class="stat-label">{{ $t('公式') }}</span>
        </div>
        <div class="stat">
          <span class="stat-value" :class="{ 'text-[#B91C1C]': report.errors.length }">{{ report.errors.length.toLocaleString() }}</span>
          <span class="stat-label">{{ $t('出错') }}</span>
        </div>
        <div class="stat">
          <span class="stat-value" :class="{ 'text-[#B45309]': report.volatileCount }">{{ report.volatileCount.toLocaleString() }}</span>
          <span class="stat-label">{{ $t('易失函数') }}</span>
        </div>
        <div class="stat">
          <span class="stat-value">{{ report.serverCount.toLocaleString() }}</span>
          <span class="stat-label">{{ $t('服务端计算') }}</span>
        </div>
        <div class="stat">
          <span v-if="recalcMs === undefined" class="stat-value">
            <el-button size="small" :loading="measuring" @click="measure">{{ $t('测量') }}</el-button>
          </span>
          <span v-else class="stat-value">{{ recalcMs == null ? $t('超时') : recalcMs >= 1000 ? $t('{0} 秒', [(recalcMs / 1000).toFixed(1)]) : $t('{0} 毫秒', [recalcMs]) }}</span>
          <span class="stat-label">{{ $t('全量重算耗时') }}</span>
        </div>
      </div>
      <div class="text-xs text-gray-400">
        {{ $t('各工作表：{0}。开销按每个公式引用的格子数估算，向下复制的同形公式合并为一组', [report.sheets.filter(s => s.formulas).map(s => `${s.name} ${s.formulas.toLocaleString()}`).join('、') || $t('没有公式')]) }}
      </div>

      <el-radio-group v-model="tab" size="small">
        <el-radio-button value="slow">
          {{ $t('可能拖慢计算（{0}）', [slow.length]) }}
        </el-radio-button>
        <el-radio-button value="errors">
          {{ $t('出错的公式（{0}）', [report.errors.length]) }}
        </el-radio-button>
        <el-radio-button value="cycles">
          {{ $t('循环引用（{0}）', [report.cycles.length]) }}
        </el-radio-button>
        <el-radio-button value="server">
          {{ $t('服务端与跨工作簿（{0}）', [serverGroups.length + imports.length]) }}
        </el-radio-button>
      </el-radio-group>

      <div class="max-h-[52vh] overflow-y-auto">
        <template v-if="tab === 'slow'">
          <div v-if="!slow.length" class="empty">
            {{ $t('没有发现明显拖慢计算的公式') }}
          </div>
          <div v-for="(g, i) in slow" :key="i" class="item">
            <div class="flex items-center gap-2">
              <button type="button" class="addr" :title="$t('定位到 {0}!{1}', [g.sheetName, g.where])" @click="locate(g)">
                {{ g.sheetName }}!{{ g.where }}
              </button>
              <span v-if="g.count > 1" class="text-xs text-gray-500">{{ $t('{0} 个同形公式', [g.count.toLocaleString()]) }}</span>
              <span v-if="g.wholeColumn" class="tag warn">{{ $t('整列引用') }}</span>
              <span v-for="v in g.volatile" :key="v" class="tag warn">{{ v }}</span>
              <span v-if="g.server" class="tag">{{ $t('服务端') }}</span>
              <span class="ml-auto text-xs text-gray-500 tabular-nums" :title="$t('单个引用 {0} 格 × {1} 个', [g.cellsEach.toLocaleString(), g.count])">{{ $t('约 {0} 格', [fmt(g.cost)]) }}</span>
            </div>
            <code class="formula">{{ g.formula }}</code>
            <div v-for="(a, ai) in g.advice" :key="ai" class="text-xs text-[#92400E]">
              · {{ a }}
            </div>
          </div>
        </template>

        <template v-else-if="tab === 'errors'">
          <div v-if="errorKinds.length" class="mb-2 flex flex-wrap gap-1.5">
            <span v-for="[k, n] in errorKinds" :key="k" class="tag bad">{{ k }} × {{ n }}</span>
          </div>
          <div v-if="!report.errors.length" class="empty">
            {{ $t('没有出错的公式') }}
          </div>
          <div v-for="(e, i) in report.errors.slice(0, 300)" :key="i" class="item">
            <div class="flex items-center gap-2">
              <button type="button" class="addr" @click="locate(e)">
                {{ e.sheetName }}!{{ e.address }}
              </button>
              <span class="text-xs text-[#B91C1C]">{{ e.value.length > 40 ? `${e.value.slice(0, 40)}…` : e.value }}</span>
              <span class="ml-auto text-xs text-gray-500">{{ hint(e) }}</span>
            </div>
            <code class="formula">{{ e.formula }}</code>
          </div>
          <div v-if="report.errors.length > 300" class="empty">
            {{ $t('只列出前 300 个') }}
          </div>
        </template>

        <template v-else-if="tab === 'cycles'">
          <div v-if="!report.cycles.length" class="empty">
            {{ $t('没有发现循环引用{0}', [report.cyclesPartial ? $t('（公式之间的引用太多，只检查了一部分）') : '']) }}
          </div>
          <div v-else class="mb-2 rounded bg-[#FDE2E2] p-2 text-xs text-[#B91C1C] leading-5">
            {{ $t('循环引用的格子互相依赖，算不出确定的结果，相关的数字都不可信。沿着下面的路径找到该断开的那个引用，改掉后重新扫描。{0}', [report.cyclesPartial ? $t('公式之间的引用太多，只检查了一部分。') : '']) }}
          </div>
          <div v-for="(c, i) in report.cycles" :key="i" class="item">
            <div class="flex flex-wrap items-center gap-1 text-[13px]">
              <template v-for="(p, pi) in c.path" :key="pi">
                <span v-if="pi" class="text-gray-400">→</span>
                <button type="button" class="addr" :title="$t('定位到 {0}!{1}', [p.sheet, cellName(p.row, p.col)])" @click="locateCell(p)">
                  {{ pathText([p], false) }}
                </button>
              </template>
              <span v-if="c.truncated" class="text-gray-400">→ …</span>
              <span class="ml-auto text-xs text-gray-500">{{ c.size > c.path.length - 1 ? $t('这一组共 {0} 个格子互相牵连', [c.size]) : $t('{0} 个格子', [c.size]) }}</span>
            </div>
          </div>
        </template>

        <template v-else>
          <div class="mb-2 rounded bg-[#F7F7F5] p-2 text-xs text-gray-600 leading-5">
            {{ $t('数据集 / 数据表函数在服务端计算：本次打开以来请求 {0} 次、算了 {1} 个，累计 {2} 秒', [server.requests, server.calls.toLocaleString(), (server.totalMs / 1000).toFixed(1)]) }}<template v-if="server.lastAt">
              {{ $t('，最近一次 {0} 毫秒（{1}）', [server.lastMs, server.lastAt]) }}
            </template>{{ $t('。 跨工作簿引用取的是对方已保存的最新版本，对方保存后一分钟内自动更新，也可在命令面板执行「刷新跨工作簿引用」') }}
          </div>
          <div v-for="(r, i) in imports" :key="`i${i}`" class="item">
            <div class="flex items-center gap-2">
              <span class="tag">IMPORTRANGE</span>
              <span>{{ r.workbook }} · {{ r.range }}</span>
              <span class="ml-auto text-xs" :class="r.error ? 'text-[#B91C1C]' : 'text-gray-500'">{{ r.error ?? $t('{0} 格', [r.cells.toLocaleString()]) }}</span>
            </div>
          </div>
          <div v-for="(g, i) in serverGroups" :key="`s${i}`" class="item">
            <div class="flex items-center gap-2">
              <button type="button" class="addr" @click="locate(g)">
                {{ g.sheetName }}!{{ g.where }}
              </button>
              <span v-if="g.count > 1" class="text-xs text-gray-500">{{ $t('{0} 个', [g.count.toLocaleString()]) }}</span>
            </div>
            <code class="formula">{{ g.formula }}</code>
          </div>
          <div v-if="!serverGroups.length && !imports.length" class="empty">
            {{ $t('没有服务端计算的公式和跨工作簿引用') }}
          </div>
        </template>
      </div>
    </div>
    <template #footer>
      <span class="mr-auto text-xs text-gray-400">{{ report ? $t('扫描用时 {0} 毫秒', [report.elapsedMs]) : '' }}</span>
      <el-button size="small" @click="scan">
        {{ $t('重新扫描') }}
      </el-button>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.stat {
  @apply flex flex-col items-center gap-0.5 rounded-md bg-[#F7F8F7] py-2;
}

.stat-value {
  @apply text-lg font-semibold tabular-nums;
}

.stat-label {
  @apply text-xs text-gray-500;
}

.item {
  @apply flex flex-col gap-1 border-b border-[#f0f2f5] border-b-solid py-2;
}

.addr {
  @apply cursor-pointer border-none bg-transparent p-0 text-[13px] font-medium text-[#0E5A31] hover:underline;
}

.formula {
  @apply block truncate rounded bg-[#f4f4f1] px-1.5 py-0.5 text-xs text-gray-700 font-mono;
}

.tag {
  @apply shrink-0 rounded bg-[#E7F4EC] px-1.5 text-[11px] text-[#217346];

  &.warn {
    @apply bg-[#FEF3C7] text-[#92400E];
  }

  &.bad {
    @apply bg-[#FDE2E2] text-[#B91C1C];
  }
}

.empty {
  @apply py-8 text-center text-xs text-gray-400;
}
</style>
