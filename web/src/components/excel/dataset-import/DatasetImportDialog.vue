<script setup lang="ts">
import type { DatasetColumnKind, DatasetInfo, ImportMode, ImportOutcome, ImportPreview } from '@/api/dataset'
import { computed, reactive, ref, watch } from 'vue'
import { commitDatasetImportApi, previewDatasetImportApi, previewDatasetImportUrlApi } from '@/api/dataset'
import BusyInline from '@/components/excel/common/BusyInline.vue'
import { errorText } from '@/utils/busy-hint'

/** 通用数据导入：任意 CSV / XLSX → 数据集（大数据表 + DSUMIFS 公式可用） */
const props = defineProps<{
  visible: boolean
  datasets: DatasetInfo[]
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 导入完成；open 为 true 时打开该数据集 */
  (e: 'imported', datasetId: string, open: boolean): void
}>()

const KIND_OPTIONS: { value: DatasetColumnKind, label: string, hint: string }[] = [
  { value: 'text', label: '文本', hint: '名称、状态等' },
  { value: 'id', label: '编号', hint: '订单号、手机号：保留前导零与全部位数' },
  { value: 'money', label: '金额', hint: '两位小数，可求和' },
  { value: 'number', label: '数值', hint: '数量、次数，可求和' },
  { value: 'datetime', label: '日期时间', hint: '统一为 YYYY-MM-DD HH:MM:SS' },
]

const step = ref<'pick' | 'map' | 'done'>('pick')
const uploading = ref(false)
const committing = ref(false)
const preview = ref<ImportPreview | null>(null)
const outcome = ref<ImportOutcome | null>(null)
/** 上传进度（字节）；全部传完后服务器还要识别表头与列类型，那一段没有进度 */
const progress = ref<{ loaded: number, total: number } | null>(null)
const uploadError = ref('')
let uploadController: AbortController | null = null
const dragging = ref(false)

interface EditColumn {
  source: string
  title: string
  kind: DatasetColumnKind
  include: boolean
  fillRate: number
  samples: string[]
}

const form = reactive({
  mode: 'create' as ImportMode,
  name: '',
  targetId: '',
  columns: [] as EditColumn[],
  primaryKey: [] as string[],
})

watch(() => props.visible, (v) => {
  if (v) {
    step.value = 'pick'
    preview.value = null
    outcome.value = null
    uploadError.value = ''
    progress.value = null
  }
  else {
    uploadController?.abort()
  }
})

const userDatasets = computed(() => props.datasets.filter(d => !d.builtin))

function defaultName(fileName: string) {
  return fileName.replace(/\.[^.]+$/, '').replace(/[_\-\s]*\d{4}.*$/, '').trim() || fileName.replace(/\.[^.]+$/, '')
}

const urlInput = ref('')

const ACCEPT = /\.(?:csv|txt|tsv|xls|xlsx|parquet|json|jsonl|ndjson)$/i

/** 选择文件（可多选：同结构的文件按列名对齐合并为一个数据集） */
async function onFile(e: Event) {
  const files = [...((e.target as HTMLInputElement).files ?? [])]
  ;(e.target as HTMLInputElement).value = ''
  await startUpload(files)
}

/** 拖进来的文件：只收支持的类型，其余的明确告诉用户被忽略了 */
async function onDrop(e: DragEvent) {
  dragging.value = false
  if (uploading.value)
    return
  const all = [...(e.dataTransfer?.files ?? [])]
  const files = all.filter(f => ACCEPT.test(f.name))
  if (all.length > files.length)
    ElMessage.warning(`已忽略 ${all.length - files.length} 个不支持的文件（支持 CSV / XLS / XLSX / Parquet / JSON）`)
  await startUpload(files)
}

