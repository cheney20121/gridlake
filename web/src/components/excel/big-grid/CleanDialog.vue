<script setup lang="ts">
import type { CellValue, CleanOp, CleanPreview, DatasetInfo, DatasetQuery, LastClean } from '@/api/dataset'
import { useDebounceFn } from '@vueuse/core'
import { isCancel } from 'axios'
import { computed, reactive, ref, watch } from 'vue'
import { applyCleanApi, lastCleanApi, previewCleanApi, undoCleanApi } from '@/api/dataset'
import { formatBool, isNumericKind } from '@/features/excel/dataset/column-kinds'
import { t } from '@/i18n'
import { errorText } from '@/utils/busy-hint'

/**
 * 数据清洗：删除重复行、去首尾空白、查找替换、填充空值、删除筛选出的行
 * 改设置就自动预览（受影响多少行、改前 / 改后的样例），确认后在服务端整表执行；可以撤销最近一次
 */
const props = defineProps<{
  visible: boolean
  dataset: DatasetInfo
  /** 数据表当前的筛选（「删除筛选出的行」用） */
  query: DatasetQuery
  /** 当前筛选的文字说明 */
  filterText: string
}>()
const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'changed'): void
}>()

type Kind = CleanOp['op']
const OPS: { value: Kind, label: string }[] = [
  { value: 'dedupe', label: t('删除重复行') },
  { value: 'trim', label: t('去首尾空白') },
  { value: 'replace', label: t('查找替换') },
  { value: 'fill', label: t('填充空值') },
  { value: 'delete_rows', label: t('删除筛选出的行') },
]

const form = reactive({
  op: 'dedupe' as Kind,
  dedupeColumns: [] as string[],
  trimColumns: [] as string[],
  column: '',
  find: '',
  replace: '',
  whole: true,
  fillMode: 'value' as 'value' | 'down',
  fillValue: '',
})

/** 能清洗的列：数据表里实际存的列（计算列不行） */
const columns = computed(() => props.dataset.columns.filter(c => !c.computed))
const textColumns = computed(() => columns.value.filter(c => c.kind === 'text' || c.kind === 'id'))
const hasFilter = computed(() => !!props.query.filters?.length || !!props.query.keyword?.trim() || !!props.query.violation)

const preview = ref<CleanPreview | null>(null)
const previewError = ref('')
const loading = ref(false)
const applying = ref(false)
const last = ref<LastClean | null>(null)
let controller: AbortController | null = null
/** 上一次预览的设置：没变就不重复预览（打开时表单重置也会触发一次） */
let lastKey = ''

function op(): CleanOp | null {
  switch (form.op) {
    case 'dedupe': return { op: 'dedupe', columns: form.dedupeColumns }
    case 'trim': return { op: 'trim', columns: form.trimColumns }
    case 'replace': return form.column && form.find ? { op: 'replace', column: form.column, find: form.find, replace: form.replace, whole: form.whole } : null
    case 'fill': return form.column && (form.fillMode === 'down' || form.fillValue.trim()) ? { op: 'fill', column: form.column, value: form.fillValue, down: form.fillMode === 'down' } : null
    default: return hasFilter.value ? { op: 'delete_rows', spec: { ...props.query, sort: [] } } : null
  }
}

async function runPreview(force = false) {
  const o = op()
  const key = JSON.stringify(o)
  if (!force && key === lastKey && (preview.value || loading.value))
    return
  lastKey = key
  controller?.abort()
  previewError.value = ''
  if (!o) {
    preview.value = null
    return
  }
  // 重新预览时先留着上一次的结果（不闪），执行按钮在加载期间不可点
  controller = new AbortController()
  loading.value = true
  try {
    preview.value = await previewCleanApi(props.dataset.id, o, controller.signal)
  }
  catch (e) {
    if (!isCancel(e)) {
      previewError.value = errorText(e)
      preview.value = null
    }
  }
  finally {
    loading.value = false
  }
}
const schedulePreview = useDebounceFn(() => runPreview(), 400)

async function loadLast() {
  last.value = await lastCleanApi(props.dataset.id).catch(() => null)
}

watch(() => props.visible, (v) => {
  if (!v)
    return
  Object.assign(form, { op: 'dedupe', dedupeColumns: [], trimColumns: [], column: textColumns.value[0]?.title ?? columns.value[0]?.title ?? '', find: '', replace: '', whole: true, fillMode: 'value', fillValue: '' })
  loadLast()
  runPreview(true)
})

watch(() => [form.op, form.dedupeColumns, form.trimColumns, form.column, form.find, form.replace, form.whole, form.fillMode, form.fillValue], () => {
  if (props.visible)
    schedulePreview()
}, { deep: true })

