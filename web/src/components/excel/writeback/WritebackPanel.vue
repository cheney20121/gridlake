<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import type { CreateForm } from './WritebackCreateForm.vue'
import type { CellValue, DatasetFilter, DatasetInfo, WritebackMode, WritebackOutcome, WritebackRecord } from '@/api/dataset'
import type { InputSpec } from '@/features/excel/writeback/input-table'
import { computed, onBeforeUnmount, reactive, ref, shallowRef, watch } from 'vue'
import { createInputDatasetApi, getDatasetsApi, getRowsApi, listWritebacksApi, openCursorApi, undoWritebackApi, writebackApi } from '@/api/dataset'
import { watchSelection } from '@/features/excel/data/selection'
import {
  cellName,
  clearErrorMarks,
  clearInputRows,
  dataRowCount,
  getInputs,
  inputAt,
  markErrors,
  MAX_INPUT_ROWS,
  overlaps,
  readInputRows,
  removeInput,
  renderInput,
  saveInputs,
  updateInput,
  writeInputRows,
} from '@/features/excel/writeback/input-table'
import { t as tr } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'
import WritebackCreateForm from './WritebackCreateForm.vue'
import WritebackRecords from './WritebackRecords.vue'

/**
 * 填报回写面板：在工作表里放填报区，填好后提交到数据集（按主键更新或追加），可查看提交记录并撤销
 */
const props = defineProps<{
  univerApi: FUniver | null
  datasets: DatasetInfo[]
  workbookId: string | null
  operator: string
}>()

const emit = defineEmits<{
  /** 工作簿内容变化（需要保存） */
  (e: 'changed'): void
  /** 数据集内容或列表变化：刷新数据集、公式、图表 */
  (e: 'datasetsChanged'): void
}>()

const ledgerUser = useLedgerUserStore()

/** 载入现有数据的行数上限 */
const LOAD_LIMIT = 5000

const sheet = shallowRef<FWorksheet | null>(null)
const inputs = ref<InputSpec[]>([])
const currentId = ref('')
const anchor = ref({ row: 0, col: 0 })
const current = computed(() => inputs.value.find(i => i.id === currentId.value) ?? null)
const currentDataset = computed(() => props.datasets.find(d => d.id === current.value?.datasetId) ?? null)
const writable = computed(() => props.datasets.filter(d => d.kind === 'table'))

/** 转成普通对象（面板里的对象是响应式代理，不能直接存进工作表元数据） */
function plain<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T
}

function syncSheet() {
  const s = props.univerApi?.getActiveWorkbook()?.getActiveSheet() ?? null
  sheet.value = s
  inputs.value = plain(getInputs(s))
  if (!inputs.value.some(i => i.id === currentId.value))
    currentId.value = inputs.value[0]?.id ?? ''
}

/** 当前格子所在的填报区（不在任何填报区内时为空） */
const anchorInput = ref<InputSpec | null>(null)

function onSelection() {
  const s = sheet.value
  const r = s?.getSelection()?.getActiveRange()
  if (!s || !r)
    return
  anchor.value = { row: r.getRow(), col: r.getColumn() }
  anchorInput.value = inputAt(s, anchor.value.row, anchor.value.col)
  if (anchorInput.value)
    currentId.value = anchorInput.value.id
}

const offs: { dispose: () => void }[] = []
watch(() => props.univerApi, (api) => {
  offs.forEach(o => o.dispose())
  offs.length = 0
  if (api) {
    offs.push(
      api.addEvent(api.Event.ActiveSheetChanged, () => {
        syncSheet()
        onSelection()
      }),
      // 全局 SelectionChanged 在当前版本下不触发，用工作簿级的选区监听
      watchSelection(api, () => onSelection()),
    )
  }
  syncSheet()
  onSelection()
}, { immediate: true })
onBeforeUnmount(() => offs.forEach(o => o.dispose()))

