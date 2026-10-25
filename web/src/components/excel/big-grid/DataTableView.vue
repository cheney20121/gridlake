<script setup lang="ts">
import type { DatasetDrill } from './drill'
import type { GridColumn, GridFooter, GridGroups, GridSelection } from './types'
import type { CellValue, DatasetColumnKind, DatasetCursor, DatasetFilter, DatasetInfo, DatasetQuery, ExportGroup, ExternalRefreshOutcome, SelectionStats, TotalFunc, ValidationRule } from '@/api/dataset'
import { onClickOutside, useDebounceFn, useLocalStorage } from '@vueuse/core'
import { isCancel } from 'axios'
import { computed, nextTick, onBeforeUnmount, ref, shallowRef, watch } from 'vue'
import { CONTRIBUTION_KEY, exportUrl, getStatsApi, groupsApi, MAX_EXPORT_ROWS, MAX_XLSX_ROWS, openCursorApi, refreshExternalApi, totalsApi } from '@/api/dataset'
import BusyOverlay from '@/components/excel/common/BusyOverlay.vue'
import ExternalDatasetDialog from '@/components/excel/dataset-import/ExternalDatasetDialog.vue'
import { clearDatasetView, publishDatasetView } from '@/features/excel/dataset/active-view'
import { formatBool } from '@/features/excel/dataset/column-kinds'
import { dataVersion } from '@/features/excel/dataset/data-changes'
import { t } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'
import { errorText } from '@/utils/busy-hint'
import { formatMoney } from '@/utils/format-money'
import BigGrid from './BigGrid.vue'
import CleanDialog from './CleanDialog.vue'
import ColumnKindDialog from './ColumnKindDialog.vue'
import FilterMenu from './FilterMenu.vue'
import { formatCell } from './grid-format'
import ProfileDrawer from './ProfileDrawer.vue'
import SplitMergeDialog from './SplitMergeDialog.vue'
import { formatTotal, pickTotal, totalFuncs, totalLabel } from './totals'
import ValidationDialog from './ValidationDialog.vue'
import WindowCalcDialog from './WindowCalcDialog.vue'

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
  /** 外部数据集刷新了文件列表：数据集定义（列、行数）变了 */
  (e: 'changed', datasetId: string): void
  /** 生成了新数据集（窗口计算）：刷新列表后打开它 */
  (e: 'openDataset', datasetId: string): void
  /** 对账结果：回到对账工作台 */
  (e: 'backToRecon'): void
  /** 以当前筛选 / 排序 / 搜索放入工作表（数据表） */
  (e: 'embed', datasetId: string, query: DatasetQuery): void
}>()

const ledgerUser = useLedgerUserStore()
const refreshing = ref(false)
/** 外部数据集设置（读取方式、自动检查） */
const externalSettings = ref(false)

/** 列头菜单里的「更改列类型」：导入 / 填报的数据表，所有者与管理员；计算列、贡献列不行 */
const kindColumn = ref<{ title: string, kind: DatasetColumnKind } | null>(null)
function canChangeKind(col: GridColumn) {
  const d = props.dataset
  const owner = !ledgerUser.authRequired || ledgerUser.isAdmin || (!!d.ownerId && d.ownerId === ledgerUser.user?.id)
  const real = d.columns.find(c => c.key === col.key)
  return d.kind === 'table' && owner && !!real && !real.computed
}

/** 刷新 / 修改外部数据集的结果提示；有变化时通知外层重新加载 */
function reportRefresh(o: ExternalRefreshOutcome) {
  const parts = [t('{0} 个文件、{1} 行', [o.fileCount, o.rows.toLocaleString()])]
  if (!o.changed)
    parts.push(t('文件没有变化'))
  if (o.addedFiles || o.removedFiles)
    parts.push(t('新增 {0} 个、移除 {1} 个文件', [o.addedFiles, o.removedFiles]))
  if (o.addedColumns.length)
    parts.push(t('新增列：{0}', [o.addedColumns.join('、')]))
  if (o.removedColumns.length)
    parts.push(t('移除列：{0}', [o.removedColumns.join('、')]))
  ElMessage.success(parts.join('；'))
  // 没变化时也刷新一下：检查时间、自动检查的设置要更新
  emit('changed', props.dataset.id)
}

