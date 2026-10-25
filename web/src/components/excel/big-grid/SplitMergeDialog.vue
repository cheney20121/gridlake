<script setup lang="ts">
import type { CalcPreview, CellValue, DatasetInfo, SplitPreview } from '@/api/dataset'
import { useDebounceFn } from '@vueuse/core'
import { isCancel } from 'axios'
import { computed, reactive, ref, watch } from 'vue'
import { addCalcApi, previewCalcApi, previewSplitApi, removeCalcApi } from '@/api/dataset'
import { defaultNames, fitParts, mergeFormula, nameProblem, splitFormulas } from '@/features/excel/dataset/split-merge'
import { t } from '@/i18n'
import { errorText } from '@/utils/busy-hint'

/**
 * 拆分 / 合并列：拆分按分隔符把一列拆成几列（如「广东-深圳-南山」→ 省、市、区），合并把几列拼成一列。
 * 结果是计算列（SPLITPART / TEXTJOIN），不改存储的数据：之后追加、同步进来的行也自动拆好，删掉计算列就回到原样
 */
const props = defineProps<{
  visible: boolean
  dataset: DatasetInfo
}>()
const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'changed'): void
}>()

const SEPARATORS = ['-', '/', ',', '，', '、', ' ', '|', '_', ':']
const MAX_COLUMNS = 10

const form = reactive({
  mode: 'split' as 'split' | 'merge',
  column: '',
  separator: '-',
  count: 2,
  names: [] as string[],
  trim: true,
  rest: true,
  mergeColumns: [] as string[],
  mergeSeparator: '-',
  ignoreEmpty: true,
  mergeName: '',
})

const titles = computed(() => props.dataset.columns.map(c => c.title))
const loading = ref(false)
const applying = ref(false)
const error = ref('')
const split = ref<SplitPreview | null>(null)
const merged = ref<CalcPreview | null>(null)
let controller: AbortController | null = null
/** 上一次拆分预览的列和分隔符：换了才按最常见的段数重设列数 */
let lastSplitKey = ''

watch(() => props.visible, (v) => {
  if (!v)
    return
  error.value = ''
  split.value = null
  merged.value = null
  form.column ||= props.dataset.columns.find(c => c.kind === 'text')?.title ?? props.dataset.columns[0]?.title ?? ''
})

/** 列数变了：补上 / 去掉新列名（已经改过的名字保留） */
watch(() => [form.count, form.column] as const, ([n, col], before) => {
  const fresh = defaultNames(col, n, titles.value)
  if (before && before[1] !== col)
    form.names = fresh
  else
    form.names = Array.from({ length: n }, (_, i) => form.names[i] ?? fresh[i]!)
}, { immediate: true })

const splitFormulasList = computed(() => splitFormulas(form.column, form.separator, form.count, { trim: form.trim, rest: form.rest }))
const mergeFormulaText = computed(() => form.mergeColumns.length >= 2 ? mergeFormula(form.mergeColumns, form.mergeSeparator, form.ignoreEmpty) : '')

const run = useDebounceFn(async () => {
  controller?.abort()
  const c = new AbortController()
  controller = c
  error.value = ''
  loading.value = true
  try {
    if (form.mode === 'split') {
      merged.value = null
      if (!form.column || !form.separator) {
        split.value = null
        return
      }
      const p = await previewSplitApi(props.dataset.id, form.column, form.separator, c.signal)
      if (c !== controller)
        return
      const changedColumn = split.value === null || lastSplitKey !== `${form.column}|${form.separator}`
      split.value = p
      lastSplitKey = `${form.column}|${form.separator}`
      // 换了列或分隔符：列数跟着最常见的段数走
      if (changedColumn && p.commonParts >= 1)
        form.count = Math.min(MAX_COLUMNS, Math.max(2, p.commonParts))
    }
    else {
      split.value = null
      if (!mergeFormulaText.value) {
        merged.value = null
        return
      }
      const p = await previewCalcApi(props.dataset.id, mergeFormulaText.value, c.signal)
      if (c === controller)
        merged.value = p
    }
  }
  catch (e) {
    if (!isCancel(e) && c === controller)
      error.value = errorText(e)
  }
  finally {
    if (c === controller)
      loading.value = false
  }
}, 300)

watch(() => [props.visible, form.mode, form.column, form.separator, mergeFormulaText.value] as const, () => {
  if (props.visible)
    run()
})