function persist(spec: InputSpec) {
  if (!sheet.value)
    return
  updateInput(sheet.value, plain(spec))
  inputs.value = plain(getInputs(sheet.value))
  emit('changed')
}

// ── 新建填报区 ──
const creating = ref(false)
const form = reactive<CreateForm>({
  target: 'existing',
  name: '',
  datasetId: '',
  fields: [] as string[],
  mode: 'upsert',
  keys: [] as string[],
  newName: '',
  newColumns: [{ title: '', kind: 'text' }],
  newKeys: [] as string[],
})
const formDataset = computed(() => props.datasets.find(d => d.id === form.datasetId) ?? null)
const formColumns = computed(() => formDataset.value?.columns.filter(c => !c.computed) ?? [])
const formHasPk = computed(() => !!formDataset.value?.primaryKey.length)
const newTitles = computed(() => form.newColumns.map(c => c.title.trim()).filter(Boolean))

function startCreate() {
  creating.value = true
  form.name = `填报${inputs.value.length + 1}`
  if (!form.datasetId && writable.value[0])
    form.datasetId = writable.value[0].id
}

const createBusy = ref(false)
async function create() {
  const api = props.univerApi
  const s = sheet.value
  if (!api || !s)
    return
  let ds: DatasetInfo | null
  let fields: string[]
  let keys: string[]
  let mode: WritebackMode
  if (form.target === 'existing') {
    ds = formDataset.value
    if (!ds)
      return ElMessage.warning(tr('请选择目标数据集'))
    // 按数据集的列顺序
    fields = formColumns.value.map(c => c.title).filter(t => form.fields.includes(t))
    mode = form.mode
    keys = mode === 'upsert' ? (formHasPk.value ? [...ds.primaryKey] : form.keys.filter(k => fields.includes(k))) : []
    if (mode === 'upsert' && !keys.length)
      return ElMessage.warning(tr('按主键更新需要选择主键列'))
    if (keys.some(k => !fields.includes(k)))
      return ElMessage.warning(tr('主键列「{0}」必须包含在填报列里', [keys.find(k => !fields.includes(k))]))
  }
  else {
    if (!form.newName.trim())
      return ElMessage.warning(tr('请填写数据集名称'))
    if (!newTitles.value.length)
      return ElMessage.warning(tr('至少需要一列'))
    fields = newTitles.value
    keys = form.newKeys.filter(k => fields.includes(k))
    mode = keys.length ? 'upsert' : 'append'
    ds = null
  }
  if (!fields.length)
    return ElMessage.warning(tr('至少选择一列'))
  const { row, col } = anchor.value
  if (overlaps(s, row, col, fields.length))
    return ElMessage.warning(tr('从 {0} 开始会与已有的填报区重叠，请换一个位置', [cellName(row, col)]))
  const occupied = s.getRange(row, col, 2, fields.length).getValues().flat().some(v => v != null && v !== '')
  if (occupied) {
    const ok = await ElMessageBox.confirm(tr('{0} 开始的位置已有内容，表头会覆盖第一行。继续？', [cellName(row, col)]), tr('新建填报区'), { type: 'warning', confirmButtonText: tr('继续') }).then(() => true, () => false)
    if (!ok)
      return
  }
  createBusy.value = true
  try {
    if (!ds) {
      const columns = form.newColumns.filter(c => c.title.trim()).map(c => ({ title: c.title.trim(), kind: c.kind }))
      const id = await createInputDatasetApi({ name: form.newName.trim(), columns, primaryKey: keys })
      ds = (await getDatasetsApi()).find(d => d.id === id) ?? null
      emit('datasetsChanged')
      if (!ds)
        return
    }
    const spec: InputSpec = { id: `in_${Date.now().toString(36)}`, name: form.name.trim() || `填报${inputs.value.length + 1}`, datasetId: ds.id, datasetName: ds.name, row, col, fields, mode, keys }
    saveInputs(s, [...getInputs(s), plain(spec)])
    await renderInput(api, s, spec, ds)
    inputs.value = plain(getInputs(s))
    currentId.value = spec.id
    creating.value = false
    onSelection()
    emit('changed')
    ElMessage.success(tr('已在 {0} 放置填报区，从表头下一行开始填写', [cellName(row, col)]))
  }
  finally {
    createBusy.value = false
  }
}