// 换操作时把列选到合适的类型上（查找替换只能文本列）
watch(() => form.op, (o) => {
  if (o === 'replace' && !textColumns.value.some(c => c.title === form.column))
    form.column = textColumns.value[0]?.title ?? ''
  if (o === 'fill' && !form.column)
    form.column = columns.value[0]?.title ?? ''
})

async function apply() {
  const o = op()
  if (!o || !preview.value?.affected)
    return
  if (preview.value.deletes) {
    const ok = await ElMessageBox.confirm(t('将删除 {0} 行（共 {1} 行）。可以撤销最近一次清洗，但之后数据集再有改动就不能撤销了。', [preview.value.affected.toLocaleString(), preview.value.rows.toLocaleString()]), t('删除行'), { type: 'warning', confirmButtonText: t('删除') }).then(() => true, () => false)
    if (!ok)
      return
  }
  applying.value = true
  try {
    const r = await applyCleanApi(props.dataset.id, o)
    ElMessage.success(r.deletes ? t('已删除 {0} 行', [r.affected.toLocaleString()]) : t('已修改 {0} 行', [r.affected.toLocaleString()]))
    emit('changed')
    await loadLast()
    runPreview(true)
  }
  catch (e) {
    ElMessage.error(errorText(e))
  }
  finally {
    applying.value = false
  }
}

async function undo() {
  applying.value = true
  try {
    await undoCleanApi(props.dataset.id)
    ElMessage.success(t('已撤销'))
    emit('changed')
    await loadLast()
    runPreview(true)
  }
  catch (e) {
    ElMessage.error(errorText(e))
  }
  finally {
    applying.value = false
  }
}

function cell(v: CellValue | undefined): string {
  if (v == null)
    return t('（空）')
  if (typeof v === 'boolean')
    return formatBool(v)
  if (typeof v === 'string' && v !== v.trim())
    return `「${v}」`
  return String(v)
}
</script>