/** 外部数据集设置已保存：读取方式变了时和刷新一样提示结果，只改了自动检查时说「已保存」 */
function onExternalSaved(o: ExternalRefreshOutcome) {
  if (o.changed)
    return reportRefresh(o)
  ElMessage.success(t('已保存'))
  emit('changed', props.dataset.id)
}

/** 外部数据集的说明（悬停在「外部文件」标签上） */
const externalTip = computed(() => {
  const e = props.dataset.external
  if (!e)
    return ''
  const lines = [t('外部数据集：直接读取 {0} 下的 {1} 个 Parquet 文件（{2}），只读；文件列表更新于 {3}', [e.path, e.fileCount, e.pattern, e.refreshedAt])]
  lines.push(e.refreshMinutes ? t('每 {0} 分钟自动检查文件变化', [e.refreshMinutes]) : t('不自动检查，文件有变化后请手动刷新'))
  if (e.checkError)
    lines.push(t('上次检查失败（{0}）：{1}', [e.checkedAt ?? '', e.checkError]))
  return lines.join('\n')
})

/** 外部数据集：重新读取文件列表与结构（只有管理员） */
async function refreshExternal() {
  refreshing.value = true
  try {
    reportRefresh(await refreshExternalApi(props.dataset.id))
  }
  catch (e) {
    ElMessage.error(errorText(e))
  }
  finally {
    refreshing.value = false
  }
}

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
/** 只看不符合某条校验规则的行（label 是规则的说明，显示在筛选标签上） */
const violation = shallowRef<{ rule: ValidationRule, label: string } | null>(null)
/** 分类汇总的列（列键）：数据先按它排序，每组后面插一行小计；按这一列排序时用那个方向 */
const groupBy = ref<string | null>(null)
const groupSort = computed(() => groupBy.value ? { column: groupBy.value, desc: sort.value?.column === groupBy.value ? sort.value.desc : false } : null)

const query = computed<DatasetQuery>(() => ({
  filters: [...(props.drill?.filters ?? []), ...userFilters.value],
  keyword: keyword.value.trim() || undefined,
  sort: groupSort.value ? [groupSort.value, ...(sort.value && sort.value.column !== groupBy.value ? [sort.value] : [])] : sort.value ? [sort.value] : [],
  rule: useRule.value ? props.drill?.rule : undefined,
  violation: violation.value?.rule,
}))

const filteredColumns = computed(() => new Set(userFilters.value.map(f => f.column)))

/** 数据概况（按当前筛选）；在概况里点一档 / 一个取值：替换这一列的筛选 */
const profileVisible = ref(false)
function onProfileFilter(filter: DatasetFilter) {
  userFilters.value = [...userFilters.value.filter(f => f.column !== filter.column), filter]
}

// ── 游标 ──
const loading = ref(false)
const error = ref('')
let controller: AbortController | null = null

/** 上一次成功的查询条件：取消 / 失败后还原到它，界面上的筛选标签、排序箭头才和表格里的数据一致 */
let lastGood: { key: string, filters: DatasetFilter[], keyword: string, sort: { column: string, desc: boolean } | null, useRule: boolean, violation: typeof violation.value, groupBy: string | null } | null = null
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
  const snapshot = { filters: JSON.parse(JSON.stringify(userFilters.value)), keyword: keyword.value, sort: sort.value ? { ...sort.value } : null, useRule: useRule.value, violation: violation.value, groupBy: groupBy.value }
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
      error.value = errorText(e, t('查询失败，请重试'))
      console.warn('[big-grid] 建游标失败:', e)
    }
  }
  finally {
    if (c === controller)
      loading.value = false
  }
}
const debouncedOpen = useDebounceFn(() => openCursor(), 300)

