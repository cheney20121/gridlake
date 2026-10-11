<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { DatasetColumn, DatasetInfo } from '@/api/dataset'
import type { FloatLocation } from '@/features/excel/chart/chart-manager'
import type { DashboardSpec, KpiSpec, SlicerSpec, TableCardSpec } from '@/features/excel/dashboard/types'
import type { CellTableLocation, CellTableSpec, DataTableSpec } from '@/features/excel/datatable/types'
import { useDebounceFn } from '@vueuse/core'
import { computed, onBeforeUnmount, ref, shallowRef, watch } from 'vue'
import { chartBus, findFloat, insertFloat, listFloats, removeFloat, updateFloat } from '@/features/excel/chart/chart-manager'
import { CHART_COMPONENT_KEY } from '@/features/excel/chart/types'
import { datasetById } from '@/features/excel/dashboard/slicer'
import { KPI_KEY, SLICER_KEY, TABLE_KEY } from '@/features/excel/dashboard/types'
import { cellsToFloat, CellTableBlockedError, floatToCells, removeCellTable, resetCellTableEdits, updateCellTable } from '@/features/excel/datatable/cell-table'
import { editCount } from '@/features/excel/datatable/edits'
import { renameTableFormulas } from '@/features/excel/datatable/formula-input'
import { checkTableName, DATA_TABLE_KEY, findCellTable, getCellTables, insertDataTable, MAX_CELL_ROWS } from '@/features/excel/datatable/types'
import { recalculateTableFormulas } from '@/features/excel/formula/dataset-functions'
import { cellName } from '@/features/excel/writeback/input-table'
import AddPartsSection from './AddPartsSection.vue'
import DataTableEditor from './editors/DataTableEditor.vue'
import KpiEditor from './editors/KpiEditor.vue'
import SlicerEditor from './editors/SlicerEditor.vue'
import TableCardEditor from './editors/TableCardEditor.vue'
import PartsList from './PartsList.vue'

/**
 * 仪表盘面板：新建仪表盘页、添加切片器 / 指标卡 / 表格 / 图表、编辑选中的部件（即时生效）、演示模式
 */
const props = defineProps<{
  univerApi: FUniver | null
  datasets: DatasetInfo[]
  /** 正在编辑的部件 ID */
  editId: string | null
}>()

const emit = defineEmits<{
  (e: 'update:editId', id: string | null): void
  (e: 'changed'): void
  /** 新建数据集图表（交给图表面板） */
  (e: 'addChart', datasetId: string): void
  (e: 'editChart', id: string): void
  (e: 'present'): void
  /** 把当前仪表盘页设为定时推送 */
  (e: 'schedule'): void
}>()

const FORM_LABEL: Record<string, string> = { slicer: '切片器', kpi: '指标卡', table: '表格', datatable: '数据表' }

const datasetId = ref('')
const parts = shallowRef<FloatLocation[]>([])
/** 本页的单元格模式数据表 */
const cellParts = shallowRef<CellTableLocation[]>([])
const form = ref<DashboardSpec | DataTableSpec | CellTableSpec | null>(null)
/** 正在编辑的是单元格模式数据表 */
const editingCells = ref(false)
/** 单元格模式写入中（取数、写格子） */
const cellBusy = ref(false)
/** 数据表表名不合法（重名等）时的提示；不合法时不写回 */
const nameError = ref('')
/** 打开表单时的定义：没有改动时不写回 */
let loaded = ''

watch(() => props.datasets, (list) => {
  if (!datasetId.value || !list.some(d => d.id === datasetId.value))
    datasetId.value = (list.find(d => !d.builtin) ?? list[0])?.id ?? ''
}, { immediate: true })

const dataset = computed(() => props.datasets.find(d => d.id === (form.value?.datasetId ?? datasetId.value)))
const columns = computed(() => dataset.value?.columns ?? [])
const numeric = computed(() => columns.value.filter(c => c.kind === 'money' || c.kind === 'number'))
const dates = computed(() => columns.value.filter(c => c.kind === 'datetime'))
const groupable = computed(() => columns.value.filter(c => c.kind !== 'money' && c.kind !== 'number'))
const kindOf = (title: string): DatasetColumn['kind'] | undefined => columns.value.find(c => c.title === title || c.key === title)?.kind

function refreshParts() {
  const api = props.univerApi
  const sheetId = api?.getActiveWorkbook()?.getActiveSheet().getSheetId()
  parts.value = api && sheetId ? listFloats(api, undefined, sheetId) : []
  const sheet = api && sheetId ? api.getActiveWorkbook()?.getSheetBySheetId(sheetId) : null
  cellParts.value = sheet ? getCellTables(sheet).map(spec => ({ sheet, spec })) : []
}

