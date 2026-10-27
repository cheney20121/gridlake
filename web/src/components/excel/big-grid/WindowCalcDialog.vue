<script setup lang="ts">
import type { CellValue, DatasetInfo, WindowCalc, WindowFunc, WindowSpec } from '@/api/dataset'
import { computed, reactive, ref, watch } from 'vue'
import { createWindowApi, previewWindowApi } from '@/api/dataset'
import { formatBool, isNumericKind } from '@/features/excel/dataset/column-kinds'
import { t } from '@/i18n'
import { errorText } from '@/utils/busy-hint'

/**
 * 窗口计算：累计、排名、上一行、移动平均、组内合计 / 占比、每组前 N 行……
 * 服务端把设置翻译成一条只读 SQL，另存为新数据集（默认源数据集更新后自动重算）；新列和普通列一样能筛选、透视、做概况
 */
const props = defineProps<{ visible: boolean, dataset: DatasetInfo }>()
const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'created', datasetId: string): void
}>()

const FUNCS: { value: WindowFunc, label: string, needsValue: boolean, needsOrder: boolean }[] = [
  { value: 'running_sum', label: t('累计'), needsValue: true, needsOrder: true },
  { value: 'rank', label: t('排名（大的在前）'), needsValue: true, needsOrder: false },
  { value: 'rank_asc', label: t('排名（小的在前）'), needsValue: true, needsOrder: false },
  { value: 'row_number', label: t('序号'), needsValue: false, needsOrder: true },
  { value: 'prev', label: t('上一行的值'), needsValue: true, needsOrder: true },
  { value: 'diff', label: t('与上一行的差'), needsValue: true, needsOrder: true },
  { value: 'pct_change', label: t('较上一行的变化率'), needsValue: true, needsOrder: true },
  { value: 'moving_avg', label: t('移动平均'), needsValue: true, needsOrder: true },
  { value: 'group_sum', label: t('组内合计'), needsValue: true, needsOrder: false },
  { value: 'group_avg', label: t('组内平均'), needsValue: true, needsOrder: false },
  { value: 'share', label: t('占组内合计的比例'), needsValue: true, needsOrder: false },
  { value: 'group_count', label: t('组内行数'), needsValue: false, needsOrder: false },
]
const funcOf = (f: WindowFunc) => FUNCS.find(x => x.value === f)!

interface Row extends WindowCalc {
  /** 名称是自动起的（改了值列 / 计算会跟着变）；用户改过就不再自动改 */
  autoName: boolean
}

const form = reactive({
  calcs: [] as Row[],
  partitionBy: [] as string[],
  order: [{ column: '', desc: false }] as { column: string, desc: boolean }[],
  topOn: false,
  topN: 10,
  name: '',
  autoRefresh: true,
})

const preview = ref<{ sql: string, result: { columns: { name: string, kind: string }[], rows: CellValue[][], elapsedMs: number } } | null>(null)
const previewing = ref(false)
const creating = ref(false)
const error = ref('')

const columns = computed(() => props.dataset.columns)
const numericColumns = computed(() => columns.value.filter(c => isNumericKind(c.kind)))
const dateColumn = computed(() => columns.value.find(c => c.kind === 'datetime'))

function autoName(r: Row): string {
  const f = funcOf(r.func)
  const base = f.needsValue ? `${r.column ?? ''}${f.label.replace(/（.*）/, '')}` : f.label
  let name = base
  for (let i = 2; columns.value.some(c => c.title === name) || form.calcs.some(x => x !== r && x.name === name); i++)
    name = `${base}${i}`
  return name
}

function addCalc(func: WindowFunc = 'running_sum') {
  const r: Row = reactive({ func, column: numericColumns.value[0]?.title, name: '', autoName: true, rows: 7 })
  form.calcs.push(r)
  r.name = autoName(r)
}

function onRowChange(r: Row) {
  if (!funcOf(r.func).needsValue)
    r.column = undefined
  else if (!r.column)
    r.column = (r.func === 'prev' ? columns.value[0] : numericColumns.value[0])?.title
  if (r.autoName)
    r.name = autoName(r)
}

