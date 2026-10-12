<script setup lang="ts">
import type { CalcPreview, DatasetColumn, DatasetColumnKind, DatasetInfo } from '@/api/dataset'
import { useDebounceFn } from '@vueuse/core'
import { isCancel } from 'axios'
import { computed, ref, watch } from 'vue'
import { addCalcApi, previewCalcApi, removeCalcApi, updateCalcApi } from '@/api/dataset'
import { formatMoney } from '@/components/excel/ledger-detail/columns'
import { t } from '@/i18n'

/** 计算列：用 Excel 风格公式在数据集上新增一列（千万行上按需计算，筛选 / 透视 / DSUMIFS 都能用） */
const props = defineProps<{
  visible: boolean
  dataset: DatasetInfo
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'changed'): void
}>()

const name = ref('')
const formula = ref('=')
const kind = ref<'' | DatasetColumnKind>('')
const preview = ref<CalcPreview | null>(null)
const error = ref('')
const loading = ref(false)
const saving = ref(false)
const inputRef = ref<{ textarea?: HTMLTextAreaElement } | null>(null)
const columnSearch = ref('')
/** 正在修改的计算列（null 为新增） */
const editing = ref<string | null>(null)

function reset() {
  editing.value = null
  name.value = ''
  formula.value = '='
  kind.value = ''
  preview.value = null
  error.value = ''
}

watch(() => props.visible, (v) => {
  if (v)
    reset()
})

function startEdit(c: DatasetColumn) {
  editing.value = c.title
  name.value = c.title
  formula.value = c.formula ?? '='
  // 沿用当前显示的类型（金额 / 数值），文本列由公式决定
  kind.value = c.kind === 'money' || c.kind === 'number' ? c.kind : ''
  inputRef.value?.textarea?.focus()
}

const calcColumns = computed(() => props.dataset.columns.filter(c => c.formula))
const columns = computed(() => {
  const kw = columnSearch.value.trim()
  return props.dataset.columns.filter(c => !kw || c.title.includes(kw))
})

const FUNCTIONS = [
  { name: 'IF(条件, 是, 否)', example: 'IF([动账场景]="货款结算入账",[订单实付应结]-[成本价],0)' },
  { name: 'ROUND(数值, 位数)', example: 'ROUND([毛利]/[订单实付应结],4)' },
  { name: 'IFNULL(值, 替代)', example: 'IFNULL([成本价],0)' },
  { name: 'LEFT(文本, 长度)', example: 'LEFT([下单时间],7)' },
  { name: 'ABS / MAX / MIN', example: 'ABS([佣金])' },
  { name: 'YEAR / MONTH / DAY', example: 'MONTH([下单时间])' },
]

/** 在光标处插入文本 */
function insert(text: string) {
  const el = inputRef.value?.textarea
  const v = formula.value
  if (!el) {
    formula.value = v + text
    return
  }
  const start = el.selectionStart ?? v.length
  const end = el.selectionEnd ?? v.length
  formula.value = v.slice(0, start) + text + v.slice(end)
  requestAnimationFrame(() => {
    el.focus()
    el.selectionStart = el.selectionEnd = start + text.length
  })
}

let controller: AbortController | null = null
const runPreview = useDebounceFn(async () => {
  const f = formula.value.trim()
  controller?.abort()
  if (f.length < 3) {
    preview.value = null
    error.value = ''
    return
  }
  const c = new AbortController()
  controller = c
  loading.value = true
  try {
    preview.value = await previewCalcApi(props.dataset.id, f, c.signal)
    error.value = ''
  }
  catch (e) {
    if (!isCancel(e)) {
      preview.value = null
      error.value = (e as { response?: { data?: { msg?: string } } })?.response?.data?.msg || '公式有误'
    }
  }
  finally {
    if (controller === c)
      loading.value = false
  }
}, 500)
watch(formula, runPreview)

/** 样例列：引用的列在前，「结果」放最后 */
const sampleKeys = computed(() => {
  const keys = Object.keys(preview.value?.samples[0] ?? {})
  return [...keys.filter(k => k !== '结果'), ...(keys.includes('结果') ? ['结果'] : [])]
})

const resultKind = computed(() => kind.value || preview.value?.kind || '')
const KIND_LABEL: Record<string, string> = { money: '金额', number: '数值', text: t('文本') }

function display(v: unknown) {
  if (v == null || v === '')
    return '（空）'
  if (typeof v === 'number')
    return resultKind.value === 'money' ? formatMoney(v) : String(Math.round(v * 10000) / 10000)
  return String(v)
}

async function save() {
  if (!name.value.trim() || !preview.value)
    return
  saving.value = true
  try {
    if (editing.value) {
      await updateCalcApi(props.dataset.id, editing.value, formula.value.trim(), kind.value || null)
      ElMessage.success(t('已修改计算列「{0}」，引用它的列已同步更新', [editing.value]))
    }
    else {
      await addCalcApi(props.dataset.id, name.value.trim(), formula.value.trim(), kind.value || null)
      ElMessage.success(t('已添加计算列「{0}」', [name.value.trim()]))
    }
    emit('changed')
    reset()
  }
  finally {
    saving.value = false
  }
}

async function remove(col: string) {
  const ok = await ElMessageBox.confirm(t('删除计算列「{0}」？引用它的透视表 / 公式会失效。', [col]), t('删除计算列'), { type: 'warning', confirmButtonText: t('删除') }).then(() => true, () => false)
  if (!ok)
    return
  await removeCalcApi(props.dataset.id, col)
  ElMessage.success(t('已删除'))
  if (editing.value === col)
    reset()
  emit('changed')
}
</script>