// 告诉 AI 助手正在看什么：数据集、当前筛选 / 搜索 / 排序（列键换成列名）、命中行数（查询中时不给，免得是上一次的）
watch(() => [props.dataset, props.drill, query.value, cursor.value, loading.value, violation.value] as const, () => {
  const d = props.dataset
  const title = (key: string) => key === CONTRIBUTION_KEY ? t('贡献金额') : d.columns.find(c => c.key === key)?.title ?? key
  publishDatasetView({
    id: d.id,
    name: d.name,
    rows: d.rowCount,
    matched: loading.value ? undefined : cursor.value?.total,
    filters: (query.value.filters ?? []).map(f => ({ ...f, column: title(f.column) })),
    sort: (query.value.sort ?? []).map(s => ({ ...s, column: title(s.column) })),
    keyword: query.value.keyword,
    drill: props.drill?.title,
    violation: violation.value?.label,
  })
}, { immediate: true })
onBeforeUnmount(() => clearDatasetView(props.dataset.id))

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
  violation.value = g.violation
  groupBy.value = g.groupBy
}

watch(() => props.drill, () => {
  userFilters.value = []
  keyword.value = ''
  sort.value = null
  useRule.value = true
  violation.value = null
  groupBy.value = null
  openCursor(true)
}, { immediate: true })
watch(() => [userFilters.value, sort.value, useRule.value, violation.value, groupBy.value], () => openCursor(), { deep: true })
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

function onGroup(on: boolean) {
  groupBy.value = on ? menu.value!.column.key : null
  menu.value = null
}

function filterLabel(f: DatasetFilter): string {
  const title = columns.value.find(c => c.key === f.column)?.title ?? f.column
  switch (f.op) {
    case 'in': {
      const shown = f.values.slice(0, 2).map(v => v || t('（空白）')).join('、')
      return `${title}：${shown}${f.values.length > 2 ? t(' 等 {0} 项', [f.values.length]) : ''}`
    }
    case 'between': return `${title}：${f.min ?? '…'} ~ ${f.max ?? '…'}`
    case 'contains': return t('{0} 包含「{1}」', [title, f.value])
    case 'prefix': return t('{0} 以「{1}」开头', [title, f.value || t('（空白）')])
    case 'prefixes': return t('{0} 以「{1}」之一开头', [title, f.values.map(v => v || t('（空白）')).join('」「')])
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
  const fmt = (v: number) => exp.kind === 'count' ? t('{0} 笔', [v.toLocaleString()]) : formatMoney(v)
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
  // 分类汇总时只数数据行（不含小计行）
  return {
    rows: s.dataRows !== undefined ? (s.dataRows ? s.dataRows[1] - s.dataRows[0] + 1 : 0) : Math.abs(s.focus.row - s.anchor.row) + 1,
    cols: Math.abs(s.focus.col - s.anchor.col) + 1,
  }
})