watch(() => props.visible, (v) => {
  if (!v)
    return
  form.calcs = []
  form.partitionBy = []
  form.order = [{ column: dateColumn.value?.title ?? '', desc: false }]
  form.topOn = false
  form.topN = 10
  form.name = t('{0}（窗口计算）', [props.dataset.name])
  form.autoRefresh = true
  preview.value = null
  error.value = ''
  addCalc()
})

const needsOrder = computed(() => form.calcs.some(r => funcOf(r.func).needsOrder) || form.topOn)

function spec(): WindowSpec {
  return {
    calcs: form.calcs.map(r => ({ func: r.func, column: r.column, name: r.name.trim(), rows: r.func === 'moving_avg' ? r.rows : undefined })),
    partitionBy: form.partitionBy,
    orderBy: form.order.filter(o => o.column),
    topN: form.topOn ? form.topN : null,
  }
}

async function runPreview() {
  previewing.value = true
  error.value = ''
  try {
    preview.value = await previewWindowApi(props.dataset.id, spec())
  }
  catch (e) {
    error.value = errorText(e)
    preview.value = null
  }
  finally {
    previewing.value = false
  }
}

async function create() {
  if (!form.name.trim())
    return ElMessage.warning(t('请填写新数据集的名称'))
  creating.value = true
  error.value = ''
  try {
    const q = await createWindowApi(props.dataset.id, { ...spec(), name: form.name.trim(), autoRefresh: form.autoRefresh })
    ElMessage.success(t('已生成「{0}」（{1} 行）', [form.name.trim(), (q.lastRows ?? 0).toLocaleString()]))
    emit('update:visible', false)
    if (q.datasetId)
      emit('created', q.datasetId)
  }
  catch (e) {
    error.value = errorText(e)
  }
  finally {
    creating.value = false
  }
}

function cell(v: CellValue): string {
  if (v == null)
    return ''
  if (typeof v === 'boolean')
    return formatBool(v)
  if (typeof v === 'number' && !Number.isInteger(v))
    return v.toLocaleString(undefined, { maximumFractionDigits: 4 })
  return String(v)
}
</script>