/** 把定义载入表单（之后的改动与它比较，没有改动时不写回） */
function loadForm(spec: DashboardSpec | DataTableSpec | CellTableSpec | null, cells: boolean) {
  form.value = spec ? JSON.parse(JSON.stringify(spec)) : null
  loaded = form.value ? JSON.stringify(form.value) : ''
  editingCells.value = cells
}

watch(() => props.editId, (id) => {
  refreshParts()
  const loc = id && props.univerApi ? findFloat(props.univerApi, id) : null
  if (loc?.key === CHART_COMPONENT_KEY) {
    emit('editChart', loc.spec.id)
    return
  }
  const cell = !loc && id && props.univerApi ? findCellTable(props.univerApi, id) : null
  loadForm((loc?.spec as DashboardSpec | DataTableSpec | undefined) ?? cell?.spec ?? null, !!cell)
  nameError.value = ''
}, { immediate: true })

// 部件自己改了定义（数据表列头筛选 / 排序、切片器勾选）：同步到表单，避免之后用旧表单覆盖
const offSpec = chartBus.on('spec', (next: DashboardSpec | DataTableSpec) => {
  if (!form.value || next.id !== form.value.id)
    return
  const json = JSON.stringify(next)
  if (json !== JSON.stringify(form.value)) {
    loaded = json
    form.value = JSON.parse(json)
  }
})
onBeforeUnmount(offSpec)

// 修改即时写回（防抖）
const apply = useDebounceFn(() => {
  if (!form.value || !props.univerApi)
    return
  const json = JSON.stringify(form.value)
  if (json === loaded)
    return
  if (editingCells.value) {
    applyCells()
    return
  }
  // 部件已在工作表上被删除（如选中后按 Delete）：关闭表单
  if (!findFloat(props.univerApi, form.value.id)) {
    form.value = null
    emit('update:editId', null)
    refreshParts()
    return
  }
  if (form.value.kind === 'datatable') {
    nameError.value = checkTableName(props.univerApi, form.value.name, form.value.id) ?? ''
    if (nameError.value)
      return
  }
  try {
    const oldName: string | undefined = JSON.parse(loaded || '{}').name
    const renamed = form.value.kind === 'datatable' && !!oldName && oldName !== form.value.name
    updateFloat(props.univerApi, JSON.parse(json))
    loaded = json
    // 公式按表名引用：改名后把引用旧名的公式一并改掉
    if (renamed && form.value.kind === 'datatable') {
      const n = renameTableFormulas(props.univerApi, oldName!, form.value.name)
      if (n)
        ElMessage.success(`已把 ${n} 个引用「${oldName}」的公式改为「${form.value.name}」`)
    }
    // 公式按表名引用：改名后重算（引用旧名的公式会提示找不到）
    if (renamed)
      recalculateTableFormulas()
    emit('changed')
    refreshParts()
    // 切片器的字段 / 范围变了：同一工作表的部件重新取数
    if (form.value.kind === 'slicer') {
      const sheetId = findFloat(props.univerApi, form.value.id)?.sheet.getSheetId()
      if (sheetId)
        chartBus.emit('slicer', sheetId)
    }
  }
  catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e))
  }
}, 250)
watch(form, apply, { deep: true })

function cellTableError(e: unknown) {
  if (e instanceof CellTableBlockedError)
    ElMessage.warning({ message: e.message, duration: 6000 })
  else
    ElMessage.error(e instanceof Error ? e.message : String(e))
}

/** 单元格模式：改定义后重新取数写入（写入位置、上次的区域以工作表上保存的为准） */
async function applyCells(force = false) {
  const api = props.univerApi
  const f = form.value as CellTableSpec | null
  if (!api || !f)
    return
  const json = JSON.stringify(f)
  if (json === loaded && !force)
    return
  const current = findCellTable(api, f.id)
  if (!current) {
    loadForm(null, false)
    emit('update:editId', null)
    refreshParts()
    return
  }
  nameError.value = checkTableName(api, f.name, f.id) ?? ''
  if (nameError.value)
    return
  const oldName = current.spec.name
  loaded = json
  cellBusy.value = true
  try {
    const next = await updateCellTable(api, f)
    if (oldName !== next.name) {
      const n = renameTableFormulas(api, oldName, next.name)
      if (n)
        ElMessage.success(`已把 ${n} 个引用「${oldName}」的公式改为「${next.name}」`)
    }
    // 只带回写入结果（区域、总行数）；写入期间表单又改过的话，保留改动，稍后再写一次
    if (form.value?.id === next.id) {
      const unchanged = JSON.stringify(form.value) === json
      Object.assign(form.value, { extent: next.extent, total: next.total })
      if (unchanged)
        loaded = JSON.stringify(form.value)
    }
    emit('changed')
    refreshParts()
  }
  catch (e) {
    cellTableError(e)
  }
  finally {
    cellBusy.value = false
  }
}

