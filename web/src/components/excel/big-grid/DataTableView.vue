<script setup lang="ts">
import type { DatasetDrill } from './drill'
import type { GridColumn, GridSelection } from './types'
import type { CellValue, DatasetCursor, DatasetFilter, DatasetInfo, DatasetQuery, SelectionStats } from '@/api/dataset'
import { useDebounceFn, useLocalStorage } from '@vueuse/core'
import { isCancel } from 'axios'
import { computed, nextTick, ref, shallowRef, watch } from 'vue'
import { CONTRIBUTION_KEY, exportUrl, getStatsApi, MAX_EXPORT_ROWS, MAX_XLSX_ROWS, openCursorApi } from '@/api/dataset'
import BusyOverlay from '@/components/excel/common/BusyOverlay.vue'
import { formatMoney } from '@/components/excel/ledger-detail/columns'
import { t } from '@/i18n'
import { errorText } from '@/utils/busy-hint'
import BigGrid from './BigGrid.vue'
import FilterMenu from './FilterMenu.vue'

/** 大数据表视图：工具栏（搜索 / 筛选标签 / 下钻核对）+ canvas 表格 + 状态栏（选区统计） */
const props = defineProps<{
  dataset: DatasetInfo
  /** 下钻上下文；null 表示浏览全部数据 */
  drill: DatasetDrill | null
}>()

const emit = defineEmits<{
  (e: 'exitDrill'): void
  /** 以当前筛选新建透视表 */
  (e: 'pivot', datasetId: string, filters: DatasetFilter[]): void
  /** 以当前筛选新建图表（服务端汇总） */
  (e: 'chart', datasetId: string, filters: DatasetFilter[]): void
  (e: 'calc', datasetId: string): void
  (e: 'join', datasetId: string): void
  /** 修改关联视图的定义 */
  (e: 'editJoin', datasetId: string): void
  /** 对账结果：回到对账工作台 */
  (e: 'backToRecon'): void
  /** 以当前筛选 / 排序 / 搜索放入工作表（数据表） */
  (e: 'embed', datasetId: string, query: DatasetQuery): void
}>()

const cursor = shallowRef<DatasetCursor | null>(null)

// ── 列 ──
const widths = useLocalStorage<Record<string, number>>(`big-grid-widths:${props.dataset.id}`, {})
const columns = computed<GridColumn[]>(() => {
  // 用户计算列在表头加 ƒ 标记
  const cols: GridColumn[] = props.dataset.columns.map(c => ({ ...c, title: c.formula ? `${c.title} ƒ` : c.title, width: widths.value[c.key] ?? c.width }))
  if (cursor.value?.hasContribution)
    cols.unshift({ key: CONTRIBUTION_KEY, title: t('贡献金额'), kind: 'money', width: widths.value[CONTRIBUTION_KEY] ?? 120, emphasis: true })
  return cols
})

function onResize(key: string, w: number) {
  widths.value = { ...widths.value, [key]: w }
}

// ── 查询条件 ──
const keyword = ref('')
const userFilters = ref<DatasetFilter[]>([])
const sort = ref<{ column: string, desc: boolean } | null>(null)
const useRule = ref(true)

const query = computed<DatasetQuery>(() => ({
  filters: [...(props.drill?.filters ?? []), ...userFilters.value],
  keyword: keyword.value.trim() || undefined,
  sort: sort.value ? [sort.value] : [],
  rule: useRule.value ? props.drill?.rule : undefined,
}))

const filteredColumns = computed(() => new Set(userFilters.value.map(f => f.column)))

// ── 游标 ──
const loading = ref(false)
const error = ref('')
let controller: AbortController | null = null

/** 上一次成功的查询条件：取消 / 失败后还原到它，界面上的筛选标签、排序箭头才和表格里的数据一致 */
let lastGood: { key: string, filters: DatasetFilter[], keyword: string, sort: { column: string, desc: boolean } | null, useRule: boolean } | null = null
const queryKey = () => JSON.stringify(query.value)

