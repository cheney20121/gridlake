<script setup lang="ts">
import type { DatasetInfo } from '@/api/dataset'
import type { TemplateDataset, TemplateInfo } from '@/api/template'
import type { WorkbookMeta } from '@/api/workbook'
import { computed, reactive, ref, watch } from 'vue'
import { KIND_LABEL, PART_LABEL, useTemplateApi } from '@/api/template'
import { compatible, suggestColumns, suggestDataset } from '@/features/excel/template/match'
import { t as tr } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'

/**
 * 使用模板：为模板需要的每份数据选择自己的数据集并对应列，生成一份新工作簿。
 * 来自参数化 SQL 查询的数据可以只填参数值（服务端按这组值生成或复用实例数据集）
 */
const props = defineProps<{
  visible: boolean
  template: TemplateInfo | null
  datasets: DatasetInfo[]
  operator?: string
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'created', meta: WorkbookMeta, warnings: string[]): void
  /** 去导入数据 */
  (e: 'importData'): void
}>()

interface BindState {
  datasetId: string
  columns: Record<string, string | null>
  /** 自动推测出的列（界面上标注「自动对应」） */
  guessed: Record<string, boolean>
}

const userStore = useLedgerUserStore()
/** 按参数生成（来自参数化查询的数据）：key → 是否按参数、参数值 */
const byParams = reactive<Record<string, boolean>>({})
const paramValues = reactive<Record<string, Record<string, string>>>({})

const name = ref('')
const clearSlicers = ref(true)
const creating = ref(false)
const binds = reactive<Record<string, BindState>>({})

function datasetOf(id: string) {
  return props.datasets.find(d => d.id === id)
}

function fill(need: TemplateDataset, datasetId: string) {
  const ds = datasetOf(datasetId)
  const cols = ds ? suggestColumns(need, ds) : {}
  binds[need.key] = {
    datasetId,
    columns: Object.fromEntries(need.columns.map(c => [c.title, cols[c.title] ?? null])),
    guessed: Object.fromEntries(need.columns.map(c => [c.title, !!cols[c.title] && cols[c.title] !== c.title])),
  }
}

watch(() => props.visible, (v) => {
  const t = props.template
  if (!v || !t)
    return
  name.value = t.name
  clearSlicers.value = true
  Object.keys(binds).forEach(k => delete binds[k])
  const taken = new Set<string>()
  Object.keys(byParams).forEach(k => delete byParams[k])
  Object.keys(paramValues).forEach(k => delete paramValues[k])
  for (const need of t.datasets) {
    const src = need.paramSource
    if (src) {
      // 受限用户不能按参数生成（需要完整的数据权限），默认改为选择数据集
      byParams[need.key] = !userStore.restricted
      paramValues[need.key] = Object.fromEntries(src.params.map(p => [p.name, src.values[p.name] ?? p.default ?? '']))
    }
    const id = suggestDataset(need, props.datasets, taken) ?? ''
    if (id && !need.builtinId)
      taken.add(id)
    fill(need, id)
  }
}, { immediate: true })

function onDatasetChange(need: TemplateDataset, id: string) {
  fill(need, id)
}

function optionsFor(need: TemplateDataset, title: string) {
  const ds = datasetOf(binds[need.key]?.datasetId ?? '')
  const col = need.columns.find(c => c.title === title)!
  return (ds?.columns ?? []).filter(c => compatible(col.kind, c.kind))
}

function partsText(need: TemplateDataset) {
  return Object.entries(need.parts).map(([k, n]) => tr('{0} 个{1}', [n, PART_LABEL[k] ?? tr('部件')])).join('、')
}

/** 每份数据的问题（未选择数据集、有列未对应、数据集为空） */
const problems = computed(() => {
  const out: Record<string, string> = {}
  for (const need of props.template?.datasets ?? []) {
    const b = binds[need.key]
    if (byParams[need.key]) {
      const missing = need.paramSource!.params.filter(p => !paramValues[need.key]?.[p.name]?.trim() && !p.default?.trim())
      if (missing.length)
        out[need.key] = tr('请填写参数「{0}」', [missing.map(p => p.name).join('」「')])
      continue
    }
    const ds = datasetOf(b?.datasetId ?? '')
    if (!ds)
      out[need.key] = need.builtinId ? tr('内置数据集「{0}」不可用', [need.name]) : tr('请选择数据集')
    else if (need.columns.some(c => !b?.columns[c.title]))
      out[need.key] = tr('还有 {0} 列没有对应', [need.columns.filter(c => !b?.columns[c.title]).length])
  }
  return out
})

const ready = computed(() => !!name.value.trim() && !Object.keys(problems.value).length)
const noDatasets = computed(() => props.template?.datasets.some(d => !d.builtinId) && !props.datasets.some(d => !d.builtin))