watch(() => form.mergeColumns.join('\u0001'), () => {
  form.mergeName = form.mergeColumns.join('').slice(0, 40)
})

/** 拆分样例：原值 + 按列数整理后的各段 */
const splitRows = computed(() => (split.value?.samples ?? []).map(([v, parts]) => [v, ...fitParts(parts, form.count, form.separator, form.rest)]))

function partsText(p: SplitPreview) {
  const pieces = p.counts.map(c => t('{0} 段 {1} 行', [c.parts >= 10 ? `${c.parts}+` : c.parts, c.rows.toLocaleString()]))
  const empty = p.rows - p.nonEmpty
  if (empty)
    pieces.push(t('空 {0} 行', [empty.toLocaleString()]))
  return pieces.join('，')
}

const problem = computed(() => {
  const names = form.mode === 'split' ? form.names : [form.mergeName]
  const p = nameProblem(names, titles.value)
  if (!p)
    return form.mode === 'merge' && form.mergeColumns.length < 2 ? t('至少选两列') : ''
  const [kind, name] = [p.split(':')[0], p.slice(p.indexOf(':') + 1)]
  return {
    empty: t('新列名不能为空'),
    long: t('列名「{0}」太长（最多 40 个字）', [name]),
    char: t('列名「{0}」不能含 [ ] "', [name]),
    taken: t('已有列「{0}」，请换个名字', [name]),
    dup: t('新列名「{0}」重复了', [name]),
  }[kind!] ?? p
})

const canApply = computed(() => !problem.value && !loading.value && !error.value && (form.mode === 'split' ? !!split.value?.nonEmpty : !!merged.value))

async function apply() {
  const items = form.mode === 'split'
    ? form.names.map((n, i) => ({ name: n.trim(), formula: splitFormulasList.value[i]! }))
    : [{ name: form.mergeName.trim(), formula: mergeFormulaText.value }]
  applying.value = true
  const added: string[] = []
  try {
    for (const it of items) {
      await addCalcApi(props.dataset.id, it.name, it.formula, 'text')
      added.push(it.name)
    }
    ElMessage.success(form.mode === 'split'
      ? t('已添加计算列：{0}（之后导入的行也会自动拆好；不需要时在列头菜单删除）', [added.join('、')])
      : t('已添加计算列：{0}（之后导入的行也会自动合并；不需要时在列头菜单删除）', [added.join('、')]))
    emit('changed')
    emit('update:visible', false)
  }
  catch (e) {
    // 一部分加上了：撤回，免得只拆了一半
    for (const n of added.reverse())
      await removeCalcApi(props.dataset.id, n).catch(() => {})
    ElMessage.error(errorText(e))
  }
  finally {
    applying.value = false
  }
}

function cell(v: CellValue | undefined) {
  return v == null || v === '' ? t('（空）') : String(v)
}
</script>

