<script setup lang="ts">
import type { DatasetInfo, DatasetValidation, OnImport, ValidationReport, ValidationRule, ValidationSuggestion } from '@/api/dataset'
import type { DraftRule } from '@/features/excel/dataset/validation-rules'
import { computed, ref, watch } from 'vue'
import { checkValidationApi, getValidationApi, saveValidationApi, suggestValidationApi } from '@/api/dataset'
import { emptyDraft, FORMATS, fromRule, RULE_KINDS, toRule } from '@/features/excel/dataset/validation-rules'
import { t } from '@/i18n'
import { errorText } from '@/utils/busy-hint'

/**
 * 数据校验：给列设必填、不能重复、取值范围、格式、只能是某几个值；保存后立即检查，
 * 之后导入、同步、清洗后自动重新检查。每条规则列出不合格的行数和样例，点「只看这些行」在数据表上筛出来
 */
const props = defineProps<{
  visible: boolean
  dataset: DatasetInfo
  /** 所有者 / 管理员才能改规则；其他人只能看、重新检查 */
  canEdit: boolean
}>()
const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 只看不符合这条规则的行 */
  (e: 'filter', rule: ValidationRule, label: string): void
  /** 规则或检查结果变了：刷新数据集列表（数据表上的小结） */
  (e: 'changed'): void
}>()

const drafts = ref<DraftRule[]>([])
const data = ref<DatasetValidation | null>(null)
const report = ref<ValidationReport | null>(null)
const loading = ref(false)
const saving = ref(false)
const checking = ref(false)
/** 上一次保存（或读取）时的规则：草稿和它不一样就是改过了 */
const savedJson = ref('[]')
/** 导入、同步时遇到不合格的行怎么办（和规则一起保存） */
const onImport = ref<OnImport>('check')
const savedOnImport = ref<OnImport>('check')
const dirty = computed(() => JSON.stringify(drafts.value.map(toRule)) !== savedJson.value || onImport.value !== savedOnImport.value)

const columns = computed(() => props.dataset.columns.filter(c => c.key !== '__contribution'))
const kindOf = (title: string) => props.dataset.columns.find(c => c.title === title)?.kind

async function load() {
  loading.value = true
  try {
    data.value = await getValidationApi(props.dataset.id)
    drafts.value = data.value.rules.map(fromRule)
    report.value = data.value.report
    savedJson.value = JSON.stringify(data.value.rules.map(r => toRule(fromRule(r))))
    onImport.value = savedOnImport.value = data.value.onImport ?? 'check'
  }
  catch (e) {
    ElMessage.error(errorText(e))
  }
  finally {
    loading.value = false
  }
}

function addRule() {
  drafts.value.push(emptyDraft(columns.value[0]?.title ?? ''))
}

// ── 推荐规则：服务端按数据本身的样子推荐（不用 AI），勾选后加进草稿，保存时才生效 ──
const suggestions = ref<ValidationSuggestion[] | null>(null)
const picked = ref<boolean[]>([])
const suggesting = ref(false)

async function suggest() {
  suggesting.value = true
  try {
    const r = await suggestValidationApi(props.dataset.id)
    // 草稿里已经有的（还没保存的）也不再推荐
    const have = new Set(drafts.value.map(d => JSON.stringify(toRule(d))))
    suggestions.value = r.suggestions.filter(x => !have.has(JSON.stringify(toRule(fromRule(x.rule)))))
    picked.value = suggestions.value.map(() => true)
    if (!suggestions.value.length)
      ElMessage.info(t('没有可推荐的规则（数据太少，或者该设的规则都有了）'))
  }
  catch (e) {
    ElMessage.error(errorText(e))
  }
  finally {
    suggesting.value = false
  }
}

function addPicked() {
  const list = suggestions.value ?? []
  drafts.value.push(...list.filter((_, i) => picked.value[i]).map(x => fromRule(x.rule)))
  suggestions.value = null
}

watch(() => props.visible, (v) => {
  if (v) {
    suggestions.value = null
    load()
  }
})

async function save() {
  saving.value = true
  try {
    data.value = await saveValidationApi(props.dataset.id, drafts.value.map(toRule), onImport.value)
    report.value = data.value.report
    drafts.value = data.value.rules.map(fromRule)
    savedJson.value = JSON.stringify(data.value.rules.map(r => toRule(fromRule(r))))
    onImport.value = savedOnImport.value = data.value.onImport ?? 'check'
    ElMessage.success(report.value?.failed ? t('已保存：{0} 条规则有不合格的行', [report.value.failed]) : drafts.value.length ? t('已保存：全部通过') : t('已清空校验规则'))
    emit('changed')
  }
  catch (e) {
    ElMessage.error(errorText(e))
  }
  finally {
    saving.value = false
  }
}