/** force：条件没变也重新取（下钻切换、列定义变化、游标失效）；否则条件与当前游标相同就不重复请求 */
async function openCursor(force = false) {
  if (!force && cursor.value && lastGood?.key === queryKey()) {
    controller?.abort()
    loading.value = false
    error.value = ''
    return
  }
  controller?.abort()
  const c = new AbortController()
  controller = c
  const key = queryKey()
  const snapshot = { filters: JSON.parse(JSON.stringify(userFilters.value)), keyword: keyword.value, sort: sort.value ? { ...sort.value } : null, useRule: useRule.value }
  loading.value = true
  error.value = ''
  try {
    const result = await openCursorApi(props.dataset.id, query.value, c.signal, true)
    if (c === controller) {
      cursor.value = result
      lastGood = { key, ...snapshot }
    }
  }
  catch (e) {
    if (!isCancel(e) && c === controller) {
      error.value = errorText(e, '查询失败，请重试')
      console.warn('[big-grid] 建游标失败:', e)
    }
  }
  finally {
    if (c === controller)
      loading.value = false
  }
}
const debouncedOpen = useDebounceFn(() => openCursor(), 300)

/** 取消正在进行的查询，条件回到上一次成功的状态（表格里一直是那次的数据） */
function cancelQuery() {
  controller?.abort()
  loading.value = false
  if (lastGood)
    revertQuery()
  else
    error.value = '已取消'
}

function revertQuery() {
  const g = lastGood
  if (!g)
    return
  error.value = ''
  userFilters.value = g.filters
  keyword.value = g.keyword
  sort.value = g.sort
  useRule.value = g.useRule
}

watch(() => props.drill, () => {
  userFilters.value = []
  keyword.value = ''
  sort.value = null
  useRule.value = true
  openCursor(true)
}, { immediate: true })
watch(() => [userFilters.value, sort.value, useRule.value], () => openCursor(), { deep: true })
watch(keyword, debouncedOpen)
// 新增 / 修改 / 删除计算列、修改关联定义后：重新建游标取数（列名不变但值变了也要刷新）
watch(() => JSON.stringify([props.dataset.columns.map(c => [c.key, c.kind, c.formula]), props.dataset.view]), (now, before) => {
  if (before !== undefined && now !== before)
    openCursor(true)
})

// 关闭规则后贡献列消失，按它排序会被后端拒绝
watch(useRule, (on) => {
  if (!on && sort.value?.column === CONTRIBUTION_KEY)
    sort.value = null
})

// ── 列头菜单 ──
const menu = shallowRef<{ column: GridColumn, rect: DOMRect } | null>(null)
const menuQuery = computed<DatasetQuery>(() => ({ ...query.value, sort: [] }))

function onApplyFilter(filter: DatasetFilter | null) {
  const col = menu.value!.column.key
  userFilters.value = [...userFilters.value.filter(f => f.column !== col), ...(filter ? [filter] : [])]
  menu.value = null
}

function onSort(desc: boolean | null) {
  const col = menu.value!.column.key
  sort.value = desc == null ? null : { column: col, desc }
  menu.value = null
}

function filterLabel(f: DatasetFilter): string {
  const title = columns.value.find(c => c.key === f.column)?.title ?? f.column
  switch (f.op) {
    case 'in': {
      const shown = f.values.slice(0, 2).map(v => v || '（空白）').join('、')
      return `${title}：${shown}${f.values.length > 2 ? ` 等 ${f.values.length} 项` : ''}`
    }
    case 'between': return `${title}：${f.min ?? '…'} ~ ${f.max ?? '…'}`
    case 'contains': return t('{0} 包含「{1}」', [title, f.value])
    case 'prefix': return `${title} 以「${f.value || '（空白）'}」开头`
    case 'prefixes': return `${title} 以「${f.values.map(v => v || '（空白）').join('」「')}」之一开头`
  }
}

function removeFilter(i: number) {
  userFilters.value = userFilters.value.filter((_, idx) => idx !== i)
}

// ── 核对 ──
/** 透视表求和格子：取下钻结果中该列的合计 */
const columnSum = ref<number | null>(null)
watch(cursor, async (cur) => {
  columnSum.value = null
  const exp = props.drill?.expected
  if (!cur || exp?.kind !== 'columnSum' || !exp.column)
    return
  if (!cur.total) {
    columnSum.value = 0
    return
  }
  const key = props.dataset.columns.find(c => c.title === exp.column || c.key === exp.column)?.key ?? exp.column
  const stats = await getStatsApi(props.dataset.id, { cursorId: cur.cursorId, from: 0, to: cur.total - 1, columns: [key] }).catch(() => null)
  if (cursor.value === cur && stats && !stats.expired)
    columnSum.value = stats.sum ?? 0
})