<template>
  <el-dialog :model-value="visible" :title="$t('拆分 / 合并列 · {0}', [dataset.name])" width="760px" append-to-body :close-on-click-modal="!applying" @update:model-value="emit('update:visible', $event)">
    <div class="flex flex-col gap-3 text-[13px]" data-testid="split-dialog">
      <el-radio-group v-model="form.mode" size="small" data-testid="split-mode">
        <el-radio-button value="split">
          {{ $t('拆分一列') }}
        </el-radio-button>
        <el-radio-button value="merge">
          {{ $t('合并几列') }}
        </el-radio-button>
      </el-radio-group>

      <el-form label-width="96px" size="small" @submit.prevent>
        <template v-if="form.mode === 'split'">
          <el-form-item :label="$t('拆分哪一列')">
            <el-select v-model="form.column" filterable class="!w-56" data-testid="split-column">
              <el-option v-for="c in dataset.columns" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
          </el-form-item>
          <el-form-item :label="$t('分隔符')">
            <el-select v-model="form.separator" filterable allow-create default-first-option class="!w-40" data-testid="split-separator">
              <el-option v-for="s in SEPARATORS" :key="s" :label="s === ' ' ? $t('空格') : s" :value="s" />
            </el-select>
            <span class="ml-2 text-xs text-gray-500">{{ $t('可直接输入其他分隔符') }}</span>
          </el-form-item>
          <el-form-item :label="$t('拆成几列')">
            <el-input-number v-model="form.count" :min="1" :max="MAX_COLUMNS" class="!w-28" data-testid="split-count" />
            <el-checkbox v-model="form.rest" class="ml-3">
              {{ $t('最后一列保留剩下的全部') }}
            </el-checkbox>
            <el-checkbox v-model="form.trim">
              {{ $t('去掉首尾空白') }}
            </el-checkbox>
          </el-form-item>
          <el-form-item :label="$t('新列名')">
            <div class="flex flex-wrap gap-2">
              <el-input v-for="(_, i) in form.names" :key="i" v-model="form.names[i]" class="!w-28" :data-testid="`split-name-${i}`" />
            </div>
          </el-form-item>
        </template>
        <template v-else>
          <el-form-item :label="$t('合并哪些列')">
            <el-select v-model="form.mergeColumns" multiple filterable class="!w-full" :placeholder="$t('按选择的先后顺序拼接')" data-testid="merge-columns">
              <el-option v-for="c in dataset.columns" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
          </el-form-item>
          <el-form-item :label="$t('分隔符')">
            <el-input v-model="form.mergeSeparator" class="!w-28" :placeholder="$t('不填直接相连')" data-testid="merge-separator" />
            <el-checkbox v-model="form.ignoreEmpty" class="ml-3" :disabled="!form.mergeSeparator">
              {{ $t('跳过空值（不留多余的分隔符）') }}
            </el-checkbox>
          </el-form-item>
          <el-form-item :label="$t('新列名')">
            <el-input v-model="form.mergeName" class="!w-56" data-testid="merge-name" />
          </el-form-item>
        </template>
      </el-form>

      <div v-if="error" class="rounded bg-[#FEF2F2] px-3 py-2 text-[#B91C1C] leading-5" role="alert">
        {{ error }}
      </div>
      <div v-loading="loading" class="min-h-24 rounded border border-[#e5e7eb] border-solid px-3 py-2" data-testid="split-preview">
        <template v-if="form.mode === 'split' && split">
          <div class="mb-2 text-gray-600">
            {{ $t('{0} 行：{1}', [split.rows.toLocaleString(), partsText(split)]) }}
          </div>
          <table v-if="splitRows.length" class="w-full text-xs">
            <thead>
              <tr class="text-left text-gray-500">
                <th class="pb-1 pr-3 font-normal">
                  {{ form.column }}
                </th>
                <th v-for="(n, i) in form.names" :key="i" class="pb-1 pr-3 text-[#0B5A2E] font-normal">
                  {{ n }}
                </th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="(r, i) in splitRows" :key="i">
                <td v-for="(v, j) in r" :key="j" class="max-w-40 truncate py-0.5 pr-3" :class="{ 'text-[#0B5A2E]': j > 0, 'text-gray-400': j > 0 && !v }">
                  {{ j > 0 ? cell(form.trim ? v?.trim() : v) : v }}
                </td>
              </tr>
            </tbody>
          </table>
        </template>
        <template v-else-if="form.mode === 'merge' && merged">
          <table class="w-full text-xs">
            <thead>
              <tr class="text-left text-gray-500">
                <th v-for="k in Object.keys(merged.samples[0] ?? {})" :key="k" class="pb-1 pr-3 font-normal">
                  {{ k }}
                </th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="(s, i) in merged.samples.slice(0, 8)" :key="i">
                <td v-for="(v, k) in s" :key="k" class="max-w-40 truncate py-0.5 pr-3">
                  {{ cell(v) }}
                </td>
              </tr>
            </tbody>
          </table>
        </template>
        <div v-else-if="!loading && !error" class="text-gray-400">
          {{ form.mode === 'split' ? $t('选好列和分隔符后自动预览') : $t('选两列以上后自动预览') }}
        </div>
      </div>
      <div class="text-xs text-gray-500 leading-5">
        {{ $t('结果是计算列，不改原来的数据：之后导入、同步进来的行也会自动处理；不需要时在列头菜单删除计算列即可') }}
      </div>
      <div v-if="problem && (form.mode === 'split' ? split : form.mergeColumns.length)" class="text-xs text-[#B45309]">
        {{ problem }}
      </div>
    </div>

    <template #footer>
      <el-button @click="emit('update:visible', false)">
        {{ $t('关闭') }}
      </el-button>
      <el-button type="primary" :loading="applying" :disabled="!canApply" data-testid="split-apply" @click="apply">
        {{ form.mode === 'split' ? $t('添加 {0} 列', [form.count]) : $t('添加合并列') }}
      </el-button>
    </template>
  </el-dialog>
</template>
