<script setup lang="ts">
import type { Alert, AlertBasis, AlertInput, AlertMetric, AlertOp, AlertPart, AlertPreview, AlertWindowPreset } from '@/api/alert'
import type { Connector } from '@/api/connector'
import type { DatasetFilter, DatasetInfo } from '@/api/dataset'
import type { ReportChannel, ReportSchedule } from '@/api/report'
import { computed, reactive, ref, watch } from 'vue'
import { BASIS_LABEL, COMPARABLE_PRESETS, createAlertApi, OP_LABEL, previewAlertApi, updateAlertApi, WINDOW_LABEL } from '@/api/alert'
import { listConnectorsApi } from '@/api/connector'
import { getDatasetsApi } from '@/api/dataset'
import { CHANNEL_LABEL } from '@/api/report'
import { t } from '@/i18n'
import AlertPartEditor from './AlertPartEditor.vue'

/** 新建 / 修改告警：选数据集和指标、设条件和检查时间；右侧「试算」按当前数据看会命中哪些分组（不发送） */
const props = defineProps<{
  visible: boolean
  /** 修改的告警；为空时新建 */
  alert: Alert | null
  channels: ReportChannel[]
  smtpConfigured: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'saved', id: string, checkNow: boolean): void
  (e: 'addChannel'): void
}>()

const WEEKDAYS = [t('周一'), t('周二'), t('周三'), t('周四'), t('周五'), t('周六'), t('周日')]

const datasets = ref<DatasetInfo[]>([])
const connectors = ref<Connector[]>([])
const preview = ref<AlertPreview | null>(null)
const previewError = ref('')
const previewing = ref(false)
const saving = ref(false)

const emptyPart = (agg: AlertPart['agg'] = 'sum'): AlertPart => ({ agg, column: null })

const form = reactive({
  name: '',
  datasetId: '',
  metricKind: 'ratio' as 'agg' | 'ratio' | 'freshness',
  /** 数据停更：监控哪一列日期 */
  freshnessColumn: '',
  basis: 'value' as AlertBasis,
  notifyRecovery: false,
  single: emptyPart('sum'),
  numerator: emptyPart('sum'),
  denominator: emptyPart('sum'),
  metricLabel: '',
  groupBy: '',
  preset: 'last_7' as AlertWindowPreset,
  dateColumn: '',
  filters: [] as { column: string, values: string[] }[],
  op: 'gt' as AlertOp,
  /** 界面上的数值：比率按百分数填（5 表示 5%） */
  threshold: null as number | null,
  guardOn: true,
  guard: { ...emptyPart('count'), min: 30 },
  scheduleKind: 'daily' as ReportSchedule['kind'],
  at: '09:00',
  weekday: 1,
  day: 1,
  connectorId: '',
  /** 「数据集更新后」检查：哪个数据集（不选就是告警自己的数据集） */
  triggerDatasetId: '',
  channelIds: [] as string[],
  repeatKind: 'until_recovered' as 'until_recovered' | 'every_run' | 'every_hours',
  repeatHours: 24,
  enabled: true,
})

/** 上一次选的指标类型：从「数据停更」切回来时要恢复时间窗口和样本量 */
const prevKind = ref(form.metricKind)
const editing = computed(() => !!props.alert)
const dataset = computed(() => datasets.value.find(d => d.id === form.datasetId) ?? null)
const columns = computed(() => dataset.value?.columns ?? [])
const dateColumns = computed(() => columns.value.filter(c => c.kind === 'datetime'))
const groupColumns = computed(() => columns.value.filter(c => c.kind === 'text' || c.kind === 'id'))
const isFreshness = computed(() => form.metricKind === 'freshness')
const isChange = computed(() => form.basis !== 'value')
/** 界面上的数值是百分数：比率的值 / 变化，以及任何指标的相对变化 */
const isPercent = computed(() => !isFreshness.value && (form.basis === 'change_pct' || form.metricKind === 'ratio'))
const thresholdUnit = computed(() => isFreshness.value ? t('天') : isPercent.value ? (form.basis === 'change_abs' ? t('个百分点') : '%') : '')
/** 比较上期只能选长度固定的窗口；本月、不设窗口都不行 */
const presetOptions = computed(() => Object.entries(WINDOW_LABEL).filter(([k]) => !isChange.value || COMPARABLE_PRESETS.includes(k as AlertWindowPreset)) as [AlertWindowPreset, string][])
const emailWithoutSmtp = computed(() => !props.smtpConfigured && props.channels.some(c => c.kind === 'email' && form.channelIds.includes(c.id)))