const check = computed(() => {
  const exp = props.drill?.expected
  const cur = cursor.value
  if (!exp || !cur || (exp.kind === 'contribution' && !cur.hasContribution))
    return null
  if (exp.kind === 'columnSum' && columnSum.value == null)
    return null
  const actual = exp.kind === 'contribution' ? (cur.contribution ?? 0) : exp.kind === 'columnSum' ? columnSum.value! : cur.total
  const fmt = (v: number) => exp.kind === 'count' ? `${v.toLocaleString()} 笔` : formatMoney(v)
  return {
    label: exp.label,
    expected: fmt(exp.value),
    actual: fmt(actual),
    ok: Math.abs(actual - exp.value) < 0.005,
    extra: userFilters.value.length > 0 || !!keyword.value.trim(),
  }
})

// ── 选区统计 ──
const selection = shallowRef<GridSelection | null>(null)
const stats = shallowRef<SelectionStats | null>(null)
let statsController: AbortController | null = null

const selectionSize = computed(() => {
  const s = selection.value
  if (!s)
    return null
  return {
    rows: Math.abs(s.focus.row - s.anchor.row) + 1,
    cols: Math.abs(s.focus.col - s.anchor.col) + 1,
  }
})

const loadStats = useDebounceFn(async () => {
  const s = selection.value
  const cur = cursor.value
  statsController?.abort()
  if (!s || !cur || (selectionSize.value!.rows === 1 && selectionSize.value!.cols === 1)) {
    stats.value = null
    return
  }
  const c = new AbortController()
  statsController = c
  const c1 = Math.min(s.anchor.col, s.focus.col)
  const c2 = Math.max(s.anchor.col, s.focus.col)
  try {
    const result = await getStatsApi(props.dataset.id, {
      cursorId: cur.cursorId,
      from: Math.min(s.anchor.row, s.focus.row),
      to: Math.max(s.anchor.row, s.focus.row),
      columns: columns.value.slice(c1, c2 + 1).map(col => col.key),
    }, c.signal)
    if (c === statsController)
      stats.value = result.expired ? null : result
  }
  catch {
    // 取消或失败时不显示统计
  }
}, 200)

function onSelection(s: GridSelection | null) {
  selection.value = s
  stats.value = null
  loadStats()
}

// ── 行详情 ──
const openedRow = shallowRef<{ row: number, values: CellValue[] } | null>(null)
const rowFields = computed(() => {
  const r = openedRow.value
  if (!r)
    return []
  return columns.value
    .map((c, i) => ({ col: c, value: r.values[i] ?? null }))
    .filter(f => f.col.kind !== 'money' || f.col.key === '动账金额' || f.col.key === CONTRIBUTION_KEY || f.value)
})

function display(v: CellValue, kind: string) {
  if (v == null || v === '')
    return '—'
  return kind === 'money' && typeof v === 'number' ? formatMoney(v) : String(v)
}

const gridRef = ref<InstanceType<typeof BigGrid>>()

// ── 跳转到行 ──
const gotoOpen = ref(false)
const gotoValue = ref('')
const gotoBad = ref(false)
const gotoInput = ref<HTMLInputElement>()

async function openGoto() {
  if (!cursor.value?.total)
    return
  gotoValue.value = ''
  gotoBad.value = false
  gotoOpen.value = true
  await nextTick()
  gotoInput.value?.focus()
}

function onMore(cmd: 'calc' | 'join' | 'copy' | 'goto') {
  if (cmd === 'calc')
    emit('calc', props.dataset.id)
  else if (cmd === 'join')
    emit('join', props.dataset.id)
  else if (cmd === 'copy')
    gridRef.value?.copySelection()
  else
    openGoto()
}

function doGoto() {
  const total = cursor.value?.total ?? 0
  const n = Number(gotoValue.value.replace(/[,，\s]/g, ''))
  if (!Number.isInteger(n) || n < 1 || n > total) {
    gotoBad.value = true
    return
  }
  gotoOpen.value = false
  gridRef.value?.goToRow(n - 1)
}

// ── 导出 ──
/** 当前筛选的文字描述（写入 Excel 汇总页签） */
const queryDesc = computed(() => [
  props.drill ? `下钻：${props.drill.title}` : '',
  ...userFilters.value.map(filterLabel),
  keyword.value.trim() ? `关键字「${keyword.value.trim()}」` : '',
  sort.value ? `按「${columns.value.find(c => c.key === sort.value!.column)?.title ?? sort.value.column}」${sort.value.desc ? '降序' : '升序'}` : '',
].filter(Boolean).join('；'))