async function removeCurrent() {
  const spec = current.value
  if (!spec || !sheet.value)
    return
  const ok = await ElMessageBox.confirm(tr('移除填报区「{0}」？表格里的内容保留，已提交到数据集的数据不受影响。', [spec.name]), tr('移除填报区'), { type: 'warning', confirmButtonText: tr('移除') }).then(() => true, () => false)
  if (!ok)
    return
  clearErrorMarks(props.univerApi!, spec.id)
  removeInput(sheet.value, spec.id)
  syncSheet()
  emit('changed')
}

// ── 提交 ──
const submitting = ref(false)
const result = shallowRef<{ specId: string, outcome: WritebackOutcome, sheetRows: number[] } | null>(null)

const modeText = (m: WritebackMode) => (m === 'upsert' ? tr('按主键更新') : tr('追加'))

async function submit() {
  const api = props.univerApi
  const s = sheet.value
  const spec = current.value
  const ds = currentDataset.value
  if (!api || !s || !spec)
    return
  if (!ds)
    return ElMessage.error(tr('目标数据集「{0}」不存在、已删除或在回收站', [spec.datasetName]))
  clearErrorMarks(api, spec.id)
  const { rows, sheetRows } = readInputRows(s, spec, ds)
  if (!rows.length)
    return ElMessage.warning(tr('填报区没有数据：从表头下一行开始填写，遇到整行空白即结束'))
  if (rows.length > MAX_INPUT_ROWS)
    return ElMessage.warning(tr('一次最多提交 {0} 行', [MAX_INPUT_ROWS.toLocaleString()]))
  submitting.value = true
  try {
    const outcome = await writebackApi(ds.id, {
      mode: spec.mode,
      columns: spec.fields,
      rows,
      keys: spec.keys,
      operator: props.operator,
      source: { workbookId: props.workbookId, sheetName: s.getSheetName(), inputName: spec.name },
    })
    result.value = { specId: spec.id, outcome, sheetRows }
    if (outcome.errors.length) {
      markErrors(api, s, spec, sheetRows, outcome.errors)
      ElMessage.error(tr('有 {0} 处需要修改（已标红），本次没有写入任何数据', [outcome.errorCount]))
      return
    }
    const changed = outcome.inserted + outcome.updated
    persist({ ...spec, last: { batchId: outcome.batchId!, at: new Date().toLocaleString('zh-CN', { hour12: false }), by: props.operator, inserted: outcome.inserted, updated: outcome.updated } })
    if (spec.mode === 'append')
      clearInputRows(s, spec)
    ElMessage.success(changed
      ? `已写入「${outcome.datasetName}」：新增 ${outcome.inserted} 行${outcome.updated ? `，更新 ${outcome.updated} 行` : ''}${outcome.unchanged ? `，${outcome.unchanged} 行无变化` : ''}${spec.mode === 'append' ? '；填报区已清空' : ''}`
      : tr('{0} 行与数据集完全相同，无需更新', [outcome.unchanged]))
    if (changed)
      emit('datasetsChanged')
    loadRecords()
  }
  finally {
    submitting.value = false
  }
}

const currentResult = computed(() => (result.value && result.value.specId === currentId.value ? result.value : null))

function gotoError(row: number, column: string) {
  const s = sheet.value
  const spec = current.value
  const sheetRow = currentResult.value?.sheetRows[row]
  const ci = spec?.fields.indexOf(column) ?? -1
  if (s && spec && sheetRow != null)
    s.getRange(sheetRow, spec.col + Math.max(ci, 0)).activate()
}

