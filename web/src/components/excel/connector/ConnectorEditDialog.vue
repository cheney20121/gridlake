<script setup lang="ts">
import type { Step } from './edit/form-state'
import type { Connector, ConnectorInput, ConnectorPreview, ConnectorSchedule, ConnectorTestResult } from '@/api/connector'
import type { DatasetInfo } from '@/api/dataset'
import { computed, reactive, ref, watch } from 'vue'
import { createConnectorApi, previewConnectorApi, testConnectorApi, updateConnectorApi } from '@/api/connector'
import { t as tr } from '@/i18n'
import { buildSource, DEFAULT_PORT, defaultForm, formFromConnector, hostOf, usedPlaceholders } from './edit/form-state'
import SourceDatabaseForm from './edit/SourceDatabaseForm.vue'
import SourceFolderForm from './edit/SourceFolderForm.vue'
import SourceHttpForm from './edit/SourceHttpForm.vue'
import SourceS3Form from './edit/SourceS3Form.vue'
import SourceTestResult from './edit/SourceTestResult.vue'
import StepSchedule from './edit/StepSchedule.vue'
import StepTarget from './edit/StepTarget.vue'

/** 新建 / 修改数据连接器：① 来源 ② 预览与目标 ③ 同步计划（各步骤的表单在 ./edit 下） */
const props = defineProps<{
  visible: boolean
  /** 修改的连接器；为空时新建 */
  connector: Connector | null
  datasets: DatasetInfo[]
  inboxRoots: string[]
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'saved', id: string, runNow: boolean): void
}>()

const step = ref<Step>(0)
const testing = ref(false)
const previewing = ref(false)
const saving = ref(false)
const testResult = ref<ConnectorTestResult | null>(null)
const preview = ref<ConnectorPreview | null>(null)
/** 预览对应的来源（来源改了需要重新预览） */
const previewKey = ref('')

const form = reactive(defaultForm())

const editing = computed(() => !!props.connector)
const isDb = computed(() => form.sourceKind === 'mysql' || form.sourceKind === 'postgres' || form.sourceKind === 'clickhouse')
const isHttp = computed(() => form.sourceKind === 'http')
const userDatasets = computed(() => props.datasets.filter(d => d.kind === 'table'))
const targetDataset = computed(() => userDatasets.value.find(d => d.id === form.datasetId))

watch(() => props.visible, (v) => {
  if (!v)
    return
  step.value = 0
  testResult.value = null
  preview.value = null
  previewKey.value = ''
  Object.assign(form, props.connector ? formFromConnector(props.connector) : defaultForm())
})

// 切换数据库类型时带出默认端口
watch(() => form.sourceKind, (k, old) => {
  const known = Object.values(DEFAULT_PORT)
  if (DEFAULT_PORT[k] && (!form.db.port || (old !== k && known.includes(form.db.port))))
    form.db.port = DEFAULT_PORT[k]!
  // ClickHouse 只有 HTTP / HTTPS 两种
  if (k === 'clickhouse' && form.db.ssl === 'prefer')
    form.db.ssl = 'disable'
  testResult.value = null
})

const source = computed(() => buildSource(form))

/** 预览只与取数相关的字段有关（增量列 / 字段不影响预览） */
const sourceKey = computed(() => JSON.stringify({ ...source.value, incrementalColumn: undefined, incrementalField: undefined }))

const sourceReady = computed(() => {
  if (form.sourceKind === 'folder')
    return !!form.folder.path.trim() && !!form.folder.pattern.trim()
  if (form.sourceKind === 's3') {
    const s = form.s3
    return !!s.bucket.trim() && !!s.accessKey.trim() && !!s.pattern.trim() && (!!s.secretKey || (editing.value && !!props.connector?.hasPassword))
  }
  if (form.sourceKind === 'http') {
    const h = form.http
    const secretOk = h.authKind === 'none' || !!h.secret || (editing.value && !!props.connector?.hasPassword)
    const doudianOk = h.authKind !== 'doudian' || (!!h.appKey.trim() && /^\d+$/.test(h.shopId.trim()) && h.method === 'post')
    return /^https?:\/\/\S+/i.test(h.url.trim()) && secretOk && doudianOk
  }
  return !!form.db.host.trim() && !!form.db.database.trim() && !!form.db.user.trim() && !!form.db.sql.trim() && (editing.value || !!form.db.password)
})

const placeholders = computed(() => usedPlaceholders(form.http))

async function runTest() {
  testing.value = true
  testResult.value = null
  try {
    testResult.value = await testConnectorApi(source.value, props.connector?.id)
  }
  finally {
    testing.value = false
  }
}

