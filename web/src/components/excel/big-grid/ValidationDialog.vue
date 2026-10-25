<script setup lang="ts">
import type { DatasetInfo, DatasetValidation, ValidationReport, ValidationRule } from '@/api/dataset'
import type { DraftRule } from '@/features/excel/dataset/validation-rules'
import { computed, ref, watch } from 'vue'
import { checkValidationApi, getValidationApi, saveValidationApi } from '@/api/dataset'
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
const dirty = computed(() => JSON.stringify(drafts.value.map(toRule)) !== savedJson.value)

const columns = computed(() => props.dataset.columns.filter(c => c.key !== '__contribution'))
const kindOf = (title: string) => props.dataset.columns.find(c => c.title === title)?.kind

async function load() {
  loading.value = true
  try {
    data.value = await getValidationApi(props.dataset.id)
    drafts.value = data.value.rules.map(fromRule)
    report.value = data.value.report
    savedJson.value = JSON.stringify(data.value.rules.map(r => toRule(fromRule(r))))
  }
  catch (e) {
    ElMessage.error(errorText(e))
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, (v) => {
  if (v)
    load()
})

function addRule() {
  drafts.value.push(emptyDraft(columns.value[0]?.title ?? ''))
}

async function save() {
  saving.value = true
  try {
    data.value = await saveValidationApi(props.dataset.id, drafts.value.map(toRule))
    report.value = data.value.report
    drafts.value = data.value.rules.map(fromRule)
    savedJson.value = JSON.stringify(data.value.rules.map(r => toRule(fromRule(r))))
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
        </div>
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