async function startUpload(files: File[]) {
  if (!files.length)
    return
  uploadError.value = ''
  progress.value = { loaded: 0, total: files.reduce((a, f) => a + f.size, 0) }
  const c = new AbortController()
  uploadController = c
  try {
    await runPreview(() => previewDatasetImportApi(files, { signal: c.signal, onProgress: (loaded, total) => (progress.value = { loaded, total: total || progress.value?.total || 0 }) }), files[0]!.name)
  }
  catch (e) {
    if (!c.signal.aborted)
      uploadError.value = errorText(e, '文件读取失败')
  }
  finally {
    progress.value = null
  }
}

function cancelUpload() {
  uploadController?.abort()
}

async function onUrl() {
  const link = urlInput.value.trim()
  if (!/^https?:\/\//i.test(link)) {
    ElMessage.warning('请输入 http:// 或 https:// 开头的网址')
    return
  }
  uploadError.value = ''
  progress.value = null
  try {
    await runPreview(() => previewDatasetImportUrlApi(link))
  }
  catch (e) {
    uploadError.value = errorText(e, '读取网址失败')
  }
}

async function runPreview(load: () => Promise<ImportPreview>, firstName?: string) {
  uploading.value = true
  try {
    const p = await load()
    preview.value = p
    form.columns = p.columns.map(c => ({ ...c, include: c.fillRate > 0 }))
    // 合并多个文件时用第一个文件名作为默认数据集名（fileName 是暂存文件名，提交时要原样带回）
    form.name = defaultName(p.sourceFiles?.length && firstName ? firstName : p.fileName)
    form.primaryKey = []
    // 列名完全对得上的已有数据集：默认追加（映射模板：同一来源的文件下次自动对齐）
    const best = p.matches.find(m => m.matched === m.total)
    form.mode = best ? 'append' : 'create'
    form.targetId = best?.datasetId ?? p.matches[0]?.datasetId ?? userDatasets.value[0]?.id ?? ''
    step.value = 'map'
  }
  finally {
    uploading.value = false
  }
}

const includedTitles = computed(() => form.columns.filter(c => c.include).map(c => c.title.trim()))
const duplicateTitle = computed(() => includedTitles.value.find((t, i) => !t || includedTitles.value.indexOf(t) !== i))
const target = computed(() => props.datasets.find(d => d.id === form.targetId))
const targetMatch = computed(() => preview.value?.matches.find(m => m.datasetId === form.targetId))

// 改名后主键里的旧名称失效
watch(includedTitles, (titles) => {
  form.primaryKey = form.primaryKey.filter(k => titles.includes(k))
})

const canCommit = computed(() => {
  if (!preview.value)
    return false
  if (form.mode === 'create')
    return !!form.name.trim() && includedTitles.value.length > 0 && !duplicateTitle.value
  return !!target.value && (targetMatch.value?.matched ?? 1) > 0
})

async function commit() {
  const p = preview.value
  if (!p)
    return
  if (form.mode === 'replace') {
    const ok = await ElMessageBox.confirm(
      `将先删除数据集「${target.value?.name}」的全部 ${target.value?.rowCount.toLocaleString()} 行，再写入本文件的数据。删除后不可恢复。`,
      '替换数据集',
      { type: 'warning', confirmButtonText: '确认替换' },
    ).then(() => true, () => false)
    if (!ok)
      return
  }
  committing.value = true
  try {
    outcome.value = await commitDatasetImportApi({
      token: p.token,
      mode: form.mode,
      fileName: p.fileName,
      ...(form.mode === 'create'
        ? {
            name: form.name.trim(),
            columns: form.columns.map(c => ({ source: c.source, title: c.title.trim(), kind: c.kind, include: c.include })),
            primaryKey: form.primaryKey,
          }
        : { datasetId: form.targetId }),
    })
    step.value = 'done'
    emit('imported', outcome.value.datasetId, false)
  }
  finally {
    committing.value = false
  }
}

function openDataset() {
  if (outcome.value)
    emit('imported', outcome.value.datasetId, true)
  emit('update:visible', false)
}