async function loadPreview() {
  previewing.value = true
  try {
    const p = await previewConnectorApi(source.value, props.connector?.id)
    preview.value = p
    previewKey.value = sourceKey.value
    // 新建目标：按预览给出列（修改时保留原来的列名 / 类型设置）
    const old = new Map(form.columns.map(c => [c.source, c]))
    form.columns = p.columns.map((c) => {
      const o = old.get(c.source)
      return { source: c.source, title: o?.title ?? c.title, kind: o?.kind ?? c.kind, include: o?.include ?? c.fillRate > 0, samples: c.samples }
    })
    if (!form.datasetName)
      form.datasetName = form.name || (p.sampleOf.replace(/\.[^.]+$/, '').replace(/[_\-\s]*\d{4}.*$/, '').trim())
    // 列完全对得上的已有数据集：默认同步到它
    const best = p.matches.find(m => m.matched === m.total)
    if (best && !editing.value) {
      form.targetType = 'existing'
      form.datasetId = best.datasetId
    }
  }
  finally {
    previewing.value = false
  }
}

async function next() {
  if (step.value === 0) {
    if (!form.name.trim()) {
      form.name = form.sourceKind === 'folder'
        ? (form.folder.path.split(/[\\/]/).filter(Boolean).pop() || '文件夹同步')
        : form.sourceKind === 's3'
          ? `${form.s3.bucket.trim()} 对象存储`
          : form.sourceKind === 'http' ? `${hostOf(form.http.url)} 接口` : `${form.db.database} 同步`
    }
    step.value = 1
    if (previewKey.value !== sourceKey.value)
      await loadPreview().catch(() => {})
    return
  }
  if (step.value === 1)
    step.value = 2
}

const includedTitles = computed(() => form.columns.filter(c => c.include).map(c => c.title.trim()))
const duplicateTitle = computed(() => includedTitles.value.find((t, i) => !t || includedTitles.value.indexOf(t) !== i))
/** 主键可选的列：新建为要导入的列，已有为数据集的列 */
const keyOptions = computed(() => form.targetType === 'new' ? includedTitles.value : (targetDataset.value?.columns.filter(c => !c.computed).map(c => c.title) ?? []))
const sourceColumns = computed(() => preview.value?.columns.map(c => c.source) ?? (testResult.value?.database?.columns ?? testResult.value?.http?.fields ?? []))

watch(keyOptions, (opts) => {
  form.primaryKey = form.primaryKey.filter(k => opts.includes(k))
})
// 选择已有数据集时沿用它的主键
watch(() => form.datasetId, () => {
  if (form.targetType === 'existing' && targetDataset.value && !form.primaryKey.length)
    form.primaryKey = [...targetDataset.value.primaryKey]
})

const targetMatch = computed(() => preview.value?.matches.find(m => m.datasetId === form.datasetId))

const needsKey = computed(() => form.mode === 'upsert' || (isDb.value && !!form.db.incrementalColumn) || (isHttp.value && !!form.http.incrementalField))
const targetError = computed(() => {
  if (form.targetType === 'new') {
    if (!form.datasetName.trim())
      return '请填写数据集名称'
    if (!form.columns.length)
      return '请先预览数据，确定要导入的列'
    if (duplicateTitle.value !== undefined)
      return `列名不能为空或重复：「${duplicateTitle.value}」`
    if (props.datasets.some(d => d.name === form.datasetName.trim()))
      return `已存在名为「${form.datasetName.trim()}」的数据集，要同步到它请选择「已有数据集」`
  }
  else if (!targetDataset.value) {
    return '请选择目标数据集'
  }
  if (needsKey.value && !form.primaryKey.length)
    return form.mode === 'upsert' ? '「增量更新」需要设置主键' : '增量同步需要设置主键（用于去重重复读到的行）'
  return ''
})

const schedule = computed<ConnectorSchedule>(() => form.schedule === 'interval'
  ? { kind: 'interval', minutes: Number(form.minutes) }
  : form.schedule === 'daily' ? { kind: 'daily', at: form.at } : { kind: 'manual' })