async function create() {
  const t = props.template
  if (!t || !ready.value)
    return
  creating.value = true
  try {
    const bindings = Object.fromEntries(t.datasets.filter(need => !byParams[need.key]).map((need) => {
      const b = binds[need.key]!
      return [need.key, { datasetId: b.datasetId, columns: Object.fromEntries(Object.entries(b.columns).filter(([k, v]) => v && v !== k)) as Record<string, string> }]
    }))
    const params = Object.fromEntries(t.datasets.filter(need => byParams[need.key]).map(need => [
      need.key,
      Object.fromEntries(Object.entries(paramValues[need.key] ?? {}).map(([k, v]) => [k, v.trim()]).filter(([, v]) => v)),
    ]))
    const r = await useTemplateApi(t.id, { name: name.value.trim(), bindings, params, clearSlicers: clearSlicers.value, operator: props.operator })
    if (r.notes?.length)
      ElMessage.success({ message: r.notes.join('；'), duration: 5000 })
    emit('update:visible', false)
    emit('created', r.workbook, r.warnings)
  }
  finally {
    creating.value = false
  }
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('使用模板 · {0}', [template?.name ?? ''])"
    width="760px"
    :close-on-click-modal="false"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <template v-if="template">
      <div class="mb-3 flex items-center gap-3 text-[13px]">
        <span class="shrink-0 text-gray-600">{{ $t('新工作簿名称') }}</span>
        <el-input v-model="name" maxlength="80" class="flex-1" />
      </div>

      <div v-if="!template.datasets.length" class="rounded-md bg-[#f9f9f7] p-3 text-[13px] text-gray-600">
        {{ $t('这个模板不引用数据集，直接创建即可。') }}
      </div>

      <div v-if="noDatasets" class="mb-3 rounded-md bg-[#FFF7E6] p-3 text-[13px] text-[#92400E]">
        {{ $t('还没有可用的数据集。先导入数据（CSV / Excel，或设置数据连接自动同步），再回来使用模板。') }}
        <el-button link type="primary" size="small" @click="emit('importData')">
          {{ $t('去导入数据') }}
        </el-button>
      </div>

      <div v-for="need in template.datasets" :key="need.key" class="need">
        <div class="flex items-center gap-2">
          <span class="font-medium">{{ need.name }}</span>
          <span class="text-xs text-gray-400">{{ $t('用于 {0}', [partsText(need)]) }}</span>
          <span v-if="problems[need.key]" class="ml-auto text-xs text-[#B91C1C]">{{ problems[need.key] }}</span>
          <span v-else class="ml-auto text-xs text-[#0B5A2E]">{{ $t('✓ 已对应') }}</span>
        </div>
        <div v-if="need.description" class="mt-0.5 text-xs text-gray-500">
          {{ need.description }}
        </div>

        <div v-if="need.paramSource" class="mt-2 flex items-center gap-2 text-[13px]" data-testid="param-mode">
          <span class="w-20 shrink-0 text-gray-600">{{ $t('数据来源') }}</span>
          <el-radio-group v-model="byParams[need.key]" size="small">
            <el-radio-button :value="true" :disabled="userStore.restricted">
              {{ $t('按参数生成') }}
            </el-radio-button>
            <el-radio-button :value="false">
              {{ $t('选择数据集') }}
            </el-radio-button>
          </el-radio-group>
          <span class="text-xs text-gray-400">{{ userStore.restricted ? $t('按参数生成需要完整的数据权限') : $t('来自参数化查询「{0}」', [need.paramSource.queryName]) }}</span>
        </div>
        <div v-if="need.paramSource && byParams[need.key]" class="params" data-testid="param-values">
          <div v-for="p in need.paramSource.params" :key="p.name" class="flex items-center gap-2">
            <span class="w-20 shrink-0 truncate text-gray-600" :title="p.name">{{ p.name }}</span>
            <el-select v-if="p.choices?.length" v-model="paramValues[need.key]![p.name]" size="small" class="flex-1" :placeholder="p.default ? $t('默认 {0}', [p.default]) : $t('请选择')">
              <el-option v-for="c in p.choices" :key="c" :label="c" :value="c" />
            </el-select>
            <el-date-picker v-else-if="p.kind === 'date'" v-model="paramValues[need.key]![p.name]" type="date" value-format="YYYY-MM-DD" size="small" class="!flex-1" :placeholder="p.default ? $t('默认 {0}', [p.default]) : $t('选择日期')" />
            <el-input v-else v-model="paramValues[need.key]![p.name]" size="small" class="flex-1" :inputmode="p.kind === 'number' ? 'decimal' : undefined" :placeholder="p.default ? $t('默认 {0}', [p.default]) : (p.kind === 'number' ? $t('数字') : $t('文本'))" />
          </div>
          <div class="text-xs text-gray-400 leading-5">
            {{ $t('按这组参数生成一份数据集（已有同样参数的就直接用，并按最新数据重算），模板里的部件都改用它，列自动对应。') }}
          </div>
        </div>

        <div v-if="!byParams[need.key]" class="mt-2 flex items-center gap-2 text-[13px]">
          <span class="w-20 shrink-0 text-gray-600">{{ $t('使用数据集') }}</span>
          <template v-if="need.builtinId">
            <span>{{ $t('内置「{0}」', [datasetOf(need.builtinId)?.name ?? need.name]) }}</span>
            <span class="text-xs text-gray-400">{{ $t('{0} 行', [(datasetOf(need.builtinId)?.rowCount ?? 0).toLocaleString()]) }}</span>
            <span v-if="datasetOf(need.builtinId)?.rowCount === 0" class="text-xs text-[#92400E]">{{ $t('暂无数据：导入店铺资金明细后自动有数') }}</span>
          </template>
          <el-select
            v-else
            :model-value="binds[need.key]?.datasetId"
            class="flex-1"
            filterable
            :placeholder="$t('选择数据集')"
            @update:model-value="(id: string) => onDatasetChange(need, id)"
          >
            <el-option v-for="d in datasets" :key="d.id" :label="d.name" :value="d.id">
              {{ d.name }} <span class="text-xs text-gray-400">{{ $t('{0} 行', [d.rowCount.toLocaleString()]) }}</span>
            </el-option>
          </el-select>
        </div>

        <table v-if="!need.builtinId && !byParams[need.key] && binds[need.key]?.datasetId" class="map">
          <thead>
            <tr>
              <th class="w-[40%]">
                {{ $t('模板需要的列') }}
              </th>
              <th>{{ $t('对应到') }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="col in need.columns" :key="col.title">
              <td>
                {{ col.title }}
                <span class="kind">{{ KIND_LABEL[col.kind] }}</span>
                <div v-if="col.description" class="text-xs text-gray-400">
                  {{ col.description }}
                </div>
              </td>
              <td>
                <div class="flex items-center gap-2">
                  <el-select
                    v-model="binds[need.key]!.columns[col.title]"
                    size="small"
                    class="flex-1"
                    :placeholder="$t('选择列')"
                    :class="{ missing: !binds[need.key]!.columns[col.title] }"
                    @change="binds[need.key]!.guessed[col.title] = false"
                  >
                    <el-option v-for="c in optionsFor(need, col.title)" :key="c.title" :label="c.title" :value="c.title">
                      {{ c.title }} <span class="text-xs text-gray-400">{{ KIND_LABEL[c.kind as keyof typeof KIND_LABEL] ?? c.kind }}</span>
                    </el-option>
                  </el-select>
                  <span v-if="binds[need.key]!.guessed[col.title]" class="shrink-0 text-xs text-[#92400E]" :title="$t('列名不同，按常见同义词自动对应，请确认')">{{ $t('自动对应') }}</span>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <div class="mt-3 flex items-center gap-4 text-[13px]">
        <el-checkbox v-model="clearSlicers">
          {{ $t('清空切片器中模板预选的值') }}
        </el-checkbox>
        <span class="text-xs text-gray-400">{{ $t('日期切片器的「今年」「本月」等保留') }}</span>
      </div>
      <div v-if="template.warnings.length" class="mt-2 text-xs text-[#92400E] leading-5">
        <div v-for="w in template.warnings" :key="w">
          · {{ w }}
        </div>
      </div>
    </template>

    <template #footer>
      <el-button @click="emit('update:visible', false)">
        {{ $t('取消') }}
      </el-button>
      <el-button type="primary" :disabled="!ready" :loading="creating" @click="create">
        {{ $t('创建工作簿') }}
      </el-button>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.need {
  @apply mb-3 rounded-md border border-[#e5e7eb] border-solid px-3 py-2.5 text-[13px];
}

.params {
  @apply mt-2 flex flex-col gap-1.5 rounded-md bg-[#f9f9f7] px-2.5 py-2 text-[13px];
}

.map {
  @apply mt-2 w-full border-collapse text-[13px];

  th {
    @apply bg-[#f9f9f7] px-2 py-1.5 text-left text-xs text-gray-500 font-normal;
  }

  td {
    @apply border-b border-b-[#f0efec] border-b-solid px-2 py-1.5 align-top;
  }
}

.kind {
  @apply ml-1 rounded bg-[#f4f4f1] px-1 text-xs text-gray-500;
}

.missing :deep(.#{$namespace}-select__wrapper) {
  box-shadow: 0 0 0 1px #dc2626 inset;
}
</style>
