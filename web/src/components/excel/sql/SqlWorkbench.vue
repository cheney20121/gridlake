<script setup lang="ts">
import type { SaveForm } from './SaveQueryDialog.vue'
import type { Connector } from '@/api/connector'
import type { DatasetInfo } from '@/api/dataset'
import type { ParamDef, SavedQuery, SqlResult, SqlSchedule } from '@/api/sql'
import type { BuilderModel } from '@/features/excel/sql/builder'
import { computed, nextTick, reactive, ref, watch } from 'vue'
import { listConnectorsApi } from '@/api/connector'
import { createQueryApi, deleteQueryApi, listQueriesApi, refreshQueryApi, runSqlApi, updateQueryApi } from '@/api/sql'
import { buildSql, emptyModel, q } from '@/features/excel/sql/builder'
import { cleanValues, paramProblems, placeholderNames, syncDefs } from '@/features/excel/sql/params'
import { t } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'
import ParamInstanceDialog from './ParamInstanceDialog.vue'
import QueryBuilder from './QueryBuilder.vue'
import ResultGrid from './ResultGrid.vue'
import SaveQueryDialog from './SaveQueryDialog.vue'
import SqlEditor from './SqlEditor.vue'
import SqlParamBar from './SqlParamBar.vue'
import SqlSidebar from './SqlSidebar.vue'

/** SQL 查询工作台：写只读 SQL 或用构建器生成，查看结果；保存查询、另存为数据集并定时重算 */
const props = defineProps<{
  visible: boolean
  datasets: DatasetInfo[]
  operator?: string
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 生成 / 重算了数据集：刷新数据集列表 */
  (e: 'datasetsChanged', id?: string): void
  /** 打开数据集 */
  (e: 'openDataset', id: string): void
}>()

const EXAMPLE = '-- 表名是数据集名称，列名是中文列名；只能查询（SELECT / WITH），不会改动数据\n-- Ctrl / ⌘ + Enter 运行\n'

const sql = ref('')
const mode = ref<'sql' | 'builder'>('sql')
const result = ref<SqlResult | null>(null)
const error = ref('')
const running = ref(false)
const queries = ref<SavedQuery[]>([])
const current = ref<SavedQuery | null>(null)
const keyword = ref('')
const editorRef = ref<InstanceType<typeof SqlEditor>>()
const refreshing = ref(false)
let controller: AbortController | null = null
function stopRun() {
  controller?.abort()
}

// 参数化查询：SQL 里的 {{名称}} 对应的参数定义与当前值
const paramDefs = ref<ParamDef[]>([])
const paramValues = ref<Record<string, string>>({})
watch(sql, (s) => {
  const names = placeholderNames(s)
  if (names.join('\n') !== paramDefs.value.map(d => d.name).join('\n'))
    paramDefs.value = syncDefs(names, paramDefs.value)
})
/** 带上默认值（保存时把当前值作为默认值，下次打开就有） */
const defsWithDefaults = () => paramDefs.value.map(d => ({ ...d, default: paramValues.value[d.name]?.trim() || d.default || null }))

const instanceVisible = ref(false)

const model = reactive<BuilderModel>(emptyModel())
const builderDs = computed(() => props.datasets.find(d => d.name === model.dataset))
const builderSql = computed(() => buildSql(model, builderDs.value))

const shownDatasets = computed(() => {
  const k = keyword.value.trim().toLowerCase()
  return k ? props.datasets.filter(d => d.name.toLowerCase().includes(k) || d.columns.some(c => c.title.toLowerCase().includes(k))) : props.datasets
})

// 数据范围受限的用户只能运行查询：不能保存查询、另存为数据集（服务端会拒绝），也看不到别人保存的查询列表
const ledgerUser = useLedgerUserStore()

async function loadQueries() {
  if (ledgerUser.restricted) {
    queries.value = []
    return
  }
  queries.value = await listQueriesApi().catch(() => [])
  if (current.value)
    current.value = queries.value.find(x => x.id === current.value!.id) ?? null
}

watch(() => props.visible, (v) => {
  if (!v)
    return
  loadQueries()
  if (!sql.value) {
    const first = props.datasets.find(d => !d.builtin) ?? props.datasets[0]
    sql.value = first ? `${EXAMPLE}SELECT *\nFROM ${q(first.name)}\nLIMIT 100` : EXAMPLE
  }
}, { immediate: true })

/** 在光标处插入文字（数据集侧栏点名称） */
async function insert(text: string) {
  if (mode.value !== 'sql') {
    mode.value = 'sql'
    await nextTick()
  }
  if (editorRef.value)
    editorRef.value.insertAtCaret(text)
  else
    sql.value += text
}