function exportData(format: 'csv' | 'xlsx') {
  const cur = cursor.value
  if (!cur)
    return
  const limit = format === 'xlsx' ? MAX_XLSX_ROWS : MAX_EXPORT_ROWS
  if (cur.total > limit) {
    ElMessage.warning(format === 'xlsx'
      ? t('当前结果 {0} 行，超过 Excel 单表上限 {1} 行，请先筛选，或改用 CSV（上限 {2} 行）', [cur.total.toLocaleString(), limit.toLocaleString(), MAX_EXPORT_ROWS.toLocaleString()])
      : t('当前结果 {0} 行，单次最多导出 {1} 行，请先筛选缩小范围', [cur.total.toLocaleString(), limit.toLocaleString()]))
    return
  }
  const name = props.drill ? `${props.dataset.name}_${props.drill.title.replace(/[\\/:*?"<>|\s]+/g, '_')}` : props.dataset.name
  const a = document.createElement('a')
  a.href = exportUrl(props.dataset.id, cur.cursorId, columns.value.map(c => c.key), name, format, queryDesc.value)
  a.click()
  // 浏览器要等服务端开始返回才会出现下载条：行数多时先说清楚在等什么，免得用户以为没反应又点一次
  const big = cur.total > (format === 'xlsx' ? 100_000 : 1_000_000)
  ElMessage({
    type: big ? 'info' : 'success',
    message: big
      ? `正在生成 ${cur.total.toLocaleString()} 行的${format === 'xlsx' ? ' Excel 文件' : ' CSV'}，需要几十秒到几分钟；生成完成后浏览器会自动开始下载，请不要重复点击`
      : `开始导出 ${cur.total.toLocaleString()} 行${format === 'xlsx' ? '（含「汇总」页签）' : ''}`,
    duration: big ? 10_000 : 3000,
    showClose: big,
  })
}
</script>