// ── 删除选中的行（按主键，同步删除数据集里的记录）──
const deleting = ref(false)

async function deleteSelected() {
  const api = props.univerApi
  const s = sheet.value
  const spec = current.value
  const ds = currentDataset.value
  const sel = s?.getSelection()?.getActiveRange()
  if (!api || !s || !spec || !ds || !sel)
    return
  if (!(spec.keys.length || ds.primaryKey.length))
    return ElMessage.warning(tr('这个填报区没有主键，不能按主键删除'))
  const first = sel.getRow()
  const last = sel.getLastRow()
  const { rows, sheetRows } = readInputRows(s, spec, ds)
  const picked = sheetRows.flatMap((sr, i) => (sr >= first && sr <= last ? [i] : []))
  if (!picked.length)
    return ElMessage.warning(tr('先在填报区的数据行里选中要删除的行（可以选多行）'))
  const keyCols = spec.keys.length ? spec.keys : ds.primaryKey
  const keyText = (i: number) => keyCols.map(k => String(rows[i]?.[spec.fields.indexOf(k)] ?? '')).join(' / ')
  const sample = picked.slice(0, 5).map(keyText).join('、')
  const ok = await ElMessageBox.confirm(
    tr('从数据集「{0}」删除选中的 {1} 行（{2}{3}）？表格里这些行也会一并删除。删除可以在提交记录里撤销。', [ds.name, picked.length, sample, picked.length > 5 ? ' 等' : '']),
    tr('删除数据集里的记录'),
    { type: 'warning', confirmButtonText: tr('删除'), confirmButtonClass: 'ec-button--danger' },
  ).then(() => true, () => false)
  if (!ok)
    return
  clearErrorMarks(api, spec.id)
  deleting.value = true
  try {
    const outcome = await writebackApi(ds.id, {
      mode: 'delete',
      columns: spec.fields,
      rows: picked.map(i => rows[i]!),
      keys: spec.keys,
      operator: props.operator,
      source: { workbookId: props.workbookId, sheetName: s.getSheetName(), inputName: spec.name },
    })
    if (outcome.errors.length) {
      result.value = { specId: spec.id, outcome, sheetRows }
      markErrors(api, s, spec, sheetRows, outcome.errors)
      ElMessage.error(tr('有 {0} 处不能删除（已标红），本次没有删除任何数据', [outcome.errorCount]))
      return
    }
    s.deleteRows(sheetRows[picked[0]!]!, picked.length)
    result.value = null
    ElMessage.success(`已从「${outcome.datasetName}」删除 ${outcome.deleted} 行${outcome.notFound ? `（另有 ${outcome.notFound} 行在数据集里本来就没有）` : ''}`)
    emit('changed')
    if (outcome.deleted)
      emit('datasetsChanged')
    loadRecords()
  }
  finally {
    deleting.value = false
  }
}

// ── 载入现有数据 ──
const loadForm = reactive({ open: false, column: '', op: 'in' as 'in' | 'prefix' | 'contains', value: '' })
const loadBusy = ref(false)