async function run(text?: string) {
  const s = (text ?? sql.value).trim()
  if (!s || running.value)
    return
  const problems = paramProblems(paramDefs.value, paramValues.value)
  if (problems.length) {
    error.value = problems.join('；')
    result.value = null
    return
  }
  controller?.abort()
  controller = new AbortController()
  running.value = true
  error.value = ''
  try {
    result.value = await runSqlApi(s, controller.signal, paramDefs.value.length ? paramDefs.value : undefined, paramDefs.value.length ? cleanValues(paramDefs.value, paramValues.value) : undefined)
  }
  catch (e) {
    if (!controller.signal.aborted) {
      error.value = e instanceof Error ? e.message : String(e)
      result.value = null
    }
  }
  finally {
    running.value = false
  }
}

function runBuilder() {
  if (!builderSql.value)
    return ElMessage.warning(t('请先选择数据集'))
  sql.value = builderSql.value
  mode.value = 'sql'
  run(builderSql.value)
}

function openQuery(sq: SavedQuery) {
  current.value = sq
  sql.value = sq.sql
  // 先放定义再放值：watch(sql) 会按占位符同步一次，已有的类型保留
  paramDefs.value = sq.params.map(p => ({ ...p }))
  paramValues.value = { ...sq.values }
  mode.value = 'sql'
  result.value = null
  error.value = ''
  run(sq.sql)
}

function newQuery() {
  current.value = null
  paramDefs.value = []
  paramValues.value = {}
  result.value = null
  error.value = ''
  sql.value = EXAMPLE
}

// ── 构建器 ──

// ── 保存 ──

const saveVisible = ref(false)
const saving = ref(false)
const connectors = ref<Connector[]>([])
const form = reactive<SaveForm>({
  name: '',
  description: '',
  asDataset: false,
  datasetName: '',
  kinds: {},
  scheduleKind: 'manual',
  at: '06:00',
  connectorId: '',
})

async function openSave(asNew = false) {
  if (!sql.value.trim())
    return
  if (!result.value) {
    await run()
    if (!result.value)
      return ElMessage.warning(t('请先修正查询中的错误'))
  }
  const c = asNew ? null : current.value
  Object.assign(form, {
    name: c?.name ?? '',
    description: c?.description ?? '',
    asDataset: !!c?.datasetId,
    datasetName: c?.datasetName ?? '',
    kinds: Object.fromEntries(result.value!.columns.map(col => [col.name, c?.columnKinds[col.name] ?? col.kind])),
    scheduleKind: c?.schedule.kind ?? 'manual',
    at: c?.schedule.kind === 'daily' ? c.schedule.at : '06:00',
    connectorId: c?.schedule.kind === 'after_sync' ? c.schedule.connectorId : '',
  })
  if (asNew)
    current.value = null
  saveVisible.value = true
  connectors.value = (await listConnectorsApi().catch(() => ({ items: [] as Connector[] }))).items
}

async function save() {
  if (!form.name.trim())
    return ElMessage.warning(t('请填写查询名称'))
  if (form.asDataset && !current.value?.datasetId && !form.datasetName.trim())
    return ElMessage.warning(t('请填写数据集名称'))
  if (form.scheduleKind === 'after_sync' && !form.connectorId)
    return ElMessage.warning(t('请选择数据连接'))
  const schedule: SqlSchedule = form.scheduleKind === 'daily' ? { kind: 'daily', at: form.at } : form.scheduleKind === 'after_sync' ? { kind: 'after_sync', connectorId: form.connectorId } : { kind: 'manual' }
  // 只提交与自动推断不同的类型
  const kinds = Object.fromEntries(Object.entries(form.kinds).filter(([n, k]) => result.value?.columns.find(c => c.name === n)?.kind !== k))
  const input = {
    name: form.name.trim(),
    sql: sql.value,
    description: form.description.trim(),
    saveAsDataset: form.asDataset && !current.value?.datasetId ? form.datasetName.trim() : null,
    columnKinds: form.asDataset ? kinds : {},
    schedule: form.asDataset ? schedule : { kind: 'manual' } as SqlSchedule,
    operator: props.operator,
    params: paramDefs.value.length ? defsWithDefaults() : [],
    values: cleanValues(paramDefs.value, paramValues.value),
  }
  saving.value = true
  try {
    const sq = current.value ? await updateQueryApi(current.value.id, input) : await createQueryApi(input)
    current.value = sq
    saveVisible.value = false
    await loadQueries()
    if (sq.datasetId) {
      emit('datasetsChanged', sq.datasetId)
      ElMessage.success(`已保存，并写入数据集「${sq.datasetName}」（${sq.lastRows?.toLocaleString()} 行）${sq.lastMessage ? `：${sq.lastMessage}` : ''}`)
    }
    else {
      ElMessage.success(t('已保存'))
    }
  }
  finally {
    saving.value = false
  }
}