<template>
  <div class="h-full flex flex-col bg-white">
    <!-- 工具栏：标题与行数不换行；搜索框与筛选标签可收缩；窄屏时按钮换到下一行，低频操作收进「更多」 -->
    <div class="toolbar">
      <el-button v-if="dataset.kind === 'recon'" size="small" type="primary" plain :title="$t('回到对账工作台：勾对、看未达账项')" @click="emit('backToRecon')">
        {{ $t('← 对账工作台') }}
      </el-button>
      <span class="shrink-0 whitespace-nowrap text-sm text-gray-800 font-semibold">{{ dataset.name }}</span>
      <el-tooltip v-if="dataset.kind === 'view' && dataset.view" placement="bottom" :content="`关联视图：按 ${dataset.view.on.map(k => k.left === k.right ? k.left : `${k.left}=${k.right}`).join(' + ')} 带入 ${dataset.view.columns.map(c => c.title).join('、')}（点击修改）`">
        <el-tag size="small" type="info" class="cursor-pointer" @click="emit('editJoin', dataset.id)">
          {{ $t('关联 ✎') }}
        </el-tag>
      </el-tooltip>
      <span class="shrink-0 whitespace-nowrap text-xs text-gray-500 tabular-nums">
        {{ $t('{0} 行', [cursor ? cursor.total.toLocaleString() : '…']) }}
        <template v-if="cursor && cursor.total !== dataset.rowCount"> {{ $t('/ 共 {0}', [dataset.rowCount.toLocaleString()]) }}</template>
      </span>
      <el-input v-if="dataset.searchable" v-model="keyword" size="small" clearable :placeholder="$t('搜索订单号 / 流水号 / 商品 / 达人 / 备注')" class="min-w-44 max-w-72 flex-1" />
      <div class="min-w-0 flex flex-1 flex-wrap items-center gap-1.5 overflow-hidden">
        <el-tag v-for="(f, i) in userFilters" :key="i" size="small" closable @close="removeFilter(i)">
          {{ filterLabel(f) }}
        </el-tag>
        <el-button v-if="userFilters.length > 1" link size="small" @click="userFilters = []">
          {{ $t('清除全部筛选') }}
        </el-button>
      </div>
      <div class="ml-auto flex shrink-0 items-center gap-1.5">
        <el-button size="small" :title="$t('以当前筛选新建数据透视表')" @click="emit('pivot', dataset.id, query.filters ?? [])">
          {{ $t('透视分析') }}
        </el-button>
        <el-button size="small" :title="$t('以当前筛选新建图表（千万行在服务端汇总）')" @click="emit('chart', dataset.id, query.filters ?? [])">
          {{ $t('图表分析') }}
        </el-button>
        <el-button size="small" :title="$t('把当前筛选的明细放进工作表的选中单元格处（内部滚动，公式可按表名引用）')" @click="emit('embed', dataset.id, { filters: query.filters, keyword: query.keyword, sort: query.sort })">
          {{ $t('放入工作表') }}
        </el-button>
        <el-dropdown trigger="click" @command="onMore">
          <el-button size="small">
            {{ $t('更多 ▾') }}
          </el-button>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item command="calc">
                {{ $t('计算列…（用公式新增一列）') }}
              </el-dropdown-item>
              <el-dropdown-item command="join">
                {{ $t('关联…（按键带入另一个数据集的列）') }}
              </el-dropdown-item>
              <el-dropdown-item command="copy" divided>
                {{ $t('复制选区（Ctrl + C）') }}
              </el-dropdown-item>
              <el-dropdown-item command="goto">
                {{ $t('跳转到行（Ctrl + G）') }}
              </el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
        <el-dropdown trigger="click" :disabled="!cursor?.total" @command="exportData">
          <el-button size="small" :disabled="!cursor?.total">
            {{ $t('导出 ▾') }}
          </el-button>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item command="xlsx">
                {{ $t('Excel（.xlsx，含汇总页签）') }}
              </el-dropdown-item>
              <el-dropdown-item command="csv">
                {{ $t('CSV（最多 100 万行）') }}
              </el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
      </div>
    </div>

    <!-- 下钻上下文与核对 -->
    <div v-if="drill" class="drill-bar">
      <el-tag type="success" effect="light" round>
        {{ $t('下钻：{0}', [drill.title]) }}
      </el-tag>
      <el-switch v-if="drill.rule" v-model="useRule" size="small" :active-text="$t('只看有贡献的流水')" />
      <span v-if="check" class="check" :class="check.extra ? 'muted' : check.ok ? 'ok' : 'bad'">
        {{ check.extra ? '' : check.ok ? '✓' : '✕' }} {{ check.label }} {{ check.expected }}<span class="ml-3">{{ $t('明细合计') }}</span> {{ check.actual }}
        <span v-if="check.extra" class="text-gray-500">{{ $t('（已叠加筛选，对比仅供参考）') }}</span>
      </span>
      <el-button link type="primary" size="small" class="ml-auto" @click="emit('exitDrill')">
        {{ $t('退出下钻，查看全部数据') }}
      </el-button>
    </div>

    <div class="relative min-h-0 flex-1">
      <BigGrid
        v-if="cursor"
        ref="gridRef"
        :key="dataset.id"
        :dataset-id="dataset.id"
        :columns="columns"
        :cursor-id="cursor.cursorId"
        :total="cursor.total"
        :sort="sort"
        :filtered="filteredColumns"
        @selection-change="onSelection"
        @header-menu="(column, rect) => menu = { column, rect }"
        @resize-column="onResize"
        @row-open="(row, values) => openedRow = { row, values }"
        @expired="openCursor(true)"
        @goto-request="openGoto"
      />
      <BusyOverlay
        :loading="loading"
        :error="error"
        :title="$t('正在查询 {0} 行数据', [dataset.rowCount.toLocaleString()])"
        :cancellable="!!cursor"
        :revertable="!!cursor"
        @cancel="cancelQuery"
        @retry="openCursor(true)"
        @revert="revertQuery"
      />
      <el-empty v-if="cursor && !cursor.total && !loading" class="absolute inset-x-0 top-20" :description="$t('没有符合条件的数据')" :image-size="80" />
    </div>

    <!-- 状态栏：与 Excel 一致，选中多个单元格时显示统计 -->
    <div class="statusbar">
      <template v-if="selectionSize && (selectionSize.rows > 1 || selectionSize.cols > 1)">
        <span>{{ $t('已选 {0} 行 × {1} 列', [selectionSize.rows.toLocaleString(), selectionSize.cols]) }}</span>
        <template v-if="stats">
          <span>{{ $t('计数 {0}', [stats.count.toLocaleString()]) }}</span>
          <template v-if="stats.numericCount">
            <span>{{ $t('求和 {0}', [formatMoney(stats.sum)]) }}</span>
            <span>{{ $t('平均 {0}', [formatMoney(stats.avg)]) }}</span>
            <span>{{ $t('最小 {0}', [formatMoney(stats.min)]) }}</span>
            <span>{{ $t('最大 {0}', [formatMoney(stats.max)]) }}</span>
          </template>
        </template>
        <span v-else class="text-gray-400">{{ $t('统计中…') }}</span>
      </template>
      <span v-else class="min-w-0 truncate text-gray-400">{{ $t('双击行查看详情 · 列头按钮排序 / 筛选 · Ctrl+C 复制选区 · Ctrl+G 跳转到行') }}</span>
      <span v-if="cursor?.total" class="ml-auto flex shrink-0 items-center gap-1 whitespace-nowrap text-gray-500">
        <template v-if="gotoOpen">
          {{ $t('第') }}
          <input
            ref="gotoInput"
            v-model="gotoValue"
            class="goto-input"
            :class="{ bad: gotoBad }"
            inputmode="numeric"
            :placeholder="`1 ~ ${cursor.total.toLocaleString()}`"
            @input="gotoBad = false"
            @keydown.enter.prevent="doGoto"
            @keydown.esc.prevent="gotoOpen = false"
            @blur="gotoOpen = false"
          >
          {{ $t('行') }}
        </template>
        <button v-else class="goto-btn" :title="$t('跳转到指定行（Ctrl / ⌘ + G）')" @click="openGoto">{{ $t('跳转到行') }}</button>
      </span>
      <span v-if="cursor" class="shrink-0 whitespace-nowrap text-gray-400">
        {{ cursor.cached ? $t('查询结果已缓存') : $t('查询耗时 {0} ms', [cursor.elapsedMs.toLocaleString()]) }}
      </span>
    </div>

    <FilterMenu
      v-if="menu"
      :dataset-id="dataset.id"
      :column="menu.column"
      :query="menuQuery"
      :current="userFilters.find(f => f.column === menu!.column.key) ?? null"
      :sort-desc="sort?.column === menu.column.key ? sort.desc : null"
      :anchor="menu.rect"
      @apply="onApplyFilter"
      @sort="onSort"
      @close="menu = null"
    />

    <el-drawer :model-value="!!openedRow" :title="$t('第 {0} 行', [(openedRow?.row ?? 0) + 1])" size="420px" append-to-body @update:model-value="openedRow = null">
      <div class="flex flex-col text-[13px]">
        <div v-for="f in rowFields" :key="f.col.key" class="flex gap-3 border-b border-[#f2f2f2] border-b-solid py-1.5">
          <span class="w-28 shrink-0 text-gray-500">{{ f.col.title }}</span>
          <span class="min-w-0 flex-1 break-all" :class="{ 'tabular-nums text-right': f.col.kind === 'money' }">{{ display(f.value, f.col.kind) }}</span>
        </div>
        <div class="mt-2 text-xs text-gray-400">
          {{ $t('金额列只显示非零项') }}
        </div>
      </div>
    </el-drawer>
  </div>