async function switchMode(mode: 'cells' | 'float') {
  const api = props.univerApi
  const f = form.value as DataTableSpec | null
  if (!api || !f || f.kind !== 'datatable' || (mode === 'cells') === editingCells.value)
    return
  cellBusy.value = true
  try {
    if (mode === 'cells') {
      const loc = findFloat<DataTableSpec>(api, f.id)
      if (!loc)
        throw new Error('数据表已被删除')
      const spec = await floatToCells(api, { ...loc, spec: { ...f } })
      loadForm(spec, true)
      if ((spec.total ?? 0) > MAX_CELL_ROWS)
        ElMessage.warning({ message: `共 ${spec.total!.toLocaleString()} 行，单元格模式只写入前 ${MAX_CELL_ROWS.toLocaleString()} 行；要看全部请用浮动模式`, duration: 6000 })
      else
        ElMessage.success(`已写入 ${spec.extent!.rows - 1} 行，可以像普通单元格一样引用`)
    }
    else {
      const spec = cellsToFloat(api, f.id)
      loadForm(spec, false)
    }
    emit('changed')
    refreshParts()
  }
  catch (e) {
    cellTableError(e)
  }
  finally {
    cellBusy.value = false
  }
}

/** 单元格模式：解除关联，内容留作普通单元格 */
async function detachCells() {
  const api = props.univerApi
  const f = form.value
  if (!api || !f)
    return
  const ok = await ElMessageBox.confirm('解除后这些单元格成为普通数据，不再随数据集、切片器刷新；引用这个表名的公式会提示找不到数据表。', '转为普通单元格', { type: 'warning', confirmButtonText: '解除关联' }).then(() => true, () => false)
  if (!ok)
    return
  removeCellTable(api, f.id, true)
  recalculateTableFormulas()
  loadForm(null, false)
  emit('update:editId', null)
  emit('changed')
  refreshParts()
}

/** 「保留手工调整」要按主键认行：数据集没有主键、主键列没显示在表里时说明原因（空表示可用） */
function keyProblem(f: CellTableSpec) {
  const pk = datasetById(f.datasetId)?.primaryKey ?? []
  if (!pk.length)
    return '这个数据集没有主键，无法按行保留'
  const shown = f.fields.length ? f.fields : columns.value.map(c => c.title)
  const missing = pk.filter(k => !shown.includes(k))
  return missing.length ? `主键列「${missing.join('」「')}」需要显示在表里` : null
}

/** 清掉记下的手工调整，按数据集重新写入 */
async function resetEdits() {
  const api = props.univerApi
  const f = form.value
  if (!api || !f)
    return
  const ok = await ElMessageBox.confirm('表里手改过的格子会恢复成数据集里的值。', '恢复数据集的值', { type: 'warning', confirmButtonText: '恢复' }).then(() => true, () => false)
  if (!ok)
    return
  cellBusy.value = true
  try {
    loadForm(await resetCellTableEdits(api, f.id), true)
    emit('changed')
  }
  catch (e) {
    cellTableError(e)
  }
  finally {
    cellBusy.value = false
  }
}

/** 单元格模式写入的区域与数据列（给公式示例用） */
const cellInfo = computed(() => {
  const f = form.value as CellTableSpec | null
  if (!editingCells.value || !f?.extent)
    return null
  const { rows, cols } = f.extent
  return {
    range: `${cellName(f.row, f.col)}:${cellName(f.row + rows - 1, f.col + cols - 1)}`,
    editCount: editCount(f.edits),
    keyProblem: keyProblem(f),
    written: rows - 1,
    total: f.total ?? rows - 1,
    firstNumeric: (() => {
      const shown = f.fields.length ? f.fields : columns.value.map(c => c.title)
      const i = shown.findIndex(n => numeric.value.some(c => c.title === n))
      return i >= 0 && rows > 1 ? `=SUM(${cellName(f.row + 1, f.col + i)}:${cellName(f.row + rows - 1, f.col + i)})` : null
    })(),
  }
})

// ── 新建 ──

const newId = (p: string) => `${p}_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`