const sizeText = (n: number) => n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1e3))} KB`
</script>

<template>
  <el-dialog
    :model-value="visible"
    title="导入数据"
    width="920px"
    :close-on-click-modal="false"
    :close-on-press-escape="!committing"
    :show-close="!committing"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <!-- 选择文件 -->
    <div
      v-if="step === 'pick'"
      class="drop"
      :class="{ dragging }"
      @dragover.prevent="dragging = true"
      @dragleave.self="dragging = false"
      @drop.prevent="onDrop"
    >
      <!-- 上传中：进度、取消；传完后服务器识别表头与列类型 -->
      <div v-if="uploading" class="w-full max-w-[520px] flex flex-col gap-3 py-6" role="status">
        <template v-if="progress && progress.total && progress.loaded < progress.total">
          <div class="text-[15px] text-gray-800 font-medium">
            正在上传 {{ sizeText(progress.loaded) }} / {{ sizeText(progress.total) }}
          </div>
          <el-progress :percentage="Math.min(99, Math.floor((progress.loaded / progress.total) * 100))" :stroke-width="8" />
        </template>
        <template v-else>
          <div class="text-[15px] text-gray-800 font-medium">
            <BusyInline :loading="uploading" text="服务器正在识别编码、表头与列类型" />
          </div>
          <el-progress :percentage="100" :indeterminate="true" :duration="2" :show-text="false" :stroke-width="8" />
        </template>
        <div>
          <el-button size="small" @click="cancelUpload">
            取消
          </el-button>
        </div>
      </div>
      <template v-else>
        <div v-if="dragging" class="text-[15px] text-[#217346] font-medium">
          松开鼠标，开始上传
        </div>
        <div v-if="uploadError" class="max-w-[560px] rounded bg-[#FEF2F2] px-3 py-2 text-left text-[13px] text-[#B91C1C] leading-5" role="alert">
          {{ uploadError }}
        </div>
        <div class="text-[15px] text-gray-800 font-medium">
          选择或拖入 CSV / XLS / XLSX / Parquet / JSON 文件
        </div>
        <div class="text-xs text-gray-500 leading-5">
          自动识别编码（UTF-8 / GBK）、表头所在行（跳过前面的标题和说明行）、列类型；百万行文件约 2 秒入库。<br>
          可一次选择多个同结构的文件（如每月一个），按列名对齐合并为一个数据集，并追加「来源文件」列。<br>
          导入后可在「数据表」里浏览筛选，并在任意单元格用 DSUMIFS("数据集名", "求和列", "条件列", 条件, …) 汇总
        </div>
        <label class="pick-btn">
          选择文件
          <input type="file" multiple accept=".csv,.txt,.tsv,.xls,.xlsx,.parquet,.json,.jsonl,.ndjson" class="hidden" @change="onFile">
        </label>
        <div class="mt-3 w-full max-w-[560px] flex items-center gap-2">
          <span class="shrink-0 text-xs text-gray-500">或从网址导入</span>
          <el-input v-model="urlInput" size="small" clearable placeholder="https://… 文件下载地址（CSV / XLSX / Parquet / JSON）" @keyup.enter="onUrl" />
          <el-button size="small" :disabled="!urlInput.trim()" @click="onUrl">
            读取
          </el-button>
        </div>
      </template>
    </div>

    <!-- 映射 -->
    <div v-else-if="step === 'map' && preview" class="flex flex-col gap-3 text-[13px]">
      <div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-gray-600">
        <span class="text-gray-900 font-medium">{{ preview.fileName }}</span>
        <span>{{ sizeText(preview.fileSize) }}</span>
        <span>约 {{ preview.estimatedRows.toLocaleString() }} 行</span>
        <span v-if="!preview.sourceFiles?.length">表头在第 {{ preview.headerRow }} 行</span>
        <span>{{ preview.columns.length }} 列</span>
      </div>
      <div v-if="preview.sourceFiles?.length" class="rounded bg-[#F7F8FA] px-3 py-2 text-xs text-gray-600 leading-6">
        <div class="text-gray-800">
          合并 {{ preview.sourceFiles.length }} 个文件，共 {{ preview.sourceFiles.reduce((a, f) => a + f.rows, 0).toLocaleString() }} 行（按列名对齐，已追加「来源文件」列）
        </div>
        <div v-for="f in preview.sourceFiles" :key="f.name" class="flex gap-3">
          <span class="min-w-0 flex-1 truncate">{{ f.name }}</span>
          <span class="tabular-nums">{{ f.rows.toLocaleString() }} 行</span>
          <span v-if="f.missing.length" class="text-[#92400E]">缺少：{{ f.missing.join('、') }}（为空）</span>
        </div>
      </div>

      <el-radio-group v-model="form.mode">
        <el-radio-button value="create">
          新建数据集
        </el-radio-button>
        <el-radio-button value="append" :disabled="!userDatasets.length">
          追加到已有
        </el-radio-button>
        <el-radio-button value="replace" :disabled="!userDatasets.length">
          替换已有
        </el-radio-button>
      </el-radio-group>

      <template v-if="form.mode === 'create'">
        <div class="flex items-center gap-3">
          <span class="w-20 text-gray-500">数据集名称</span>
          <el-input v-model="form.name" class="!w-64" placeholder="公式里用这个名称引用" />
          <span class="w-14 text-gray-500">主键</span>
          <el-select v-model="form.primaryKey" multiple collapse-tags clearable placeholder="可选：用于去重（如订单号）" class="!w-72">
            <el-option v-for="t in includedTitles" :key="t" :label="t" :value="t" />
          </el-select>
        </div>
        <div v-if="duplicateTitle !== undefined" class="text-xs text-[#B91C1C]">
          列名不能为空或重复：「{{ duplicateTitle }}」
        </div>
        <el-table :data="form.columns" size="small" max-height="340">
          <el-table-column width="48" align="center">
            <template #default="{ row }">
              <el-checkbox v-model="row.include" />
            </template>
          </el-table-column>
          <el-table-column label="文件中的列" prop="source" min-width="130" show-overflow-tooltip />
          <el-table-column label="列名" min-width="150">
            <template #default="{ row }">
              <el-input v-model="row.title" size="small" :disabled="!row.include" />
            </template>
          </el-table-column>
          <el-table-column label="类型" width="130">
            <template #default="{ row }">
              <el-select v-model="row.kind" size="small" :disabled="!row.include">
                <el-option v-for="k in KIND_OPTIONS" :key="k.value" :label="k.label" :value="k.value">
                  <span>{{ k.label }}</span>
                  <span class="ml-2 text-xs text-gray-400">{{ k.hint }}</span>
                </el-option>
              </el-select>
            </template>
          </el-table-column>
          <el-table-column label="填充率" width="76" align="right">
            <template #default="{ row }">
              <span :class="{ 'text-gray-400': row.fillRate < 0.5 }">{{ Math.round(row.fillRate * 100) }}%</span>
            </template>
          </el-table-column>
          <el-table-column label="样例" min-width="220" show-overflow-tooltip>
            <template #default="{ row }">
              <span class="text-gray-500">{{ row.samples.join(' · ') }}</span>
            </template>
          </el-table-column>
        </el-table>
      </template>

      <template v-else>
        <div class="flex items-center gap-3">
          <span class="w-20 text-gray-500">目标数据集</span>
          <el-select v-model="form.targetId" class="!w-80">
            <el-option v-for="d in userDatasets" :key="d.id" :label="d.name" :value="d.id">
              <span>{{ d.name }}</span>
              <span class="ml-2 text-xs text-gray-400">{{ d.rowCount.toLocaleString() }} 行</span>
              <span v-if="preview.matches.find(m => m.datasetId === d.id)" class="ml-2 text-xs text-[#0B5A2E]">
                列匹配 {{ preview.matches.find(m => m.datasetId === d.id)!.matched }}/{{ preview.matches.find(m => m.datasetId === d.id)!.total }}
              </span>
            </el-option>
          </el-select>
        </div>
        <div v-if="target" class="rounded bg-[#F7F8FA] px-3 py-2 text-xs text-gray-600 leading-5">
          <div>按「文件中的列名」与数据集对齐（首次导入时记录，相同来源的文件自动套用）。</div>
          <div v-if="targetMatch">
            对上 {{ targetMatch.matched }}/{{ targetMatch.total }} 列
            <span v-if="targetMatch.missing.length" class="text-[#92400E]">；文件缺少：{{ targetMatch.missing.join('、') }}（写入空值）</span>
          </div>
          <div v-else class="text-[#B91C1C]">
            文件的列名与该数据集对不上，可能选错了数据集
          </div>
          <div v-if="target.primaryKey.length">
            按主键「{{ target.primaryKey.join(' + ') }}」去重：已存在的记录会跳过
          </div>
          <div v-if="form.mode === 'replace'" class="text-[#B91C1C]">
            替换会先删除该数据集现有的全部 {{ target.rowCount.toLocaleString() }} 行
          </div>
        </div>
      </template>

      <details class="text-xs">
        <summary class="cursor-pointer text-gray-500">
          数据预览（前 {{ preview.sampleRows.length }} 行）
        </summary>
        <el-table :data="preview.sampleRows" size="small" max-height="220" class="mt-2">
          <el-table-column v-for="(c, i) in preview.columns" :key="i" :label="c.title" min-width="110" show-overflow-tooltip>
            <template #default="{ row }">
              {{ row[i] }}
            </template>
          </el-table-column>
        </el-table>
      </details>
    </div>

    <!-- 结果 -->
    <div v-else-if="step === 'done' && outcome" class="flex flex-col gap-2 text-[13px] leading-6">
      <div class="text-[15px] text-[#0B5A2E] font-medium">
        已导入「{{ outcome.name }}」
      </div>
      <div>
        读取 {{ outcome.rowsRead.toLocaleString() }} 行，新增 {{ outcome.inserted.toLocaleString() }} 行
        <span v-if="outcome.skipped">，主键重复跳过 {{ outcome.skipped.toLocaleString() }} 行</span>
        <span v-if="outcome.replaced">，替换删除旧数据 {{ outcome.replaced.toLocaleString() }} 行</span>
        <span class="text-gray-400">（{{ (outcome.elapsedMs / 1000).toFixed(1) }} 秒）</span>
      </div>
      <div v-for="x in outcome.invalid" :key="x.column" class="text-[#92400E]">
        「{{ x.column }}」有 {{ x.count.toLocaleString() }} 个值无法识别为数字，已按空值入库
      </div>
      <div class="mt-1 rounded bg-[#F7F8FA] px-3 py-2 text-xs text-gray-600">
        在任意单元格使用：<code>=DSUMIFS("{{ outcome.name }}", "求和列", "条件列", 条件)</code>
      </div>
    </div>

    <template #footer>
      <template v-if="step === 'map'">
        <div v-if="committing" class="mr-auto text-xs">
          <BusyInline :loading="committing" text="正在写入数据库" />
          <div class="text-gray-400 leading-5">
            大文件需要几分钟，请保持此窗口打开
          </div>
        </div>
        <el-button :disabled="committing" @click="step = 'pick'">
          重新选择
        </el-button>
        <el-button type="primary" :disabled="!canCommit" :loading="committing" @click="commit">
          {{ form.mode === 'create' ? '创建并导入' : form.mode === 'append' ? '追加导入' : '替换导入' }}
        </el-button>
      </template>
      <template v-else-if="step === 'done'">
        <el-button @click="step = 'pick'">
          继续导入
        </el-button>
        <el-button type="primary" @click="openDataset">
          查看数据
        </el-button>
      </template>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.drop {
  @apply flex flex-col items-center gap-3 rounded-md border border-dashed border-[#c8c8c8] px-6 py-10 text-center transition-colors;

  &.dragging {
    @apply border-[#217346] bg-[#F0F9F4];
  }
}

.pick-btn {
  @apply mt-2 cursor-pointer rounded bg-[#217346] px-5 py-1.5 text-[13px] text-white hover:bg-[#1b5e3a];
}
</style>