</template>

<style scoped lang="scss">
.toolbar {
  @apply flex flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-[#e6e6e6] border-b-solid px-3 py-1.5;
}

.drill-bar {
  @apply flex items-center gap-3 border-b border-[#e6e6e6] border-b-solid bg-[#F7FBF8] px-3 py-1.5 text-[13px];
}

.check {
  @apply rounded px-2 py-0.5 text-xs font-medium;

  &.ok {
    @apply bg-[#E7F4EC] text-[#0B5A2E];
  }

  &.bad {
    @apply bg-[#FEE2E2] text-[#B91C1C];
  }

  &.muted {
    @apply bg-gray-100 text-gray-600;
  }
}

.goto-btn {
  @apply cursor-pointer rounded border-none bg-transparent px-1.5 py-0.5 text-xs text-gray-500 hover:bg-gray-200 hover:text-gray-800;
}

.goto-input {
  @apply h-5 w-28 rounded border border-[#d1d5db] border-solid px-1.5 text-xs tabular-nums outline-none focus:border-[#217346];

  &.bad {
    @apply border-[#B91C1C] bg-[#FEF2F2];
  }
}

.statusbar {
  @apply flex h-7 items-center gap-4 border-t border-[#e6e6e6] border-t-solid bg-[#F7F7F7] px-3 text-xs text-gray-700 tabular-nums;
}
</style>