async function refresh(sq: SavedQuery) {
  refreshing.value = true
  try {
    const r = await refreshQueryApi(sq.id)
    await loadQueries()
    emit('datasetsChanged', r.datasetId)
    ElMessage.success(t('「{0}」已重算：{1} 行{2}', [r.datasetName, r.rows.toLocaleString(), r.rebuilt ? '（结构有变化，已重建）' : '']))
  }
  finally {
    refreshing.value = false
  }
}

async function remove(sq: SavedQuery) {
  const ok = await ElMessageBox.confirm(`删除查询「${sq.name}」？${sq.datasetName ? `已生成的数据集「${sq.datasetName}」会保留，但不再自动重算。` : ''}`, '删除查询', { type: 'warning', confirmButtonText: t('删除') }).then(() => true, () => false)
  if (!ok)
    return
  await deleteQueryApi(sq.id)
  if (current.value?.id === sq.id)
    current.value = null
  loadQueries()
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('SQL 查询')"
    width="94vw"
    top="3vh"
    class="sql-dialog"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="flex gap-3" style="height: 82vh">
      <SqlSidebar v-model:keyword="keyword" :datasets="shownDatasets" :queries="queries" :current="current" @insert="insert" @open="openQuery" @new="newQuery" />

      <!-- 右：编辑器与结果 -->
      <div class="min-w-0 flex flex-1 flex-col gap-2">
        <div class="flex items-center gap-2">
          <el-radio-group v-model="mode" size="small">
            <el-radio-button value="sql">
              SQL
            </el-radio-button>
            <el-radio-button value="builder">
              {{ $t('查询构建器') }}
            </el-radio-button>
          </el-radio-group>
          <span v-if="current" class="min-w-0 truncate text-xs text-gray-500">
            {{ $t('当前：') }}<b class="text-gray-700">{{ current.name }}</b>
            <template v-if="current.datasetName">
              → <el-button link type="primary" size="small" class="!p-0 !text-xs" @click="emit('openDataset', current.datasetId!)">{{ current.datasetName }}</el-button>
              <span v-if="current.lastRunAt"> {{ $t('· 上次重算 {0}{1}', [current.lastRunAt.slice(5, 16), current.lastStatus === 'failed' ? $t('（失败）') : '']) }}</span>
              <span v-if="current.nextRun"> {{ $t('· 下次 {0}', [current.nextRun.slice(5, 16)]) }}</span>
            </template>
          </span>
          <span class="flex-1" />
          <template v-if="current">
            <el-button v-if="current.datasetId" size="small" :loading="refreshing" @click="refresh(current)">
              {{ $t('立即重算') }}
            </el-button>
            <el-button v-if="current.params.length && !current.templateId" size="small" :title="$t('用另一组参数值运行，把结果存成新的数据集')" @click="instanceVisible = true">
              {{ $t('生成实例…') }}
            </el-button>
            <el-button size="small" text type="danger" @click="remove(current)">
              {{ $t('删除') }}
            </el-button>
          </template>
          <el-button v-if="!ledgerUser.restricted" size="small" @click="openSave(false)">
            {{ current ? $t('保存') : $t('保存查询…') }}
          </el-button>
          <el-button v-if="current && !ledgerUser.restricted" size="small" @click="openSave(true)">
            {{ $t('另存为…') }}
          </el-button>
          <el-button v-if="running" size="small" type="danger" plain @click="stopRun">
            {{ $t('停止') }}
          </el-button>
          <el-button v-else size="small" type="primary" @click="mode === 'builder' ? runBuilder() : run()">
            {{ $t('运行') }} <span class="ml-1 text-xs opacity-70">⌘↵</span>
          </el-button>
        </div>

        <SqlParamBar v-if="paramDefs.length" v-model:values="paramValues" :defs="paramDefs" />
        <SqlEditor v-if="mode === 'sql'" ref="editorRef" v-model="sql" :datasets="datasets" @run="run()" />
        <QueryBuilder v-else v-model="model" :datasets="datasets" :builder-sql="builderSql" />

        <ResultGrid :result="result" :error="error" :running="running" />
      </div>
    </div>

    <ParamInstanceDialog v-model:visible="instanceVisible" :query="current" @created="(q: SavedQuery) => { loadQueries(); emit('datasetsChanged', q.datasetId ?? undefined) }" />
    <SaveQueryDialog v-model:visible="saveVisible" v-model:form="form" :current="current" :connectors="connectors" :saving="saving" @save="save" />
  </el-dialog>
</template>