const loadStats = useDebounceFn(async () => {
  const s = selection.value
  const cur = cursor.value
  statsController?.abort()
  if (!s || !cur || s.dataRows === null || (selectionSize.value!.rows <= 1 && selectionSize.value!.cols === 1)) {
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
      from: s.dataRows ? s.dataRows[0] : Math.min(s.anchor.row, s.focus.row),
      to: s.dataRows ? s.dataRows[1] : Math.max(s.anchor.row, s.focus.row),
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

// ── 汇总行：每列选一种汇总方式，服务端按当前筛选对整个结果集计算；开关和每列的选择记在本机 ──
const totalsOn = useLocalStorage(`big-grid-totals-on:${props.dataset.id}`, false)
const totalsSaved = useLocalStorage<Record<string, TotalFunc | ''>>(`big-grid-totals:${props.dataset.id}`, {})
const totalPicks = computed(() => columns.value.map(c => pickTotal(c, totalsSaved.value)))
/** 算好的值：「列|方式」→ 值（改了某列的方式，别的列不用等） */
const totalValues = shallowRef<Record<string, CellValue>>({})
const totalsInfo = ref<{ rows: number, elapsedMs: number } | null>(null)
const totalsError = ref('')
let totalsController: AbortController | null = null

const totalItems = computed(() => columns.value
  .map((c, i) => ({ column: c.key, func: totalPicks.value[i]! }))
  .filter((x): x is { column: string, func: TotalFunc } => !!x.func))

const loadTotals = useDebounceFn(async () => {
  totalsController?.abort()
  const cur = cursor.value
  const items = totalItems.value
  if (!totalsOn.value || !cur || !items.length)
    return
  const c = new AbortController()
  totalsController = c
  totalsError.value = ''
  try {
    const r = await totalsApi(props.dataset.id, { cursorId: cur.cursorId, items }, c.signal)
    // 游标失效时表格会重新建游标，换了游标后再算
    if (c !== totalsController || r.expired)
      return
    const next: Record<string, CellValue> = {}
    items.forEach((it, i) => next[`${it.column}|${it.func}`] = r.values[i] ?? null)
    totalValues.value = next
    totalsInfo.value = { rows: cur.total, elapsedMs: r.elapsedMs }
  }
  catch (e) {
    if (!isCancel(e) && c === totalsController)
      totalsError.value = errorText(e)
  }
}, 150)

watch(() => totalsOn.value && cursor.value ? JSON.stringify([cursor.value.cursorId, cursor.value.total, totalItems.value]) : '', (key, before) => {
  if (!key) {
    totalsController?.abort()
    return
  }
  // 换了游标（筛选 / 数据变了）：旧的值不能再显示
  if (before && JSON.parse(before)[0] !== JSON.parse(key)[0])
    totalValues.value = {}
  loadTotals()
}, { immediate: true })

const footer = computed<GridFooter | null>(() => {
  if (!totalsOn.value)
    return null
  const info = totalsInfo.value
  return {
    label: totalsError.value ? t('出错') : t('汇总'),
    error: totalsError.value,
    tip: info ? t('按当前筛选的 {0} 行计算，耗时 {1} ms；点各列的汇总格选择汇总方式', [info.rows.toLocaleString(), info.elapsedMs.toLocaleString()]) : '',
    cells: columns.value.map((c, i) => {
      const f = totalPicks.value[i]
      if (!f)
        return { label: '', value: '' }
      const k = `${c.key}|${f}`
      return { label: totalLabel(f, c.kind), value: k in totalValues.value ? formatTotal(totalValues.value[k] ?? null, f, c) : '…' }
    }),
  }
})

// ── 分类汇总：按分组列每组一行小计，汇总方式与汇总行相同（默认数值列求和）──
const groups = shallowRef<{ cursorId: string, column: string, items: { column: string, func: TotalFunc }[], rows: { key: CellValue, count: number, values: CellValue[] }[] } | null>(null)
let groupsController: AbortController | null = null
/** 折叠的组（按组的取值记，筛选变了组的位置变了也还认得）；换分组列时清空 */
const collapsedKeys = ref(new Set<string>())
const groupKey = (k: CellValue) => JSON.stringify(k ?? null)
watch(groupBy, () => collapsedKeys.value = new Set())

function toggleGroup(g: number) {
  const row = groups.value?.rows[g]
  if (!row)
    return
  const next = new Set(collapsedKeys.value)
  const k = groupKey(row.key)
  if (!next.delete(k))
    next.add(k)
  collapsedKeys.value = next
}

/** 全部折叠（只看各组小计）/ 全部展开 */
function collapseAll(on: boolean) {
  collapsedKeys.value = on ? new Set((groups.value?.rows ?? []).map(r => groupKey(r.key))) : new Set()
}

const loadGroups = useDebounceFn(async () => {
  groupsController?.abort()
  const cur = cursor.value
  const column = groupBy.value
  if (!cur || !column || loading.value)
    return
  const items = totalItems.value.filter(it => it.column !== CONTRIBUTION_KEY)
  const c = new AbortController()
  groupsController = c
  try {
    const q = query.value
    const r = await groupsApi(props.dataset.id, { filters: q.filters, keyword: q.keyword, rule: q.rule, violation: q.violation, column, desc: groupSort.value?.desc ?? false, items }, c.signal)
    if (c === groupsController)
      groups.value = { cursorId: cur.cursorId, column, items, rows: r.groups }
  }
  catch (e) {
    if (!isCancel(e) && c === groupsController) {
      ElMessage.warning(errorText(e))
      groupBy.value = null
    }
  }
}, 100)

watch(() => groupBy.value && cursor.value && !loading.value ? JSON.stringify([cursor.value.cursorId, groupBy.value, totalItems.value]) : '', (key) => {
  if (key)
    loadGroups()
  else if (!groupBy.value)
    groups.value = null
})

/** 交给表格的分组：只用和当前游标、当前分组列对得上的那一份 */
const gridGroups = computed<GridGroups | null>(() => {
  const g = groups.value
  if (!g || !groupBy.value || g.column !== groupBy.value || g.cursorId !== cursor.value?.cursorId)
    return null
  const cols = columns.value
  const at = new Map(g.items.map((it, i) => [it.column, i]))
  const collapsed = collapsedKeys.value
  return {
    sizes: g.rows.map(r => r.count),
    collapsed: g.rows.map(r => collapsed.has(groupKey(r.key))),
    cells: (gi) => {
      const row = g.rows[gi]!
      return cols.map((c) => {
        if (c.key === g.column)
          return t('{0} 小计（{1} 行）', [row.key == null || row.key === '' ? t('（空）') : formatCell(row.key, c) || String(row.key), row.count.toLocaleString()])
        const i = at.get(c.key)
        return i == null ? null : formatTotal(row.values[i] ?? null, g.items[i]!.func, c)
      })
    },
  }
})
const groupInfo = computed(() => gridGroups.value ? t('（{0} 组）', [gridGroups.value.sizes.length.toLocaleString()]) : '')

const totalMenu = shallowRef<{ column: GridColumn, rect: DOMRect } | null>(null)
const totalMenuRef = ref<HTMLElement>()
onClickOutside(totalMenuRef, () => totalMenu.value = null)
const totalMenuStyle = computed(() => {
  const r = totalMenu.value?.rect
  return r ? { left: `${Math.max(4, Math.min(r.left, window.innerWidth - 160))}px`, bottom: `${window.innerHeight - r.top + 4}px` } : {}
})
const totalMenuCurrent = computed(() => totalMenu.value ? pickTotal(totalMenu.value.column, totalsSaved.value) : '')

function setTotal(col: GridColumn, func: TotalFunc | '') {
  totalsSaved.value = { ...totalsSaved.value, [col.key]: func }
  totalMenu.value = null
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
  if (typeof v === 'boolean')
    return formatBool(v)
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

const windowVisible = ref(false)
/** 数据校验：所有人能看规则、重新检查；所有者 / 管理员能改规则 */
const validationVisible = ref(false)
const canManage = computed(() => {
  const d = props.dataset
  return !ledgerUser.authRequired || ledgerUser.isAdmin || (!!d.ownerId && d.ownerId === ledgerUser.user?.id)
})
/** 拆分 / 合并列（加计算列）：关联视图上不能加计算列 */
const splitVisible = ref(false)
/** 数据清洗：只有数据表（导入 / 填报的）的所有者或管理员，数据范围受限的不行 */
const cleanVisible = ref(false)
const canClean = computed(() => {
  const d = props.dataset
  const owner = !ledgerUser.authRequired || ledgerUser.isAdmin || (!!d.ownerId && d.ownerId === ledgerUser.user?.id)
  return d.kind === 'table' && owner && !ledgerUser.restricted
})

// 别处（AI 助手）改了这个数据集的数据：重新取数
watch(() => dataVersion(props.dataset.id), () => openCursor(true))

/** 清洗改了数据（行数可能没变）：数据表重新取数，外层刷新数据集列表 */
function onCleaned() {
  openCursor(true)
  emit('changed', props.dataset.id)
}

function onMore(cmd: 'calc' | 'split' | 'join' | 'window' | 'clean' | 'validate' | 'copy' | 'goto') {
  if (cmd === 'validate')
    validationVisible.value = true
  else if (cmd === 'split')
    splitVisible.value = true
  else if (cmd === 'clean')
    cleanVisible.value = true
  else if (cmd === 'window')
    windowVisible.value = true
  else if (cmd === 'calc')
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
  props.drill ? t('下钻：{0}', [props.drill.title]) : '',
  violation.value ? t('不符合 {0}', [violation.value.label]) : '',
  ...userFilters.value.map(filterLabel),
  keyword.value.trim() ? t('关键字「{0}」', [keyword.value.trim()]) : '',
  sort.value ? t('按「{0}」{1}', [columns.value.find(c => c.key === sort.value!.column)?.title ?? sort.value.column, sort.value.desc ? t('降序') : t('升序')]) : '',
].filter(Boolean).join('；'))

/** 分类汇总时导出 Excel：带上分组、小计方式和折叠的组（折叠的太多时只在全部折叠时带上） */
function exportGroup(): ExportGroup | null {
  const g = groups.value
  if (!gridGroups.value || !g)
    return null
  const collapsed = g.rows.map((r, i) => collapsedKeys.value.has(groupKey(r.key)) ? i : -1).filter(i => i >= 0)
  const all = collapsed.length === g.rows.length
  return { column: g.column, desc: groupSort.value?.desc ?? false, items: g.items, collapsed: all || collapsed.length > 1000 ? [] : collapsed, allCollapsed: all }
}

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
  a.href = exportUrl(props.dataset.id, cur.cursorId, columns.value.map(c => c.key), name, format, queryDesc.value, exportGroup())
  a.click()
  // 浏览器要等服务端开始返回才会出现下载条：行数多时先说清楚在等什么，免得用户以为没反应又点一次
  const big = cur.total > (format === 'xlsx' ? 100_000 : 1_000_000)
  ElMessage({
    type: big ? 'info' : 'success',
    message: big
      ? t('正在生成 {0} 行的{1}，需要几十秒到几分钟；生成完成后浏览器会自动开始下载，请不要重复点击', [cur.total.toLocaleString(), format === 'xlsx' ? t(' Excel 文件') : t(' CSV 文件')])
      : t('开始导出 {0} 行{1}', [cur.total.toLocaleString(), format === 'xlsx' ? (gridGroups.value ? t('（含「汇总」页签和分类汇总，可在 Excel 里折叠）') : t('（含「汇总」页签）')) : '']),
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
      <el-tooltip v-if="dataset.kind === 'view' && dataset.view" placement="bottom" :content="$t('关联视图：按 {0} 带入 {1}（点击修改）', [dataset.view.on.map(k => k.left === k.right ? k.left : `${k.left}=${k.right}`).join(' + '), dataset.view.columns.map(c => c.title).join('、')])">
        <el-tag size="small" type="info" class="cursor-pointer" @click="emit('editJoin', dataset.id)">
          {{ $t('关联 ✎') }}
        </el-tag>
      </el-tooltip>
      <el-tooltip v-if="dataset.kind === 'external' && dataset.external" placement="bottom" :content="externalTip" popper-class="whitespace-pre-line">
        <el-tag size="small" :type="dataset.external.checkError ? 'danger' : 'info'" data-testid="external-tag">
          {{ dataset.external.refreshMinutes ? $t('外部文件 · 自动') : $t('外部文件') }}
        </el-tag>
      </el-tooltip>
      <el-button v-if="dataset.kind === 'external' && ledgerUser.isAdmin" link size="small" :loading="refreshing" :title="$t('文件有增减或结构变化后，重新读取文件列表')" @click="refreshExternal">
        {{ $t('刷新文件列表') }}
      </el-button>
      <el-button v-if="dataset.kind === 'external' && ledgerUser.isAdmin" link size="small" data-testid="external-settings" @click="externalSettings = true">
        {{ $t('设置…') }}
      </el-button>
      <span class="shrink-0 whitespace-nowrap text-xs text-gray-500 tabular-nums">
        {{ $t('{0} 行', [cursor ? cursor.total.toLocaleString() : '…']) }}
        <template v-if="cursor && cursor.total !== dataset.rowCount"> {{ $t('/ 共 {0}', [dataset.rowCount.toLocaleString()]) }}</template>
      </span>
      <el-input v-if="dataset.searchable" v-model="keyword" size="small" clearable :placeholder="$t('搜索订单号 / 流水号 / 商品 / 达人 / 备注')" class="min-w-44 max-w-72 flex-1" />
      <div class="min-w-0 flex flex-1 flex-wrap items-center gap-1.5 overflow-hidden">
        <el-tag v-if="dataset.validation" size="small" :type="dataset.validation.failed ? 'danger' : 'success'" effect="plain" class="cursor-pointer" data-testid="validation-badge" :title="dataset.validation.checkedAt ? $t('检查于 {0}', [dataset.validation.checkedAt]) : ''" @click="validationVisible = true">
          {{ dataset.validation.failed ? $t('校验：{0} 项不通过（{1} 行）', [dataset.validation.failed, dataset.validation.badRows.toLocaleString()]) : $t('校验通过') }}
        </el-tag>
        <el-tag v-if="groupBy" size="small" type="success" closable data-testid="group-tag" @close="groupBy = null">
          {{ $t('按「{0}」分类汇总', [columns.find(c => c.key === groupBy)?.title ?? groupBy]) }}{{ groupInfo }}
        </el-tag>
        <template v-if="gridGroups">
          <el-button v-if="collapsedKeys.size < gridGroups.sizes.length" link size="small" data-testid="collapse-all" :title="$t('只看各组的小计；点小计行的行号展开一组')" @click="collapseAll(true)">
            {{ $t('全部折叠') }}
          </el-button>
          <el-button v-if="collapsedKeys.size" link size="small" data-testid="expand-all" @click="collapseAll(false)">
            {{ $t('全部展开') }}
          </el-button>
        </template>
        <el-tag v-if="violation" size="small" type="warning" closable data-testid="violation-tag" @close="violation = null">
          {{ $t('只看不合格：{0}', [violation.label]) }}
        </el-tag>
        <el-tag v-for="(f, i) in userFilters" :key="i" size="small" closable @close="removeFilter(i)">
          {{ filterLabel(f) }}
        </el-tag>
        <el-button v-if="userFilters.length > 1" link size="small" @click="userFilters = []">
          {{ $t('清除全部筛选') }}
        </el-button>
      </div>
      <div class="ml-auto flex shrink-0 items-center gap-1.5">
        <el-button size="small" :title="$t('按当前筛选看每列的空值、取值个数、范围和分布')" data-testid="open-profile" @click="profileVisible = true">
          {{ $t('数据概况') }}
        </el-button>
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
              <el-dropdown-item v-if="dataset.kind !== 'view'" command="split" data-testid="open-split">
                {{ $t('拆分 / 合并列…（按分隔符拆成几列，或几列拼成一列）') }}
              </el-dropdown-item>
              <el-dropdown-item command="join">
                {{ $t('关联…（按键带入另一个数据集的列）') }}
              </el-dropdown-item>
              <el-dropdown-item command="validate" data-testid="open-validation">
                {{ $t('数据校验…（必填、不重复、范围、格式，找出不合格的行）') }}
              </el-dropdown-item>
              <el-dropdown-item v-if="canClean" command="clean" data-testid="open-clean">
                {{ $t('数据清洗…（去重、去空白、查找替换、填充空值）') }}
              </el-dropdown-item>
              <el-dropdown-item v-if="!ledgerUser.restricted" command="window" data-testid="open-window">
                {{ $t('窗口计算…（累计、排名、占比，生成新数据集）') }}
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
                {{ gridGroups ? $t('Excel（.xlsx，含分类汇总，在 Excel 里可折叠）') : $t('Excel（.xlsx，含汇总页签）') }}
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
        :footer="footer"
        :groups="gridGroups"
        @group-toggle="toggleGroup"
        @selection-change="onSelection"
        @footer-click="(column, rect) => totalMenu = { column, rect }"
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
        <button class="goto-btn" :class="{ on: totalsOn }" data-testid="toggle-totals" :title="$t('在表格底部显示每列的合计、平均、计数等（按当前筛选的全部行计算）')" @click="totalsOn = !totalsOn">
          Σ {{ $t('汇总行') }}
        </button>
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
      :can-change-kind="canChangeKind(menu.column)"
      :group-state="groupBy === menu.column.key ? 'this' : groupBy ? 'other' : 'none'"
      @apply="onApplyFilter"
      @sort="onSort"
      @group="onGroup"
      @close="menu = null"
      @change-kind="kindColumn = { title: menu!.column.title, kind: menu!.column.kind }; menu = null"
    />
    <div v-if="totalMenu" ref="totalMenuRef" class="total-menu" :style="totalMenuStyle" data-testid="total-menu">
      <div class="total-menu-title">
        {{ $t('「{0}」的汇总', [totalMenu.column.title]) }}
      </div>
      <button v-for="f in totalFuncs(totalMenu.column.kind)" :key="f" class="total-menu-item" :class="{ on: totalMenuCurrent === f }" @click="setTotal(totalMenu.column, f)">
        {{ totalLabel(f, totalMenu.column.kind) }}
      </button>
      <button class="total-menu-item" :class="{ on: !totalMenuCurrent }" @click="setTotal(totalMenu.column, '')">
        {{ $t('不汇总') }}
      </button>
    </div>
    <ValidationDialog v-model:visible="validationVisible" :dataset="dataset" :can-edit="canManage && !ledgerUser.restricted" @filter="(rule, label) => violation = { rule, label }" @changed="emit('changed', dataset.id)" />
    <SplitMergeDialog v-model:visible="splitVisible" :dataset="dataset" @changed="emit('changed', dataset.id)" />
    <CleanDialog v-model:visible="cleanVisible" :dataset="dataset" :query="query" :filter-text="queryDesc" @changed="onCleaned" />
    <WindowCalcDialog v-model:visible="windowVisible" :dataset="dataset" @created="(id: string) => emit('openDataset', id)" />
    <ProfileDrawer v-model:visible="profileVisible" :dataset-id="dataset.id" :query="query" @filter="onProfileFilter" />
    <ColumnKindDialog :dataset-id="dataset.id" :column="kindColumn" @close="kindColumn = null" @changed="emit('changed', dataset.id)" />
    <ExternalDatasetDialog v-if="dataset.kind === 'external'" v-model:visible="externalSettings" :dataset="dataset" @saved="onExternalSaved" />

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

  &.on {
    @apply bg-[#E7F4EC] text-[#0B5A2E];
  }
}

.total-menu {
  @apply fixed z-[3000] min-w-[140px] flex flex-col rounded border border-[#e6e6e6] border-solid bg-white py-1 text-[13px] shadow-lg;
}

.total-menu-title {
  @apply truncate px-3 py-1 text-xs text-gray-400;
}

.total-menu-item {
  @apply cursor-pointer border-none bg-transparent px-3 py-1 text-left hover:bg-[#F2F8F4];

  &.on {
    @apply font-semibold text-[#217346];
  }
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