function createDashboardSheet() {
  const wb = props.univerApi?.getActiveWorkbook()
  if (!wb)
    return
  const names = new Set(wb.getSheets().map(s => s.getSheetName()))
  let n = 1
  while (names.has(`仪表盘${n}`))
    n++
  const sheet = wb.insertSheet(`仪表盘${n}`)
  ;(sheet as unknown as { setHiddenGridlines?: (h: boolean) => void }).setHiddenGridlines?.(true)
  wb.setActiveSheet(sheet)
  refreshParts()
  emit('changed')
  ElMessage.success(`已新建「仪表盘${n}」：从下方添加切片器、指标卡、图表、表格`)
}

function add(kind: 'slicer' | 'kpi' | 'table' | 'datatable') {
  const api = props.univerApi
  const ds = props.datasets.find(d => d.id === datasetId.value)
  if (!api || !ds) {
    ElMessage.warning('请先选择数据集')
    return
  }
  if (kind === 'datatable') {
    try {
      const spec = insertDataTable(api, { datasetId: ds.id, datasetName: ds.name })
      emit('changed')
      emit('update:editId', spec.id)
      recalculateTableFormulas()
    }
    catch (e) {
      ElMessage.error(e instanceof Error ? e.message : String(e))
    }
    return
  }
  const text = ds.columns.find(c => c.kind === 'text') ?? ds.columns.find(c => c.kind === 'id')
  const date = ds.columns.find(c => c.kind === 'datetime')
  const money = ds.columns.find(c => c.kind === 'money') ?? ds.columns.find(c => c.kind === 'number')
  let spec: DashboardSpec
  let key: string
  let size: { width: number, height: number }
  if (kind === 'slicer') {
    const col = text ?? date ?? ds.columns[0]
    if (!col)
      return
    spec = { v: 1, id: newId('sl'), kind: 'slicer', title: col.title, datasetId: ds.id, column: col.title, mode: col.kind === 'datetime' ? 'date' : 'values', values: [], range: null } satisfies SlicerSpec
    key = SLICER_KEY
    size = col.kind === 'datetime' ? { width: 280, height: 150 } : { width: 220, height: 260 }
  }
  else if (kind === 'kpi') {
    spec = {
      v: 1,
      id: newId('kpi'),
      kind: 'kpi',
      title: money ? `${money.title}合计` : '行数',
      datasetId: ds.id,
      value: money ? { column: money.title, agg: 'sum' } : { agg: 'count' },
      dateColumn: date?.title ?? null,
      granularity: 'month',
      compare: date ? 'mom' : 'none',
      upIsGood: true,
      sparkline: !!date,
    } satisfies KpiSpec
    key = KPI_KEY
    size = { width: 260, height: 136 }
  }
  else {
    const row = text ?? ds.columns[0]
    if (!row)
      return
    spec = {
      v: 1,
      id: newId('tb'),
      kind: 'table',
      title: money ? `${money.title}前 10 的${row.title}` : `${row.title}行数`,
      datasetId: ds.id,
      row: { column: row.title, granularity: null },
      values: [money ? { column: money.title, agg: 'sum' } : { agg: 'count' }],
      top: 10,
      sort: 'value',
    } satisfies TableCardSpec
    key = TABLE_KEY
    size = { width: 380, height: 340 }
  }
  try {
    insertFloat(api, key, spec, size)
    emit('changed')
    emit('update:editId', spec.id)
  }
  catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e))
  }
}

async function remove(id: string) {
  const isCells = !!(props.univerApi && findCellTable(props.univerApi, id))
  const ok = await ElMessageBox.confirm(isCells ? '删除这个数据表？写入的单元格会被清空。' : '删除这个部件？', '删除', { type: 'warning', confirmButtonText: '删除' }).then(() => true, () => false)
  if (!ok || !props.univerApi)
    return
  if (isCells) {
    removeCellTable(props.univerApi, id)
    recalculateTableFormulas()
    if (props.editId === id)
      emit('update:editId', null)
    refreshParts()
    emit('changed')
    return
  }
  const loc = findFloat(props.univerApi, id)
  removeFloat(props.univerApi, id)
  if (loc?.key === SLICER_KEY)
    chartBus.emit('slicer', loc.sheet.getSheetId())
  if (loc?.key === DATA_TABLE_KEY)
    recalculateTableFormulas()
  if (props.editId === id)
    emit('update:editId', null)
  refreshParts()
  emit('changed')
}