function fillFrom(a: Alert | null) {
  const m = a?.metric
  const basis = a?.condition.basis ?? 'value'
  Object.assign(form, {
    name: a?.name ?? '',
    datasetId: a?.datasetId ?? '',
    metricKind: m?.kind ?? 'ratio',
    freshnessColumn: m?.kind === 'freshness' ? m.dateColumn : '',
    basis,
    notifyRecovery: a?.notifyRecovery ?? false,
    single: m?.kind === 'agg' ? { agg: m.agg, column: m.column ?? null } : emptyPart('sum'),
    numerator: m?.kind === 'ratio' ? { ...m.numerator } : emptyPart('sum'),
    denominator: m?.kind === 'ratio' ? { ...m.denominator } : emptyPart('sum'),
    metricLabel: a?.metricLabel ?? '',
    groupBy: a?.groupBy ?? '',
    preset: a?.window.preset ?? 'last_7',
    dateColumn: a?.window.dateColumn ?? '',
    filters: (a?.filters ?? []).flatMap(f => f.op === 'in' ? [{ column: f.column, values: [...f.values] }] : []),
    op: a?.condition.op ?? 'gt',
    threshold: a ? (m?.kind !== 'freshness' && (m?.kind === 'ratio' || basis === 'change_pct') ? Math.round(a.condition.threshold * 1e6) / 1e4 : a.condition.threshold) : null,
    guardOn: a ? !!a.guard : true,
    guard: a?.guard ? { agg: a.guard.agg, column: a.guard.column ?? null, min: a.guard.min } : { ...emptyPart('count'), min: 30 },
    scheduleKind: a?.schedule.kind ?? 'daily',
    at: a && 'at' in a.schedule ? a.schedule.at : '09:00',
    weekday: a?.schedule.kind === 'weekly' ? a.schedule.weekday : 1,
    day: a?.schedule.kind === 'monthly' ? a.schedule.day : 1,
    connectorId: a?.schedule.kind === 'after_sync' ? a.schedule.connectorId : '',
    triggerDatasetId: a?.schedule.kind === 'after_data' ? a.schedule.datasetId : '',
    channelIds: a ? [...a.channelIds] : props.channels.length === 1 ? [props.channels[0]!.id] : [],
    repeatKind: a?.repeat.kind ?? 'until_recovered',
    repeatHours: a?.repeat.kind === 'every_hours' ? a.repeat.hours : 24,
    enabled: a?.enabled ?? true,
  })
  prevKind.value = form.metricKind
}

watch(() => props.visible, async (v) => {
  if (!v)
    return
  preview.value = null
  previewError.value = ''
  fillFrom(props.alert)
  const [ds, cs] = await Promise.all([getDatasetsApi().catch(() => [] as DatasetInfo[]), listConnectorsApi().catch(() => ({ items: [] as Connector[] }))])
  datasets.value = ds
  connectors.value = cs.items
  if (!props.alert && !ds.some(d => d.id === form.datasetId))
    form.datasetId = ds[0]?.id ?? ''
  if (!form.dateColumn)
    form.dateColumn = dateColumns.value[0]?.key ?? ''
}, { immediate: true })

/** 换数据集：列都变了，清掉依赖旧数据集的选择 */
function onDatasetChange() {
  Object.assign(form, { single: emptyPart('sum'), numerator: emptyPart('sum'), denominator: emptyPart('sum'), groupBy: '', filters: [], dateColumn: dateColumns.value[0]?.key ?? '', freshnessColumn: dateColumns.value[0]?.key ?? '' })
  form.guard = { ...emptyPart('count'), min: 30 }
  preview.value = null
}

/** 换指标类型：数据停更没有时间窗口、样本量和「较上期」；阈值的单位也跟着变，清掉重填 */
function onMetricKindChange() {
  const was = prevKind.value
  prevKind.value = form.metricKind
  preview.value = null
  form.threshold = null
  if (isFreshness.value) {
    form.basis = 'value'
    form.preset = 'none'
    form.guardOn = false
    form.op = 'gt'
    form.freshnessColumn ||= dateColumns.value[0]?.key ?? ''
  }
  else if (was === 'freshness') {
    form.preset = 'last_7'
    form.guardOn = true
  }
}