async function loadExisting() {
  const s = sheet.value
  const spec = current.value
  const ds = currentDataset.value
  if (!s || !spec || !ds)
    return
  const filters: DatasetFilter[] = []
  const v = loadForm.value.trim()
  if (loadForm.column && v) {
    if (loadForm.op === 'in')
      filters.push({ op: 'in', column: loadForm.column, values: v.split(/[,，、]/).map(x => x.trim()).filter(Boolean) })
    else
      filters.push({ op: loadForm.op, column: loadForm.column, value: v })
  }
  loadBusy.value = true
  try {
    const cursor = await openCursorApi(ds.id, { filters })
    if (!cursor.total)
      return ElMessage.info(tr('没有符合条件的数据'))
    if (cursor.total > LOAD_LIMIT) {
      const ok = await ElMessageBox.confirm(tr('符合条件的有 {0} 行，最多载入前 {1} 行。建议加筛选条件缩小范围。仍然载入？', [cursor.total.toLocaleString(), LOAD_LIMIT.toLocaleString()]), tr('载入现有数据'), { type: 'warning', confirmButtonText: tr('载入') }).then(() => true, () => false)
      if (!ok)
        return
    }
    if (dataRowCount(s, spec)) {
      const ok = await ElMessageBox.confirm(tr('载入会替换填报区里现有的内容（还没提交的修改会丢失）。继续？'), tr('载入现有数据'), { type: 'warning', confirmButtonText: tr('替换') }).then(() => true, () => false)
      if (!ok)
        return
    }
    const keys = spec.fields.map(f => ds.columns.find(c => c.title === f)?.key ?? f)
    const rows: CellValue[][] = []
    const total = Math.min(cursor.total, LOAD_LIMIT)
    while (rows.length < total) {
      const page = await getRowsApi(ds.id, cursor.cursorId, rows.length, Math.min(2000, total - rows.length), undefined, keys)
      if (page.expired || !page.rows.length)
        break
      rows.push(...page.rows)
    }
    clearErrorMarks(props.univerApi!, spec.id)
    writeInputRows(s, spec, rows)
    result.value = null
    loadForm.open = false
    emit('changed')
    ElMessage.success(tr('已载入 {0} 行，修改后点「提交」写回', [rows.length.toLocaleString()]))
  }
  finally {
    loadBusy.value = false
  }
}

async function clearRows() {
  const s = sheet.value
  const spec = current.value
  if (!s || !spec)
    return
  const n = dataRowCount(s, spec)
  if (!n)
    return
  const ok = await ElMessageBox.confirm(tr('清空填报区里的 {0} 行？只清空表格，不影响数据集。', [n]), tr('清空填报区'), { type: 'warning', confirmButtonText: tr('清空') }).then(() => true, () => false)
  if (!ok)
    return
  clearErrorMarks(props.univerApi!, spec.id)
  clearInputRows(s, spec)
  result.value = null
  emit('changed')
}

// ── 提交记录 ──
const records = ref<WritebackRecord[]>([])
async function loadRecords() {
  const id = current.value?.datasetId
  records.value = id ? await listWritebacksApi(id, 20).catch(() => []) : []
}
watch(() => current.value?.datasetId, loadRecords, { immediate: true })

async function undo(r: WritebackRecord) {
  const ok = await ElMessageBox.confirm(
    `撤销 ${r.createdAt} ${r.operator || ''} 的提交：${r.mode === 'delete' ? `把删除的 ${r.deleted} 行插回数据集` : `${r.inserted ? `删除新增的 ${r.inserted} 行` : ''}${r.inserted && r.updated ? '，' : ''}${r.updated ? `把更新的 ${r.updated} 行恢复为提交前的值` : ''}`}。工作表里的内容不会改变。`,
    tr('撤销提交'),
    { type: 'warning', confirmButtonText: tr('撤销') },
  ).then(() => true, () => false)
  if (!ok)
    return
  const u = await undoWritebackApi(r.id, props.operator)
  ElMessage.success(`已撤销：「${u.datasetName}」${u.reinserted ? `插回 ${u.reinserted} 行` : `删除 ${u.removed} 行、恢复 ${u.restored} 行`}`)
  emit('datasetsChanged')
  loadRecords()
}
</script>