<template>
  <el-dialog :model-value="visible" :title="$t('计算列 · {0}', [dataset.name])" width="820px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div class="grid grid-cols-[1fr_220px] gap-4 text-[13px]">
      <div class="flex flex-col gap-2">
        <div class="flex items-center gap-2">
          <span class="w-12 text-gray-500">{{ $t('列名') }}</span>
          <el-input v-model="name" size="small" :placeholder="$t('如：毛利')" class="!w-48" :disabled="!!editing" />
          <span class="ml-2 text-gray-500">{{ $t('类型') }}</span>
          <el-select v-model="kind" size="small" class="!w-28" :disabled="preview?.kind === 'text'">
            <el-option :label="$t('自动')" value="" />
            <el-option :label="$t('金额')" value="money" />
            <el-option :label="$t('数值')" value="number" />
          </el-select>
        </div>
        <el-input
          ref="inputRef"
          v-model="formula"
          type="textarea"
          :rows="3"
          :placeholder="$t('=[订单实付应结]-[成本价]')"
          class="formula"
        />
        <div v-if="error" class="text-xs text-[#B91C1C]">
          {{ error }}
        </div>
        <div v-else-if="preview" class="text-xs leading-5 text-gray-600">
          {{ $t('结果类型：') }}<b>{{ KIND_LABEL[resultKind] ?? resultKind }}</b>
          <span v-if="preview.nullRows" class="ml-2 text-[#92400E]">
            {{ $t('{0} / {1} 行结果为空（引用的列为空，如关联不到成本价）；需要按 0 计算时用 IFNULL', [preview.nullRows.toLocaleString(), preview.totalRows.toLocaleString()]) }}
          </span>
        </div>
        <el-table v-if="preview?.samples.length" v-loading="loading" :data="preview.samples" size="small" max-height="220">
          <el-table-column v-for="k in sampleKeys" :key="k" :label="k" min-width="100" align="right">
            <template #default="{ row }">
              <span :class="{ 'font-semibold': k === '结果' }">{{ display(row[k]) }}</span>
            </template>
          </el-table-column>
        </el-table>

        <div v-if="calcColumns.length" class="mt-2">
          <div class="mb-1 text-xs text-gray-500">
            {{ $t('已有计算列') }}
          </div>
          <div
            v-for="c in calcColumns"
            :key="c.key"
            class="flex items-center gap-2 rounded px-1 py-0.5 text-xs"
            :class="{ 'bg-[#E7F4EC]': editing === c.title }"
          >
            <span class="font-medium">{{ c.title }}</span>
            <code class="min-w-0 flex-1 truncate text-gray-500" :title="c.formula">{{ c.formula }}</code>
            <span v-if="c.origin" class="text-gray-400" :title="$t('在「{0}」上定义，关联视图自动继承；请到「{1}」中修改', [c.origin, c.origin])">{{ $t('来自「{0}」', [c.origin]) }}</span>
            <template v-else>
              <el-button link size="small" type="primary" :disabled="editing === c.title" @click="startEdit(c)">
                {{ $t('修改') }}
              </el-button>
              <el-button link size="small" type="danger" @click="remove(c.title)">
                {{ $t('删除') }}
              </el-button>
            </template>
          </div>
        </div>
      </div>

      <div class="flex flex-col gap-2">
        <div class="text-xs text-gray-500">
          {{ $t('点击插入列') }}
        </div>
        <el-input v-model="columnSearch" size="small" clearable :placeholder="$t('搜索列')" />
        <div class="max-h-48 overflow-auto rounded border border-[#e5e7eb] border-solid p-1">
          <button v-for="c in columns" :key="c.key" class="col-btn" @click="insert(`[${c.title}]`)">
            {{ c.title }}<span v-if="c.formula" class="ml-1 text-gray-400">ƒ</span>
          </button>
        </div>
        <div class="text-xs text-gray-500">
          {{ $t('函数（点击插入示例）') }}
        </div>
        <button v-for="f in FUNCTIONS" :key="f.name" class="fn-btn" :title="f.example" @click="insert(f.example)">
          {{ f.name }}
        </button>
        <div class="text-xs text-gray-400 leading-5">
          {{ $t('运算：+ − × ÷（除数为 0 得空）、& 连接文本、比较 = <> > <；空值参与运算结果为空') }}
        </div>
      </div>
    </div>
    <template #footer>
      <el-button v-if="editing" @click="reset">
        {{ $t('取消修改') }}
      </el-button>
      <el-button @click="emit('update:visible', false)">
        {{ $t('关闭') }}
      </el-button>
      <el-button type="primary" :disabled="!name.trim() || !preview || !!error" :loading="saving" @click="save">
        {{ editing ? $t('保存修改') : $t('添加计算列') }}
      </el-button>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.formula :deep(textarea) {
  font-family: ui-monospace, Menlo, Consolas, monospace;
}

.col-btn {
  @apply block w-full truncate rounded border-none bg-transparent px-1.5 py-0.5 text-left text-xs hover:bg-[#E7F4EC];
}

.fn-btn {
  @apply rounded border border-[#e5e7eb] border-solid bg-white px-2 py-0.5 text-left text-xs hover:border-[#217346];
}
</style>