/** 换判断依据：比较上期需要固定长度的时间窗口；阈值的单位变了，清掉重填 */
function onBasisChange() {
  preview.value = null
  form.threshold = null
  if (isChange.value && !COMPARABLE_PRESETS.includes(form.preset))
    form.preset = 'last_7'
  if (isChange.value && !form.dateColumn)
    form.dateColumn = dateColumns.value[0]?.key ?? ''
  // 比较上期时「大于」「小于」更常用：下降 → 小于
  form.op = form.basis === 'change_pct' ? 'lt' : 'gt'
}

function addFilter() {
  form.filters.push({ column: '', values: [] })
}

function schedule(): ReportSchedule {
  switch (form.scheduleKind) {
    case 'daily': return { kind: 'daily', at: form.at }
    case 'weekly': return { kind: 'weekly', weekday: form.weekday, at: form.at }
    case 'monthly': return { kind: 'monthly', day: form.day, at: form.at }
    case 'after_sync': return { kind: 'after_sync', connectorId: form.connectorId }
    case 'after_data': return { kind: 'after_data', datasetId: form.triggerDatasetId || form.datasetId }
    default: return { kind: 'manual' }
  }
}

function partOk(p: AlertPart) {
  return p.agg === 'count' || !!p.column
}

/** 表单 → 提交内容；不完整时返回提示 */
function build(forPreview: boolean): AlertInput | string {
  if (!forPreview && !form.name.trim())
    return t('请填写告警名称')
  if (!form.datasetId)
    return t('请选择数据集')
  let metric: AlertMetric
  if (form.metricKind === 'freshness') {
    if (!form.freshnessColumn)
      return t('请选择日期列')
    metric = { kind: 'freshness', dateColumn: form.freshnessColumn }
  }
  else if (form.metricKind === 'agg') {
    if (!partOk(form.single))
      return t('请选择要监控的列')
    metric = { kind: 'agg', agg: form.single.agg, column: form.single.column }
  }
  else {
    if (!partOk(form.numerator) || !partOk(form.denominator))
      return t('请选择分子和分母的列')
    metric = { kind: 'ratio', numerator: { ...form.numerator }, denominator: { ...form.denominator } }
  }
  if (form.threshold === null || Number.isNaN(form.threshold))
    return t('请填写阈值')
  if (!isFreshness.value && form.preset !== 'none' && !form.dateColumn)
    return t('设置了时间窗口，请选择日期列')
  if (isChange.value && !COMPARABLE_PRESETS.includes(form.preset))
    return t('比较上期需要选择时间窗口：昨天、近 7 天、近 30 天或上月')
  if (form.guardOn && !isFreshness.value && !partOk(form.guard))
    return t('请选择样本量统计的列')
  if (!forPreview) {
    if (!form.channelIds.length)
      return t('请至少选择一个推送渠道')
    if (form.scheduleKind === 'after_sync' && !form.connectorId)
      return t('请选择数据连接')
  }
  const filters: DatasetFilter[] = form.filters.filter(f => f.column && f.values.length).map(f => ({ op: 'in', column: f.column, values: f.values }))
  return {
    name: form.name.trim() || t('试算'),
    datasetId: form.datasetId,
    metric,
    metricLabel: form.metricLabel.trim() || undefined,
    groupBy: form.groupBy || null,
    filters,
    window: { preset: form.preset, dateColumn: form.preset === 'none' ? null : form.dateColumn },
    condition: { op: form.op, threshold: isPercent.value ? Math.round(form.threshold * 1e4) / 1e6 : form.threshold, basis: form.basis },
    guard: form.guardOn && !isFreshness.value ? { agg: form.guard.agg, column: form.guard.column, min: form.guard.min } : null,
    schedule: schedule(),
    channelIds: form.channelIds,
    repeat: form.repeatKind === 'every_hours' ? { kind: 'every_hours', hours: form.repeatHours } : { kind: form.repeatKind },
    notifyRecovery: form.notifyRecovery,
    enabled: form.enabled,
  }
}