async function recheck() {
  checking.value = true
  try {
    report.value = await checkValidationApi(props.dataset.id)
    emit('changed')
  }
  catch (e) {
    ElMessage.error(errorText(e))
  }
  finally {
    checking.value = false
  }
}

function only(rule: ValidationRule, label: string) {
  emit('filter', rule, label)
  emit('update:visible', false)
}
</script>

<template>
  <el-dialog :model-value="visible" :title="$t('数据校验 · {0}', [dataset.name])" width="860px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div v-loading="loading" class="flex flex-col gap-3 text-[13px]" data-testid="validation-dialog">
      <div class="text-xs text-gray-500 leading-5">
        {{ $t('给列设规则：保存后立即检查，之后导入、同步、清洗数据时自动重新检查；有不合格的行会在数据表上提示，可以只看这些行，交给「数据清洗」处理') }}
      </div>

      <div class="flex flex-col gap-2">
        <div v-for="(d, i) in drafts" :key="i" class="flex flex-wrap items-center gap-2" :data-testid="`rule-${i}`">
          <el-select v-model="d.column" filterable size="small" class="!w-36" :disabled="!canEdit">
            <el-option v-for="c in columns" :key="c.key" :label="c.title" :value="c.title" />
          </el-select>
          <el-select v-model="d.kind" size="small" class="!w-28" :disabled="!canEdit" :data-testid="`rule-kind-${i}`">
            <el-option v-for="k in RULE_KINDS" :key="k.value" :label="k.label" :value="k.value" :disabled="k.value === 'range' && !['money', 'number', 'integer', 'datetime'].includes(kindOf(d.column) ?? '')" />
          </el-select>
          <template v-if="d.kind === 'range'">
            <el-input v-model="d.min" size="small" class="!w-32" :placeholder="kindOf(d.column) === 'datetime' ? '2026-01-01' : $t('最小（不填不限）')" :disabled="!canEdit" :data-testid="`rule-min-${i}`" />
            <span class="text-gray-400">~</span>
            <el-input v-model="d.max" size="small" class="!w-32" :placeholder="kindOf(d.column) === 'datetime' ? '2026-12-31' : $t('最大（不填不限）')" :disabled="!canEdit" :data-testid="`rule-max-${i}`" />
          </template>
          <template v-else-if="d.kind === 'format'">
            <el-select v-model="d.format" size="small" class="!w-28" :disabled="!canEdit">
              <el-option v-for="f in FORMATS" :key="f.value" :label="f.label" :value="f.value" />
            </el-select>
            <el-input v-if="d.format === 'regex'" v-model="d.pattern" size="small" class="!w-56" :placeholder="$t('正则，整格匹配，如 SKU-\\d+')" :disabled="!canEdit" />
          </template>
          <el-input v-else-if="d.kind === 'allowed'" v-model="d.values" size="small" class="!w-80" :placeholder="$t('允许的取值，用逗号分隔，如 金卡，银卡')" :disabled="!canEdit" :data-testid="`rule-values-${i}`" />
          <el-button v-if="canEdit" link size="small" type="danger" @click="drafts.splice(i, 1)">
            {{ $t('删除') }}
          </el-button>
        </div>
        <div v-if="!drafts.length && !loading" class="text-gray-400">
          {{ $t('还没有校验规则') }}
        </div>
        <div v-if="canEdit">
          <el-button size="small" data-testid="add-rule" @click="addRule">
            {{ $t('+ 添加规则') }}
          </el-button>
          <el-button size="small" :loading="suggesting" data-testid="suggest-rules" :title="$t('按数据本身的样子推荐：手机号 / 邮箱格式、编号不能重复、差不多都填了的列必填、取值很少的列只能是这几个值、金额不小于 0')" @click="suggest">
            {{ $t('推荐规则') }}
          </el-button>
        </div>
        <div v-if="suggestions?.length" class="rounded bg-[#F6FAF7] px-3 py-2" data-testid="suggestions">
          <div class="mb-1 text-xs text-gray-500">
            {{ $t('按现有数据推荐了 {0} 条规则，勾选后加进上面的列表，保存后才生效', [suggestions.length]) }}
          </div>
          <div v-for="(x, i) in suggestions" :key="i" class="flex items-center gap-2 py-0.5" :data-testid="`suggestion-${i}`">
            <el-checkbox v-model="picked[i]" size="small" />
            <span class="shrink-0">{{ x.label }}</span>
            <span class="min-w-0 truncate text-xs text-gray-500" :title="x.reason">{{ x.reason }}</span>
            <span class="ml-auto shrink-0 text-xs" :class="x.violations ? 'text-[#B91C1C]' : 'text-[#0B5A2E]'">
              {{ x.violations ? $t('现在 {0} 行不合格', [x.violations.toLocaleString()]) : $t('现在全部合格') }}
            </span>
          </div>
          <div class="mt-1 flex gap-2">
            <el-button size="small" type="primary" :disabled="!picked.some(Boolean)" data-testid="add-suggestions" @click="addPicked">
              {{ $t('添加选中的 {0} 条', [picked.filter(Boolean).length]) }}
            </el-button>
            <el-button size="small" link @click="suggestions = null">
              {{ $t('不用了') }}
            </el-button>
          </div>
        </div>
      </div>

      <div v-if="drafts.length" class="flex flex-wrap items-center gap-x-3 gap-y-1" data-testid="on-import">
        <span class="text-gray-600">{{ $t('导入、同步时发现不合格的行：') }}</span>
        <el-radio-group v-model="onImport" size="small" :disabled="!canEdit">
          <el-radio value="check">
            {{ $t('照常导入，之后提示') }}
          </el-radio>
          <el-radio value="skip" data-testid="on-import-skip">
            {{ $t('跳过这些行') }}
          </el-radio>
          <el-radio value="reject" data-testid="on-import-reject">
            {{ $t('整批不导入') }}
          </el-radio>
        </el-radio-group>
        <span v-if="onImport !== 'check'" class="text-xs text-gray-400">{{ $t('文件导入和数据连接同步时生效；计算列上的规则仍在导入后检查') }}</span>
      </div>

      <div v-if="report" class="rounded border border-[#e5e7eb] border-solid px-3 py-2" data-testid="validation-report">
        <div class="mb-2" :class="report.failed ? 'text-[#B91C1C]' : 'text-[#0B5A2E]'">
          <template v-if="report.failed">
            {{ $t('共 {0} 行，{1} 行至少有一项不合格（{2} 条规则不通过）', [report.rows.toLocaleString(), report.badRows.toLocaleString(), report.failed]) }}
          </template>
          <template v-else>
            {{ $t('共 {0} 行，全部通过', [report.rows.toLocaleString()]) }}
          </template>
          <span class="ml-2 text-xs text-gray-400">{{ $t('检查于 {0}，耗时 {1} ms', [report.checkedAt.slice(5, 16), report.elapsedMs.toLocaleString()]) }}</span>
          <span v-if="dirty" class="ml-2 text-xs text-[#B45309]">{{ $t('规则改过了，保存后重新检查') }}</span>
        </div>
        <div v-for="(r, i) in report.results" :key="i" class="flex items-center gap-2 py-0.5" :data-testid="`result-${i}`">
          <span :class="r.violations ? 'text-[#B91C1C]' : 'text-[#0B5A2E]'">{{ r.violations ? '✕' : '✓' }}</span>
          <span class="min-w-0">{{ r.label }}</span>
          <template v-if="r.violations">
            <span class="shrink-0 text-[#B91C1C]">{{ $t('{0} 行不合格', [r.violations.toLocaleString()]) }}</span>
            <span v-if="r.samples.length" class="min-w-0 truncate text-xs text-gray-500" :title="r.samples.join('、')">{{ $t('如 {0}', [r.samples.map(s => s === '' ? $t('（空）') : `「${s}」`).join('、')]) }}</span>
            <el-button link size="small" type="primary" class="ml-auto shrink-0" :data-testid="`only-${i}`" @click="only(r.rule, r.label)">
              {{ $t('只看这些行') }}
            </el-button>
          </template>
        </div>
      </div>
    </div>

    <template #footer>
      <el-button @click="emit('update:visible', false)">
        {{ $t('关闭') }}
      </el-button>
      <el-button v-if="data?.rules.length" :loading="checking" :disabled="dirty" data-testid="validation-recheck" @click="recheck">
        {{ $t('重新检查') }}
      </el-button>
      <el-button v-if="canEdit" type="primary" :loading="saving" :disabled="!dirty" data-testid="validation-save" @click="save">
        {{ $t('保存并检查') }}
      </el-button>
    </template>
  </el-dialog>
</template>