<template>
  <div class="wb-panel">
    <div v-if="!sheet" class="text-gray-500">
      {{ $t('工作簿尚未就绪') }}
    </div>
    <template v-else>
      <!-- 当前页签的填报区 -->
      <div v-if="inputs.length" class="section">
        <div class="section-title">
          {{ $t('本页填报区') }}
        </div>
        <div class="flex flex-wrap gap-1.5">
          <span
            v-for="i in inputs"
            :key="i.id"
            class="tag"
            :class="{ active: i.id === currentId }"
            :title="$t('{0} 起 · {1}', [cellName(i.row, i.col), i.datasetName])"
            @click="currentId = i.id; sheet?.getRange(i.row, i.col).activate()"
          >{{ i.name }}</span>
        </div>
      </div>

      <div v-if="current && !creating" class="card">
        <div class="flex items-center gap-2">
          <span class="font-semibold">{{ current.name }}</span>
          <span class="text-xs text-gray-400">{{ $t('{0} 起', [cellName(current.row, current.col)]) }}</span>
          <el-button link size="small" type="danger" class="ml-auto" @click="removeCurrent">
            {{ $t('移除') }}
          </el-button>
        </div>
        <div class="grid grid-cols-[72px_1fr] gap-y-1 text-xs text-gray-600">
          <span>{{ $t('写入') }}</span>
          <span :class="{ 'text-[#B91C1C]': !currentDataset }">{{ current.datasetName }}{{ currentDataset ? '' : $t('（不存在或在回收站）') }}</span>
          <span>{{ $t('方式') }}</span>
          <span>{{ modeText(current.mode) }}<template v-if="current.mode === 'upsert'">{{ $t('（主键：{0}，深绿色表头）', [current.keys.join('、')]) }}</template></span>
          <span>{{ $t('列') }}</span>
          <span class="truncate" :title="current.fields.join('、')">{{ current.fields.join('、') }}</span>
          <template v-if="current.last">
            <span>{{ $t('上次提交') }}</span>
            <span>{{ $t('{0} {1} · 新增 {2} / 更新 {3}', [current.last.at, current.last.by, current.last.inserted, current.last.updated]) }}</span>
          </template>
        </div>
        <div class="flex items-center gap-2">
          <el-button type="primary" size="small" :loading="submitting" :disabled="!currentDataset" @click="submit">
            {{ $t('提交到数据集') }}
          </el-button>
          <el-button size="small" :disabled="!currentDataset" @click="loadForm.open = !loadForm.open">
            {{ $t('载入现有数据') }}
          </el-button>
          <el-button size="small" link @click="clearRows">
            {{ $t('清空') }}
          </el-button>
          <el-button v-if="current.mode === 'upsert'" size="small" link type="danger" :loading="deleting" :disabled="!currentDataset" :title="$t('选中填报区里的行，同时从数据集删除这些主键对应的记录')" @click="deleteSelected">
            {{ $t('删除选中的行') }}
          </el-button>
        </div>

        <!-- 载入现有数据 -->
        <div v-if="loadForm.open && currentDataset" class="sub">
          <div class="text-xs text-gray-500">
            {{ $t('把数据集里已有的行载入填报区，修改后再提交（如调整已录入的预算）。不填条件则载入全部（最多 {0} 行）', [LOAD_LIMIT.toLocaleString()]) }}
          </div>
          <div class="flex items-center gap-1.5">
            <el-select v-model="loadForm.column" size="small" clearable :placeholder="$t('筛选列（可选）')" class="flex-1">
              <el-option v-for="c in currentDataset.columns" :key="c.key" :label="c.title" :value="c.key" />
            </el-select>
            <el-select v-model="loadForm.op" size="small" class="!w-24">
              <el-option :label="$t('等于')" value="in" />
              <el-option :label="$t('开头是')" value="prefix" />
              <el-option :label="$t('包含')" value="contains" />
            </el-select>
          </div>
          <el-input v-model="loadForm.value" size="small" :disabled="!loadForm.column" :placeholder="$t('值（等于时多个值用逗号分隔）')" />
          <div class="flex justify-end">
            <el-button size="small" type="primary" plain :loading="loadBusy" @click="loadExisting">
              {{ $t('载入') }}
            </el-button>
          </div>
        </div>

        <!-- 提交结果 -->
        <div v-if="currentResult?.outcome.errors.length" class="errors">
          <div class="mb-1 text-xs text-[#B91C1C]">
            {{ $t('{0} 处需要修改，本次没有写入任何数据（点击定位）：', [currentResult.outcome.errorCount]) }}
          </div>
          <div class="max-h-48 overflow-auto">
            <div
              v-for="(e, i) in currentResult.outcome.errors"
              :key="i"
              class="err"
              @click="gotoError(e.row, e.column)"
            >
              <span class="shrink-0 text-gray-500">{{ cellName(currentResult.sheetRows[e.row] ?? 0, current.col + Math.max(current.fields.indexOf(e.column), 0)) }}</span>
              <span>{{ e.message }}</span>
            </div>
          </div>
          <div v-if="currentResult.outcome.errorCount > currentResult.outcome.errors.length" class="text-xs text-gray-400">
            {{ $t('只列出前 {0} 处', [currentResult.outcome.errors.length]) }}
          </div>
        </div>
      </div>

      <!-- 新建 -->
      <template v-if="!creating">
        <el-button v-if="!anchorInput" size="small" :type="inputs.length ? 'default' : 'primary'" @click="startCreate">
          {{ $t('在 {0} 新建填报区', [cellName(anchor.row, anchor.col)]) }}
        </el-button>
        <div v-else class="text-xs text-gray-400">
          {{ $t('要再放一个填报区，先在表格里点选一个空白位置') }}
        </div>
      </template>
      <WritebackCreateForm v-else v-model="form" :writable="writable" :anchor="anchor" :busy="createBusy" :form-dataset="formDataset" :form-columns="formColumns" :form-has-pk="formHasPk" :new-titles="newTitles" @cancel="creating = false" @create="create" />

      <!-- 提交记录 -->
      <WritebackRecords v-if="current && records.length" :dataset-name="current.datasetName" :records="records" @refresh="loadRecords" @undo="undo" />

      <div v-if="ledgerUser.restricted" class="rounded bg-[#FFF7E6] px-2 py-1.5 text-xs text-[#92400E] leading-5">
        {{ $t('你的数据范围限定在{0}。只能填报按某一列划分范围的数据集，填报区必须包含范围列，并且只能新增、修改、删除自己范围内的行。', [ledgerUser.scopeText]) }}
      </div>
      <div class="text-xs text-gray-400 leading-5">
        {{ $t('在表头下方逐行填写（可以用公式），遇到整行空白即结束。提交时整批校验：数字、日期格式不对或主键为空 / 重复时会标红，一行都不写入。只有最近一次提交、且之后数据集没有别的改动时可以撤销。') }}
      </div>
    </template>
  </div>