<template>
  <el-dialog :model-value="visible" :title="$t('数据清洗 · {0}', [dataset.name])" width="820px" append-to-body :close-on-click-modal="!applying" @update:model-value="emit('update:visible', $event)">
    <div class="flex flex-col gap-3 text-[13px]" data-testid="clean-dialog">
      <el-radio-group v-model="form.op" size="small" data-testid="clean-op">
        <el-radio-button v-for="o in OPS" :key="o.value" :value="o.value">
          {{ o.label }}
        </el-radio-button>
      </el-radio-group>

      <el-form label-width="96px" size="small" @submit.prevent>
        <el-form-item v-if="form.op === 'dedupe'" :label="$t('按哪些列')">
          <el-select v-model="form.dedupeColumns" multiple filterable clearable class="!w-full" :placeholder="$t('不选：所有列都相同才算重复')">
            <el-option v-for="c in columns" :key="c.key" :label="c.title" :value="c.title" />
          </el-select>
          <div class="mt-1 text-xs text-gray-500">
            {{ $t('这些列都相同的行只留最先导入的一行，其余删除') }}
          </div>
        </el-form-item>
        <el-form-item v-else-if="form.op === 'trim'" :label="$t('哪些列')">
          <el-select v-model="form.trimColumns" multiple filterable clearable class="!w-full" :placeholder="$t('不选：全部文本列')">
            <el-option v-for="c in textColumns" :key="c.key" :label="c.title" :value="c.title" />
          </el-select>
          <div class="mt-1 text-xs text-gray-500">
            {{ $t('去掉开头和结尾的空格、制表、换行、全角空格；只有空白的格子变成空值') }}
          </div>
        </el-form-item>
        <template v-else-if="form.op === 'replace'">
          <el-form-item :label="$t('列')">
            <el-select v-model="form.column" filterable class="!w-56" data-testid="clean-column">
              <el-option v-for="c in textColumns" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
            <el-radio-group v-model="form.whole" class="ml-3">
              <el-radio :value="true">
                {{ $t('整格等于') }}
              </el-radio>
              <el-radio :value="false">
                {{ $t('包含（替换每一处）') }}
              </el-radio>
            </el-radio-group>
          </el-form-item>
          <el-form-item :label="$t('查找')">
            <el-input v-model="form.find" class="!w-56" data-testid="clean-find" />
            <span class="mx-2 text-gray-400">→</span>
            <el-input v-model="form.replace" class="!w-56" :placeholder="$t('替换为（留空即变成空值）')" data-testid="clean-replace" />
          </el-form-item>
        </template>
        <template v-else-if="form.op === 'fill'">
          <el-form-item :label="$t('列')">
            <el-select v-model="form.column" filterable class="!w-56">
              <el-option v-for="c in columns" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
          </el-form-item>
          <el-form-item :label="$t('空值')">
            <el-radio-group v-model="form.fillMode">
              <el-radio value="value">
                {{ $t('填为') }}
              </el-radio>
              <el-radio value="down">
                {{ $t('用上一行的值（向下填充）') }}
              </el-radio>
            </el-radio-group>
            <el-input v-if="form.fillMode === 'value'" v-model="form.fillValue" class="ml-2 !w-40" :placeholder="isNumericKind(dataset.columns.find(c => c.title === form.column)?.kind) ? '0' : ''" data-testid="clean-fill-value" />
          </el-form-item>
          <div class="pl-24 text-xs text-gray-500">
            {{ $t('文本列里只有空白的也算空值；向下填充按导入的先后顺序') }}
          </div>
        </template>
        <el-form-item v-else :label="$t('要删除的行')">
          <span v-if="hasFilter" class="text-gray-700">{{ $t('数据表当前筛选出的行：{0}', [filterText]) }}</span>
          <span v-else class="text-[#B45309]">{{ $t('请先在数据表上筛选出要删除的行（没有筛选条件时不能整表删除）') }}</span>
        </el-form-item>
      </el-form>

      <div v-if="previewError" class="rounded bg-[#FEF2F2] px-3 py-2 text-[#B91C1C] leading-5" role="alert">
        {{ previewError }}
      </div>
      <div v-loading="loading" class="min-h-24 rounded border border-[#e5e7eb] border-solid px-3 py-2" data-testid="clean-preview">
        <template v-if="preview">
          <div class="mb-2" :class="preview.affected ? (preview.deletes ? 'text-[#B91C1C]' : 'text-[#0B5A2E]') : 'text-gray-500'">
            <template v-if="!preview.affected">
              {{ $t('{0}：没有需要处理的行', [preview.summary]) }}
            </template>
            <template v-else-if="preview.deletes">
              {{ $t('{0}：将删除 {1} 行（共 {2} 行）', [preview.summary, preview.affected.toLocaleString(), preview.rows.toLocaleString()]) }}
            </template>
            <template v-else>
              {{ $t('{0}：将修改 {1} 行（共 {2} 行）', [preview.summary, preview.affected.toLocaleString(), preview.rows.toLocaleString()]) }}
            </template>
          </div>
          <div v-if="preview.samples.length" class="max-h-56 overflow-auto">
            <table class="w-full text-xs">
              <thead>
                <tr class="text-left text-gray-500">
                  <th v-for="c in preview.columns" :key="c" class="pb-1 pr-3 font-normal">
                    {{ c }}
                  </th>
                  <template v-if="!preview.deletes">
                    <th v-for="c in preview.columns" :key="`a-${c}`" class="pb-1 pr-3 text-[#0B5A2E] font-normal">
                      {{ $t('{0}（改后）', [c]) }}
                    </th>
                  </template>
                </tr>
              </thead>
              <tbody>
                <tr v-for="(s, i) in preview.samples" :key="i" :class="{ 'line-through text-gray-400': preview.deletes }">
                  <td v-for="(v, j) in s.before" :key="j" class="max-w-40 truncate py-0.5 pr-3">
                    {{ cell(v) }}
                  </td>
                  <template v-if="s.after">
                    <td v-for="(v, j) in s.after" :key="`a${j}`" class="max-w-40 truncate py-0.5 pr-3 text-[#0B5A2E]">
                      {{ cell(v) }}
                    </td>
                  </template>
                </tr>
              </tbody>
            </table>
            <div v-if="preview.affected > preview.samples.length" class="mt-1 text-xs text-gray-400">
              {{ $t('（只列出前 {0} 行）', [preview.samples.length]) }}
            </div>
          </div>
        </template>
        <div v-else-if="!loading && !previewError" class="text-gray-400">
          {{ $t('填好设置后自动预览') }}
        </div>
      </div>

      <div v-if="last" class="flex items-center gap-2 text-xs text-gray-500" data-testid="clean-last">
        <span>{{ $t('最近一次：{0}（{1} 行，{2}{3}）', [last.summary, last.affected.toLocaleString(), last.createdAt.slice(5, 16), last.operator ? ` · ${last.operator}` : '']) }}</span>
        <el-button v-if="last.canUndo" link size="small" type="primary" :loading="applying" data-testid="clean-undo" @click="undo">
          {{ $t('撤销') }}
        </el-button>
        <span v-else class="text-gray-400">{{ $t('不能撤销：{0}', [last.reason ?? '']) }}</span>
      </div>
    </div>

    <template #footer>
      <el-button @click="emit('update:visible', false)">
        {{ $t('关闭') }}
      </el-button>
      <el-button :type="preview?.deletes ? 'danger' : 'primary'" :loading="applying" :disabled="!preview?.affected || loading" data-testid="clean-apply" @click="apply">
        {{ preview?.deletes ? $t('删除 {0} 行', [(preview?.affected ?? 0).toLocaleString()]) : $t('修改 {0} 行', [(preview?.affected ?? 0).toLocaleString()]) }}
      </el-button>
    </template>
  </el-dialog>
</template>