function editPart(p: FloatLocation) {
  if (p.key === CHART_COMPONENT_KEY)
    emit('editChart', p.spec.id)
  else
    emit('update:editId', p.spec.id)
}

// 切换数据集：字段重置为该数据集的默认
function onDatasetChange() {
  const f = form.value
  const ds = dataset.value
  if (!f || !ds)
    return
  const text = ds.columns.find(c => c.kind === 'text')
  const money = ds.columns.find(c => c.kind === 'money')
  if (f.kind === 'slicer' && !ds.columns.some(c => c.title === f.column)) {
    f.column = text?.title ?? ds.columns[0]?.title ?? ''
    f.values = []
  }
  if (f.kind === 'kpi') {
    f.value = money ? { column: money.title, agg: 'sum' } : { agg: 'count' }
    f.dateColumn = ds.columns.find(c => c.kind === 'datetime')?.title ?? null
  }
  if (f.kind === 'table') {
    f.row = { column: text?.title ?? ds.columns[0]?.title ?? '', granularity: null }
    f.values = [money ? { column: money.title, agg: 'sum' } : { agg: 'count' }]
  }
  if (f.kind === 'datatable') {
    f.fields = []
    f.filters = []
    f.sort = []
    f.keyword = undefined
    f.widths = {}
  }
}

defineExpose({ refreshParts })
</script>

<template>
  <div class="flex flex-col gap-3 text-[13px]">
    <div class="flex gap-2">
      <el-button size="small" type="primary" @click="createDashboardSheet">
        新建仪表盘页
      </el-button>
      <el-button size="small" title="全屏展示当前工作表，隐藏工具栏（Esc 退出）" @click="emit('present')">
        演示
      </el-button>
      <el-button size="small" title="按计划把当前仪表盘页推送到企业微信 / 钉钉 / 飞书群或邮箱" @click="emit('schedule')">
        定时推送
      </el-button>
    </div>

    <!-- 添加 -->
    <AddPartsSection v-model="datasetId" :datasets="datasets" @add="add" @add-chart="emit('addChart', datasetId)" />

    <!-- 编辑 -->
    <div v-if="form" class="section">
      <div class="section-title flex items-center justify-between">
        <span>编辑{{ FORM_LABEL[form.kind] }}</span>
        <el-button link size="small" @click="emit('update:editId', null)">
          完成
        </el-button>
      </div>
      <div v-if="form.kind !== 'datatable'" class="row">
        <span class="label">标题</span>
        <el-input v-model="form.title" size="small" maxlength="40" />
      </div>
      <template v-else>
        <div class="row">
          <span class="label">表名</span>
          <el-input v-model="form.name" size="small" maxlength="30" />
        </div>
        <div v-if="nameError" class="text-xs text-[#B91C1C]">
          {{ nameError }}
        </div>
      </template>
      <div class="row">
        <span class="label">数据集</span>
        <el-select v-model="form.datasetId" size="small" filterable class="flex-1" @change="onDatasetChange">
          <el-option v-for="d in datasets" :key="d.id" :label="d.name" :value="d.id" />
        </el-select>
      </div>

      <DataTableEditor
        v-if="form.kind === 'datatable'"
        v-model="form"
        :columns="columns"
        :numeric="numeric"
        :editing-cells="editingCells"
        :cell-busy="cellBusy"
        :cell-info="cellInfo"
        @switch-mode="switchMode"
        @refresh-cells="applyCells(true)"
        @detach-cells="detachCells"
        @reset-edits="resetEdits"
      />
      <SlicerEditor v-else-if="form.kind === 'slicer'" v-model="form" :columns="columns" :kind-of="kindOf" />
      <KpiEditor v-else-if="form.kind === 'kpi'" v-model="form" :columns="columns" :numeric="numeric" :dates="dates" />
      <TableCardEditor v-else-if="form.kind === 'table'" v-model="form" :columns="columns" :numeric="numeric" :groupable="groupable" :kind-of="kindOf" />
    </div>

    <!-- 本页部件 -->
    <PartsList
      :parts="parts"
      :cell-parts="cellParts"
      :edit-id="editId"
      @refresh="refreshParts"
      @edit="editPart"
      @edit-cells="(id: string) => emit('update:editId', id)"
      @remove="remove"
    />
  </div>
</template>

<style scoped lang="scss">
.section {
  @apply flex flex-col gap-2 border-t border-[#eef0f3] border-t-solid pt-3;
}

.section-title {
  @apply text-xs text-gray-500 font-medium;
}

.row {
  @apply flex items-center gap-2;
}

.label {
  @apply w-14 shrink-0 text-xs text-gray-500;
}
</style>