<template>
  <el-dialog :model-value="visible" :title="$t('窗口计算 · {0}', [dataset.name])" width="920px" append-to-body :close-on-click-modal="false" @update:model-value="emit('update:visible', $event)">
    <div class="flex flex-col gap-3 text-[13px]" data-testid="window-dialog">
      <div class="rounded bg-[#F7F8FA] px-3 py-2 text-xs text-gray-600 leading-5">
        {{ $t('按分组、按顺序算累计、排名、和上一行比、移动平均、组内占比……在服务端整表计算（亿级也可以），结果另存为一个新数据集：新列和普通列一样能筛选、透视、写公式。源数据集更新后可以自动重算。') }}
      </div>

      <div class="font-medium text-gray-800">
        {{ $t('计算') }}
      </div>
      <div v-for="(r, i) in form.calcs" :key="i" class="flex items-center gap-2" :data-testid="`window-calc-${i}`">
        <el-select v-model="r.func" size="small" class="!w-44" @change="onRowChange(r)">
          <el-option v-for="f in FUNCS" :key="f.value" :label="f.label" :value="f.value" />
        </el-select>
        <el-select v-if="funcOf(r.func).needsValue" v-model="r.column" size="small" class="!w-40" :placeholder="$t('值列')" @change="onRowChange(r)">
          <el-option v-for="c in (r.func === 'prev' ? columns : numericColumns)" :key="c.key" :label="c.title" :value="c.title" />
        </el-select>
        <span v-else class="w-40 text-xs text-gray-400">{{ $t('（不需要值列）') }}</span>
        <template v-if="r.func === 'moving_avg'">
          <el-input-number v-model="r.rows" size="small" :min="2" :max="1000" controls-position="right" class="!w-24" />
          <span class="text-xs text-gray-500">{{ $t('行') }}</span>
        </template>
        <span class="text-gray-400">→</span>
        <el-input v-model="r.name" size="small" class="!w-48" maxlength="40" :placeholder="$t('新列名')" @input="r.autoName = false" />
        <el-button link size="small" type="danger" :disabled="form.calcs.length === 1" @click="form.calcs.splice(i, 1)">
          {{ $t('删除') }}
        </el-button>
      </div>
      <div>
        <el-button size="small" data-testid="window-add" @click="addCalc('rank')">
          {{ $t('+ 再加一个计算') }}
        </el-button>
      </div>

      <el-form label-width="96px" size="small" @submit.prevent>
        <el-form-item :label="$t('分组')">
          <el-select v-model="form.partitionBy" multiple filterable clearable class="!w-96" :placeholder="$t('不分组：整个数据集算一组')" data-testid="window-partition">
            <el-option v-for="c in columns" :key="c.key" :label="c.title" :value="c.title" />
          </el-select>
        </el-form-item>
        <el-form-item :label="$t('组内顺序')" :required="needsOrder">
          <div class="flex flex-col gap-1">
            <div v-for="(o, i) in form.order" :key="i" class="flex items-center gap-2">
              <el-select v-model="o.column" clearable filterable class="!w-48" :placeholder="$t('排序列')">
                <el-option v-for="c in columns" :key="c.key" :label="c.title" :value="c.title" />
              </el-select>
              <el-radio-group v-model="o.desc">
                <el-radio-button :value="false">
                  {{ $t('升序') }}
                </el-radio-button>
                <el-radio-button :value="true">
                  {{ $t('降序') }}
                </el-radio-button>
              </el-radio-group>
              <el-button v-if="i === form.order.length - 1 && form.order.length < 3" link size="small" @click="form.order.push({ column: '', desc: false })">
                {{ $t('+ 再按一列') }}
              </el-button>
            </div>
            <div class="text-xs text-gray-500 leading-5">
              {{ $t('累计、序号、上一行、移动平均按这个顺序算；有并列值时再加一个更细的列（如时间之后再按单号），结果才稳定') }}
            </div>
          </div>
        </el-form-item>
        <el-form-item :label="$t('只保留')">
          <el-checkbox v-model="form.topOn">
            {{ $t('每组按组内顺序的前') }}
          </el-checkbox>
          <el-input-number v-model="form.topN" :min="1" :max="100000" :disabled="!form.topOn" controls-position="right" class="mx-2 !w-28" />
          <span class="text-gray-500">{{ $t('行（如每家店铺销售额最高的 10 天）') }}</span>
        </el-form-item>
      </el-form>

      <div v-if="error" class="rounded bg-[#FEF2F2] px-3 py-2 text-[#B91C1C] leading-5" role="alert">
        {{ error }}
      </div>

      <template v-if="preview">
        <div class="flex items-center gap-2 text-gray-600">
          <span>{{ $t('预览（前 {0} 行，用时 {1} 毫秒）', [preview.result.rows.length, preview.result.elapsedMs]) }}</span>
          <el-popover placement="bottom" :width="600" trigger="click">
            <template #reference>
              <el-button link size="small">
                {{ $t('查看生成的 SQL') }}
              </el-button>
            </template>
            <pre class="m-0 whitespace-pre-wrap break-all text-xs font-mono">{{ preview.sql }}</pre>
          </el-popover>
        </div>
        <el-table :data="preview.result.rows" size="small" max-height="260" border data-testid="window-preview">
          <el-table-column v-for="(c, i) in preview.result.columns" :key="c.name" :min-width="110" show-overflow-tooltip>
            <template #header>
              <span :class="{ 'text-[#0B5A2E] font-semibold': form.calcs.some(r => r.name.trim() === c.name) }">{{ c.name }}</span>
            </template>
            <template #default="{ row }">
              {{ cell(row[i]) }}
            </template>
          </el-table-column>
        </el-table>
      </template>

      <el-form label-width="96px" size="small" @submit.prevent>
        <el-form-item :label="$t('新数据集')" required>
          <el-input v-model="form.name" maxlength="40" class="!w-72" data-testid="window-name" />
          <el-checkbox v-model="form.autoRefresh" class="ml-3">
            {{ $t('「{0}」更新后自动重算', [dataset.name]) }}
          </el-checkbox>
        </el-form-item>
      </el-form>
    </div>

    <template #footer>
      <el-button @click="emit('update:visible', false)">
        {{ $t('取消') }}
      </el-button>
      <el-button :loading="previewing" data-testid="window-preview-btn" @click="runPreview">
        {{ $t('预览') }}
      </el-button>
      <el-button type="primary" :loading="creating" data-testid="window-create" @click="create">
        {{ $t('生成数据集') }}
      </el-button>
    </template>
  </el-dialog>
</template>