async function save() {
  const input: ConnectorInput = {
    name: form.name.trim(),
    source: source.value,
    target: {
      datasetId: form.targetType === 'existing' ? form.datasetId : null,
      name: form.targetType === 'new' ? form.datasetName.trim() : null,
      mode: form.mode,
      primaryKey: form.primaryKey,
      columns: form.targetType === 'new' ? form.columns.map(c => ({ source: c.source, title: c.title.trim(), kind: c.kind, include: c.include })) : [],
    },
    schedule: schedule.value,
    enabled: form.enabled,
  }
  if (form.targetType === 'existing' && form.mode === 'replace' && targetDataset.value) {
    const ok = await ElMessageBox.confirm(
      tr('「全量替换」每次同步都会先删除数据集「{0}」的全部数据（当前 {1} 行），再写入来源的最新数据。', [targetDataset.value.name, targetDataset.value.rowCount.toLocaleString()]),
      tr('全量替换已有数据集'),
      { type: 'warning', confirmButtonText: tr('确定') },
    ).then(() => true, () => false)
    if (!ok)
      return
  }
  saving.value = true
  try {
    let id = props.connector?.id
    if (id) {
      await updateConnectorApi(id, input)
    }
    else {
      id = await createConnectorApi(input)
    }
    ElMessage.success(editing.value ? tr('已保存') : tr('已创建连接器'))
    emit('saved', id, form.runNow)
    emit('update:visible', false)
  }
  finally {
    saving.value = false
  }
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="editing ? $t('修改连接器 · {0}', [connector?.name]) : $t('新建数据连接')"
    width="920px"
    :close-on-click-modal="false"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <el-steps :active="step" finish-status="success" simple class="mb-4">
      <el-step :title="$t('数据来源')" />
      <el-step :title="$t('预览与目标')" />
      <el-step :title="$t('同步计划')" />
    </el-steps>

    <!-- ① 来源 -->
    <div v-show="step === 0" class="flex flex-col gap-3 text-[13px]">
      <div class="row">
        <span class="label">{{ $t('名称') }}</span>
        <el-input v-model="form.name" class="!w-72" :placeholder="$t('如：ERP 订单、抖店资金流水收件')" maxlength="40" />
      </div>
      <div class="row">
        <span class="label">{{ $t('来源类型') }}</span>
        <el-radio-group v-model="form.sourceKind">
          <el-radio-button value="folder">
            {{ $t('文件夹') }}
          </el-radio-button>
          <el-radio-button value="s3">
            {{ $t('对象存储 (S3)') }}
          </el-radio-button>
          <el-radio-button value="mysql">
            MySQL / MariaDB
          </el-radio-button>
          <el-radio-button value="postgres">
            PostgreSQL
          </el-radio-button>
          <el-radio-button value="clickhouse">
            ClickHouse
          </el-radio-button>
          <el-radio-button value="http">
            {{ $t('HTTP 接口') }}
          </el-radio-button>
        </el-radio-group>
      </div>

      <SourceFolderForm v-if="form.sourceKind === 'folder'" v-model="form.folder" :inbox-roots="inboxRoots" />
      <SourceS3Form v-else-if="form.sourceKind === 's3'" v-model="form.s3" :editing="editing" :has-password="!!connector?.hasPassword" />
      <SourceHttpForm v-else-if="isHttp" v-model="form.http" :editing="editing" :has-password="!!connector?.hasPassword" />
      <SourceDatabaseForm v-else v-model="form.db" :engine="form.sourceKind" :editing="editing" :has-password="!!connector?.hasPassword" />

      <div class="row">
        <span class="label" />
        <el-button :disabled="!sourceReady" :loading="testing" @click="runTest">
          {{ $t('测试连接') }}
        </el-button>
        <span v-if="testResult" class="text-[#0B5A2E]">✓ {{ testResult.message }}</span>
      </div>
      <SourceTestResult :result="testResult" />
    </div>

    <!-- ② 预览与目标 -->
    <StepTarget
      v-show="step === 1"
      v-model="form"
      :preview="preview"
      :previewing="previewing"
      :user-datasets="userDatasets"
      :target-dataset="targetDataset"
      :target-match="targetMatch"
      :key-options="keyOptions"
      :source-columns="sourceColumns"
      :needs-key="needsKey"
      :is-db="isDb"
      :is-http="isHttp"
      :placeholders="placeholders"
      :editing="editing"
      :watermark="connector?.watermark"
      :target-error="targetError"
      @reload="loadPreview"
    />

    <!-- ③ 计划 -->
    <StepSchedule v-show="step === 2" v-model="form" :target-dataset="targetDataset" />

    <template #footer>
      <el-button v-if="step > 0" @click="step = (step - 1) as Step">
        {{ $t('上一步') }}
      </el-button>
      <el-button @click="emit('update:visible', false)">
        {{ $t('取消') }}
      </el-button>
      <el-button v-if="step < 2" type="primary" :disabled="(step === 0 && !sourceReady) || (step === 1 && !!targetError)" @click="next">
        {{ $t('下一步') }}
      </el-button>
      <el-button v-else type="primary" :loading="saving" :disabled="!form.name.trim() || !!targetError" @click="save">
        {{ editing ? $t('保存') : $t('创建') }}
      </el-button>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.row {
  @apply flex items-center gap-3;
}

.label {
  @apply w-20 shrink-0 text-gray-500;
}
</style>