async function runPreview() {
  const input = build(true)
  if (typeof input === 'string') {
    previewError.value = input
    preview.value = null
    return
  }
  previewing.value = true
  previewError.value = ''
  try {
    preview.value = await previewAlertApi(input)
  }
  catch (e) {
    preview.value = null
    previewError.value = e instanceof Error ? e.message : String(e)
  }
  finally {
    previewing.value = false
  }
}

async function save(checkNow: boolean) {
  const input = build(false)
  if (typeof input === 'string')
    return ElMessage.warning(input)
  saving.value = true
  try {
    const id = props.alert ? (await updateAlertApi(props.alert.id, input), props.alert.id) : await createAlertApi(input)
    ElMessage.success(t('已保存'))
    emit('update:visible', false)
    emit('saved', id, checkNow)
  }
  finally {
    saving.value = false
  }
}

const VERDICT: Record<string, { text: string, cls: string }> = {
  hit: { text: t('超出条件'), cls: 'text-[#B91C1C]' },
  clear: { text: t('正常'), cls: 'text-[#0B5A2E]' },
  unknown: { text: t('样本不足 / 无数据'), cls: 'text-gray-400' },
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="editing ? $t('修改告警 · {0}', [alert?.name]) : $t('新建告警')"
    width="1120px"
    :close-on-click-modal="false"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="flex gap-4">
      <el-form label-width="84px" class="w-[500px] shrink-0" @submit.prevent>
        <el-form-item :label="$t('告警名称')" required>
          <el-input v-model="form.name" maxlength="60" :placeholder="$t('如：店铺退款率偏高')" />
        </el-form-item>
        <el-form-item :label="$t('数据集')" required>
          <el-select v-model="form.datasetId" class="w-full" filterable :placeholder="$t('选择要监控的数据集')" @change="onDatasetChange">
            <el-option v-for="d in datasets" :key="d.id" :label="d.name" :value="d.id" />
          </el-select>
        </el-form-item>

        <el-form-item :label="$t('监控指标')" required>
          <div class="w-full">
            <el-radio-group v-model="form.metricKind" size="small" class="mb-2" @change="onMetricKindChange">
              <el-radio-button value="ratio">
                {{ $t('比率（如退款率）') }}
              </el-radio-button>
              <el-radio-button value="agg">
                {{ $t('汇总值（如金额）') }}
              </el-radio-button>
              <el-radio-button value="freshness">
                {{ $t('数据停更') }}
              </el-radio-button>
            </el-radio-group>
            <template v-if="form.metricKind === 'freshness'">
              <el-select v-model="form.freshnessColumn" class="w-full" filterable :placeholder="$t('选择日期列')" :no-data-text="$t('这个数据集没有日期列')" @change="preview = null">
                <el-option v-for="c in dateColumns" :key="c.key" :label="c.title" :value="c.key" />
              </el-select>
              <div class="hint">
                {{ $t('看这一列最新的一条数据距今天几天。按店铺等分组时，逐个看谁多久没有新数据了（某个分组从来没有数据则看不到）') }}
              </div>
            </template>
            <template v-else-if="form.metricKind === 'agg'">
              <AlertPartEditor v-model="form.single" :columns="columns" />
            </template>
            <template v-else>
              <div class="mb-1 text-xs text-gray-500">
                {{ $t('分子') }}
              </div>
              <AlertPartEditor v-model="form.numerator" :columns="columns" />
              <div class="mb-1 mt-2 text-xs text-gray-500">
                {{ $t('分母') }}
              </div>
              <AlertPartEditor v-model="form.denominator" :columns="columns" />
              <div class="hint">
                {{ $t('先分别汇总再相除（不是各行比率的平均）；分母为 0 的分组不判断') }}
              </div>
            </template>
            <el-input v-if="!isFreshness" v-model="form.metricLabel" class="mt-2" maxlength="30" :placeholder="$t('指标名称（可选），如：退款率')" />
          </div>
        </el-form-item>

        <el-form-item :label="$t('按…分组')">
          <el-select v-model="form.groupBy" class="w-full" clearable filterable :placeholder="$t('不分组：整体算一个值')" @change="preview = null">
            <el-option v-for="c in groupColumns" :key="c.key" :label="c.title" :value="c.key" />
          </el-select>
          <div class="hint">
            {{ $t('选了分组列，会逐个取值（如每家店铺）分别判断，列出所有超出条件的') }}
          </div>
        </el-form-item>

        <el-form-item v-if="!isFreshness" :label="$t('时间窗口')">
          <div class="w-full flex gap-2">
            <el-select v-model="form.preset" class="min-w-0 flex-1" @change="preview = null">
              <el-option v-for="[k, label] in presetOptions" :key="k" :label="label" :value="k" />
            </el-select>
            <el-select v-if="form.preset !== 'none'" v-model="form.dateColumn" class="!w-40" :placeholder="$t('日期列')" @change="preview = null">
              <el-option v-for="c in dateColumns" :key="c.key" :label="c.title" :value="c.key" />
            </el-select>
          </div>
          <div v-if="form.preset !== 'none'" class="hint">
            {{ isChange ? $t('本期是这个窗口，上期是紧挨着它之前、一样长的一段（如近 7 天 对比 再往前的 7 天）') : $t('窗口按检查当天计算，默认不含今天（今天的数据还没跑完）') }}
          </div>
        </el-form-item>

        <el-form-item :label="$t('筛选')">
          <div class="w-full">
            <div v-for="(f, i) in form.filters" :key="i" class="mb-1 flex gap-2">
              <el-select v-model="f.column" class="!w-36" filterable :placeholder="$t('列')" @change="preview = null">
                <el-option v-for="c in columns" :key="c.key" :label="c.title" :value="c.key" />
              </el-select>
              <el-select v-model="f.values" class="min-w-0 flex-1" multiple filterable allow-create default-first-option collapse-tags collapse-tags-tooltip :placeholder="$t('等于（输入后回车，可多个）')" @change="preview = null" />
              <el-button link type="danger" @click="form.filters.splice(i, 1)">
                {{ $t('删除') }}
              </el-button>
            </div>
            <el-button link type="primary" size="small" @click="addFilter">
              {{ $t('+ 添加筛选') }}
            </el-button>
          </div>
        </el-form-item>

        <el-form-item :label="$t('触发条件')" required>
          <div class="w-full flex flex-wrap items-center gap-2">
            <el-select v-if="!isFreshness" v-model="form.basis" class="!w-52" @change="onBasisChange">
              <el-option v-for="(label, k) in BASIS_LABEL" :key="k" :label="label" :value="k" />
            </el-select>
            <span v-else class="shrink-0 text-[13px] text-gray-600">{{ $t('最新数据距今') }}</span>
            <el-select v-model="form.op" class="!w-32" @change="preview = null">
              <el-option v-for="(label, k) in OP_LABEL" :key="k" :label="label" :value="k" />
            </el-select>
            <el-input-number v-model="form.threshold" class="!w-36" :controls="false" :precision="isPercent ? 2 : isFreshness ? 0 : undefined" :placeholder="$t('阈值')" @change="preview = null" />
            <span v-if="thresholdUnit" class="text-[13px] text-gray-600">{{ thresholdUnit }}</span>
          </div>
          <div v-if="isChange" class="hint">
            {{ form.basis === 'change_pct' ? $t('下降 20% 写：小于 -20；上期为 0 的分组不判断。销量掉到 0 的分组也会抓到（求和 / 计数类指标）') : $t('比率的变化按百分点算：退款率由 4% 升到 8% 是上升 4 个百分点') }}
          </div>
        </el-form-item>

        <el-form-item v-if="!isFreshness" :label="$t('最小样本量')">
          <div class="w-full">
            <el-checkbox v-model="form.guardOn" @change="preview = null">
              {{ $t('样本太小的分组不判断') }}
            </el-checkbox>
            <template v-if="form.guardOn">
              <div class="mt-1 flex items-center gap-2">
                <AlertPartEditor v-model="form.guard" class="min-w-0 flex-1" :columns="columns" />
                <span class="shrink-0 text-[13px] text-gray-600">≥</span>
                <el-input-number v-model="form.guard.min" class="!w-28" :controls="false" :min="0" @change="preview = null" />
              </div>
              <div class="hint">
                {{ $t('例：订单数至少 30。不设的话，2 单里退了 1 单的店铺会显示退款率 50% 并触发告警') }}
              </div>
              <div v-if="isChange" class="hint">
                {{ $t('比较上期时：求和 / 计数类指标只看上期够不够（本期掉得很少正是要抓的），其他指标本期和上期都要够') }}
              </div>
            </template>
          </div>
        </el-form-item>

        <el-form-item :label="$t('检查时间')">
          <div class="w-full flex flex-wrap items-center gap-2">
            <el-select v-model="form.scheduleKind" class="!w-36">
              <el-option :label="$t('每天')" value="daily" />
              <el-option :label="$t('每周')" value="weekly" />
              <el-option :label="$t('每月')" value="monthly" />
              <el-option :label="$t('数据同步后')" value="after_sync" />
              <el-option :label="$t('数据集更新后')" value="after_data" />
              <el-option :label="$t('仅手动检查')" value="manual" />
            </el-select>
            <el-select v-if="form.scheduleKind === 'weekly'" v-model="form.weekday" class="!w-24">
              <el-option v-for="(w, i) in WEEKDAYS" :key="w" :label="w" :value="i + 1" />
            </el-select>
            <el-select v-if="form.scheduleKind === 'monthly'" v-model="form.day" class="!w-24">
              <el-option v-for="d in 31" :key="d" :label="$t('{0} 号', [d])" :value="d" />
            </el-select>
            <el-time-select v-if="['daily', 'weekly', 'monthly'].includes(form.scheduleKind)" v-model="form.at" class="!w-28" start="00:00" step="00:15" end="23:45" :clearable="false" />
            <el-select v-if="form.scheduleKind === 'after_sync'" v-model="form.connectorId" class="min-w-0 flex-1" :placeholder="$t('选择数据连接')" :no-data-text="$t('还没有数据连接')">
              <el-option v-for="c in connectors" :key="c.id" :label="c.name" :value="c.id" />
            </el-select>
            <el-select v-if="form.scheduleKind === 'after_data'" v-model="form.triggerDatasetId" filterable clearable class="min-w-0 flex-1" :placeholder="$t('告警用的数据集（{0}）', [dataset?.name ?? ''])">
              <el-option v-for="d in datasets" :key="d.id" :label="d.name" :value="d.id" />
            </el-select>
          </div>
          <div v-if="form.scheduleKind === 'after_sync'" class="hint">
            {{ $t('该连接每次同步到新数据后检查（没有新数据不检查）') }}
          </div>
          <div v-if="form.scheduleKind === 'after_data'" class="hint">
            {{ $t('这个数据集导入了新数据、数据连接同步完成、外部文件有变化，或 SQL 查询重算写入后检查（填报提交不触发）') }}
          </div>
        </el-form-item>

        <el-form-item :label="$t('通知到')" required>
          <div class="w-full">
            <el-checkbox-group v-if="channels.length" v-model="form.channelIds" class="flex flex-col">
              <el-checkbox v-for="c in channels" :key="c.id" :value="c.id" class="!mr-0 !h-7">
                {{ c.name }} <span class="text-xs text-gray-400">{{ CHANNEL_LABEL[c.kind] }} · {{ c.target }}</span>
              </el-checkbox>
            </el-checkbox-group>
            <div v-else class="text-xs text-gray-500">
              {{ $t('还没有推送渠道') }}
            </div>
            <el-button link type="primary" size="small" @click="emit('addChannel')">
              {{ $t('+ 添加渠道') }}
            </el-button>
            <div v-if="emailWithoutSmtp" class="hint text-[#B91C1C]">
              {{ $t('服务端尚未配置邮件发送（LEDGER_SMTP_HOST 等），邮件渠道会发送失败') }}
            </div>
          </div>
        </el-form-item>

        <el-form-item :label="$t('重复提醒')">
          <div class="w-full flex flex-wrap items-center gap-2">
            <el-select v-model="form.repeatKind" class="!w-72">
              <el-option :label="$t('只在新超标时通知一次（推荐）')" value="until_recovered" />
              <el-option :label="$t('每次检查仍超标都通知')" value="every_run" />
              <el-option :label="$t('持续超标时每隔一段时间再提醒')" value="every_hours" />
            </el-select>
            <template v-if="form.repeatKind === 'every_hours'">
              <el-input-number v-model="form.repeatHours" class="!w-28" :min="1" :max="720" :controls="false" />
              <span class="text-[13px] text-gray-600">{{ $t('小时') }}</span>
            </template>
          </div>
          <div class="hint">
            {{ $t('恢复正常后再次超标，会重新通知') }}
          </div>
          <el-checkbox v-model="form.notifyRecovery" class="mt-1">
            {{ $t('恢复正常时也通知一下') }}
          </el-checkbox>
        </el-form-item>

        <el-form-item :label="$t('状态')">
          <el-switch v-model="form.enabled" :active-text="$t('启用')" :inactive-text="$t('停用')" inline-prompt />
        </el-form-item>
      </el-form>

      <div class="min-w-0 flex flex-1 flex-col border border-[#e5e7eb] rounded-md border-solid">
        <div class="flex items-center gap-2 border-b border-b-[#e5e7eb] border-b-solid px-3 py-2 text-[13px]">
          <span class="font-medium">{{ $t('试算') }}</span>
          <span class="flex-1 truncate text-xs text-gray-400">{{ $t('按当前数据算一遍，不保存、不发送') }}</span>
          <el-button type="primary" size="small" :loading="previewing" @click="runPreview">
            {{ $t('试算') }}
          </el-button>
        </div>
        <div v-loading="previewing" class="relative h-[640px] min-h-0 overflow-auto p-3 text-[13px]">
          <div v-if="previewError" class="text-[#B91C1C]">
            {{ previewError }}
          </div>
          <div v-else-if="!preview" class="text-gray-400 leading-6">
            {{ $t('设好指标和条件后点「试算」，看现在有哪些分组会触发告警。阈值定多少合适，先看一眼再定。') }}
          </div>
          <template v-else>
            <div class="mb-2 leading-6">
              <div>
                <b>{{ preview.hits }}</b> {{ $t('项超出条件') }}
                <span class="text-gray-500">{{ $t('· 正常 {0} · 样本不足 / 无数据 {1} · 共 {2} 组', [preview.clear, preview.unknown, preview.totalGroups]) }}</span>
              </div>
              <div class="text-xs text-gray-500">
                {{ preview.condition }}<template v-if="preview.guard">
                  {{ $t('；') }}{{ preview.guard }}
                </template>
              </div>
              <div v-if="preview.window" class="text-xs text-gray-500">
                {{ preview.window }}
              </div>
            </div>
            <el-table :data="preview.rows" size="small" :empty-text="$t('没有数据')">
              <el-table-column :label="form.groupBy ? (columns.find(c => c.key === form.groupBy)?.title ?? $t('分组')) : $t('整体')" :min-width="isChange ? 90 : 140" show-overflow-tooltip>
                <template #default="{ row }">
                  {{ row.key || $t('整体') }}
                </template>
              </el-table-column>
              <el-table-column :label="isChange ? $t('本期') : preview.metricLabel" prop="valueText" :width="isChange ? 100 : 130" align="right" show-overflow-tooltip />
              <el-table-column v-if="isChange" :label="$t('上期')" prop="prevText" width="100" align="right" show-overflow-tooltip />
              <el-table-column v-if="isChange" :label="$t('变化')" prop="changeText" width="90" align="right" show-overflow-tooltip />
              <el-table-column v-if="form.guardOn && !isFreshness && !isChange" :label="$t('样本量')" width="86" align="right">
                <template #default="{ row }">
                  {{ row.guard ?? '—' }}
                </template>
              </el-table-column>
              <el-table-column :label="$t('结果')" :width="isChange ? 100 : 120">
                <template #default="{ row }">
                  <span :class="VERDICT[row.verdict]?.cls">{{ VERDICT[row.verdict]?.text }}</span>
                </template>
              </el-table-column>
            </el-table>
            <div v-if="preview.totalGroups > preview.rows.length" class="mt-1 text-xs text-gray-400">
              {{ $t('只显示前 {0} 组（超出条件的排在最前）', [preview.rows.length]) }}
            </div>
            <div class="mt-2 text-xs text-gray-400">
              {{ $t('耗时 {0} 毫秒', [preview.elapsedMs]) }}
            </div>
          </template>
        </div>
      </div>
    </div>

    <template #footer>
      <el-button @click="emit('update:visible', false)">
        {{ $t('取消') }}
      </el-button>
      <el-button :loading="saving" @click="save(false)">
        {{ $t('保存') }}
      </el-button>
      <el-button type="primary" :loading="saving" @click="save(true)">
        {{ $t('保存并立即检查') }}
      </el-button>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.hint {
  @apply mt-1 w-full text-xs text-gray-500 leading-5;
}
</style>