</template>

<style scoped lang="scss">
.wb-panel {
  @apply flex flex-col gap-3 text-[13px];
}

.section {
  @apply flex flex-col gap-1.5;
}

.section-title {
  @apply flex items-center justify-between text-xs font-semibold text-gray-600;
}

.card {
  @apply flex flex-col gap-2 rounded border border-[#e5e7eb] border-solid p-2.5;
}

.sub {
  @apply flex flex-col gap-1.5 rounded bg-[#F9FAFB] p-2;
}

.tag {
  @apply cursor-pointer rounded border border-[#d1d5db] border-solid px-2 py-0.5 text-xs hover:border-[#217346];

  &.active {
    @apply border-[#217346] bg-[#E7F4EC] text-[#0E5A31];
  }
}

.errors {
  @apply rounded bg-[#FEF2F2] p-2;
}

.err {
  @apply flex cursor-pointer gap-2 rounded px-1 py-0.5 text-xs hover:bg-[#FDE2E2];
}

.rec {
  @apply flex flex-col gap-0.5 border-b border-[#f0f0f0] border-b-solid py-1.5 text-xs last:border-b-0;

  &.undone {
    @apply opacity-60;
  }
}

.remove {
  @apply cursor-pointer px-1 text-gray-400 hover:text-[#B91C1C];
}
</style>
