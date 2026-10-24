<script setup lang="ts">
import type { Connector } from '@/api/connector'
import type { DatasetInfo } from '@/api/dataset'
import type { Report, ReportChannel, ReportInput, ReportPreview, ReportSchedule, ReportSheet } from '@/api/report'
import type { WorkbookMeta } from '@/api/workbook'
import { computed, reactive, ref, watch } from 'vue'
import { listConnectorsApi } from '@/api/connector'
import { getDatasetsApi } from '@/api/dataset'
import { CHANNEL_LABEL, createReportApi, previewReportApi, reportSheetsApi, updateReportApi } from '@/api/report'
import { listWorkbooksApi } from '@/api/workbook'
import { t } from '@/i18n'

/** 新建 / 修改推送任务：选择仪表盘页、计划、渠道；右侧预览推送内容（按当前数据实时计算） */
const props = defineProps<{
  visible: boolean
  /** 修改的任务；为空时新建 */
  report: Report | null
  channels: ReportChannel[]
  smtpConfigured: boolean
  /** 新建时默认选中的工作簿 / 工作表（当前打开的） */
  defaultWorkbookId?: string | null
  defaultSheetId?: string | null
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'saved', id: string, sendNow: boolean): void
  /** 去添加渠道 */
  (e: 'addChannel'): void
}>()

const WEEKDAYS = [t('周一'), t('周二'), t('周三'), t('周四'), t('周五'), t('周六'), t('周日')]

const workbooks = ref<WorkbookMeta[]>([])
const sheets = ref<ReportSheet[]>([])
const connectors = ref<Connector[]>([])
const datasets = ref<DatasetInfo[]>([])
const preview = ref<ReportPreview | null>(null)
const previewError = ref('')
const previewing = ref(false)
const previewTab = ref<'email' | 'bot'>('email')
const saving = ref(false)

const form = reactive({
  name: '',
  title: '',
  workbookId: '',
  sheetId: '',
  scheduleKind: 'daily' as ReportSchedule['kind'],
  at: '09:00',
  weekday: 1,
  day: 1,
  connectorId: '',
  triggerDatasetId: '',
  channelIds: [] as string[],
  attachExcel: false,
  enabled: true,
})

const editing = computed(() => !!props.report)
const dashboardSheets = computed(() => sheets.value.filter(s => s.parts > 0))

async function loadSheets() {
  sheets.value = form.workbookId ? await reportSheetsApi(form.workbookId).catch(() => []) : []
  if (!dashboardSheets.value.some(s => s.id === form.sheetId))
    form.sheetId = dashboardSheets.value[0]?.id ?? ''
}

watch(() => props.visible, async (v) => {
  if (!v)
    return
  preview.value = null
  previewError.value = ''
  const r = props.report
  Object.assign(form, {
    name: r?.name ?? '',
    title: r?.title ?? '',
    workbookId: r?.workbookId ?? props.defaultWorkbookId ?? '',
    sheetId: r?.sheetId ?? props.defaultSheetId ?? '',
    scheduleKind: r?.schedule.kind ?? 'daily',
    at: r && 'at' in r.schedule ? r.schedule.at : '09:00',
    weekday: r?.schedule.kind === 'weekly' ? r.schedule.weekday : 1,
    day: r?.schedule.kind === 'monthly' ? r.schedule.day : 1,
    connectorId: r?.schedule.kind === 'after_sync' ? r.schedule.connectorId : '',
    triggerDatasetId: r?.schedule.kind === 'after_data' ? r.schedule.datasetId : '',
    channelIds: r ? [...r.channelIds] : props.channels.length === 1 ? [props.channels[0]!.id] : [],
    attachExcel: r?.attachExcel ?? false,
    enabled: r?.enabled ?? true,
  })
  const [wbs, cs, ds] = await Promise.all([listWorkbooksApi().catch(() => []), listConnectorsApi().catch(() => ({ items: [] as Connector[] })), getDatasetsApi().catch(() => [] as DatasetInfo[])])
  workbooks.value = wbs
  connectors.value = cs.items
  datasets.value = ds
  if (!wbs.some(w => w.id === form.workbookId))
    form.workbookId = wbs[0]?.id ?? ''
  await loadSheets()
  if (!form.name) {
    const s = sheets.value.find(x => x.id === form.sheetId)
    form.name = s ? t('{0}推送', [s.name]) : ''
  }
  refreshPreview()
}, { immediate: true })

async function onWorkbookChange() {
  await loadSheets()
  refreshPreview()
}

let seq = 0
async function refreshPreview() {
  if (!form.workbookId || !form.sheetId) {
    preview.value = null
    return
  }
  const my = ++seq
  previewing.value = true
  try {
    const p = await previewReportApi({ workbookId: form.workbookId, sheetId: form.sheetId, title: form.title || form.name })
    if (my === seq) {
      preview.value = p
      previewError.value = ''
    }
  }
  catch (e) {
    if (my === seq)
      previewError.value = e instanceof Error ? e.message : String(e)
  }
  finally {
    if (my === seq)
      previewing.value = false
  }
}

function schedule(): ReportSchedule {
  switch (form.scheduleKind) {
    case 'daily': return { kind: 'daily', at: form.at }
    case 'weekly': return { kind: 'weekly', weekday: form.weekday, at: form.at }
    case 'monthly': return { kind: 'monthly', day: form.day, at: form.at }
    case 'after_sync': return { kind: 'after_sync', connectorId: form.connectorId }
    case 'after_data': return { kind: 'after_data', datasetId: form.triggerDatasetId }
    default: return { kind: 'manual' }
  }
}

const emailWithoutSmtp = computed(() => !props.smtpConfigured && props.channels.some(c => c.kind === 'email' && form.channelIds.includes(c.id)))
const attachUnsupported = computed(() => form.attachExcel && props.channels.some(c => (c.kind === 'dingtalk' || c.kind === 'feishu') && form.channelIds.includes(c.id)))

async function save(sendNow: boolean) {
  if (!form.name.trim())
    return ElMessage.warning(t('请填写任务名称'))
  if (!form.sheetId)
    return ElMessage.warning(t('请选择要推送的仪表盘页'))
  if (!form.channelIds.length)
    return ElMessage.warning(t('请至少选择一个推送渠道'))
  if (form.scheduleKind === 'after_sync' && !form.connectorId)
    return ElMessage.warning(t('请选择数据连接'))
  if (form.scheduleKind === 'after_data' && !form.triggerDatasetId)
    return ElMessage.warning(t('请选择数据集'))
  const input: ReportInput = {
    name: form.name.trim(),
    workbookId: form.workbookId,
    sheetId: form.sheetId,
    title: form.title.trim(),
    schedule: schedule(),
    channelIds: form.channelIds,
    attachExcel: form.attachExcel,
    enabled: form.enabled,
  }
  saving.value = true
  try {
    const id = props.report ? (await updateReportApi(props.report.id, input), props.report.id) : await createReportApi(input)
    ElMessage.success(t('已保存'))
    emit('update:visible', false)
    emit('saved', id, sendNow)
  }
  finally {
    saving.value = false
  }
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="editing ? $t('修改推送 · {0}', [report?.name]) : $t('新建定时推送')"
    width="1080px"
    :close-on-click-modal="false"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="flex gap-4">
      <el-form label-width="84px" class="w-[440px] shrink-0" @submit.prevent>
        <el-form-item :label="$t('任务名称')" required>
          <el-input v-model="form.name" maxlength="40" :placeholder="$t('如：经营日报')" />
        </el-form-item>
        <el-form-item :label="$t('仪表盘')" required>
          <div class="w-full flex gap-2">
            <el-select v-model="form.workbookId" class="flex-1" :placeholder="$t('工作簿')" filterable @change="onWorkbookChange">
              <el-option v-for="w in workbooks" :key="w.id" :label="w.name" :value="w.id" />
            </el-select>
            <el-select v-model="form.sheetId" class="w-36" :placeholder="$t('工作表')" @change="refreshPreview">
              <el-option v-for="s in dashboardSheets" :key="s.id" :label="s.name" :value="s.id">
                {{ s.name }} <span class="text-xs text-gray-400">{{ $t('{0} 个部件', [s.parts]) }}</span>
              </el-option>
            </el-select>
          </div>
          <div v-if="form.workbookId && !dashboardSheets.length" class="hint text-[#92400E]">
            {{ $t('该工作簿没有已保存的仪表盘页：先在「仪表盘」里添加指标卡 / 明细卡并保存工作簿') }}
          </div>
          <div v-else class="hint">
            {{ $t('按仪表盘上保存的切片器选择计算；「本月」「近 30 天」等按发送当天计算') }}
          </div>
        </el-form-item>
        <el-form-item :label="$t('消息标题')">
          <el-input v-model="form.title" maxlength="60" :placeholder="form.name || $t('默认用任务名称')" @change="refreshPreview" />
        </el-form-item>
        <el-form-item :label="$t('推送时间')">
          <div class="w-full flex flex-wrap items-center gap-2">
            <el-select v-model="form.scheduleKind" class="w-36">
              <el-option :label="$t('每天')" value="daily" />
              <el-option :label="$t('每周')" value="weekly" />
              <el-option :label="$t('每月')" value="monthly" />
              <el-option :label="$t('数据同步后')" value="after_sync" />
              <el-option :label="$t('数据集更新后')" value="after_data" />
              <el-option :label="$t('仅手动发送')" value="manual" />
            </el-select>
            <el-select v-if="form.scheduleKind === 'weekly'" v-model="form.weekday" class="w-24">
              <el-option v-for="(w, i) in WEEKDAYS" :key="w" :label="w" :value="i + 1" />
            </el-select>
            <el-select v-if="form.scheduleKind === 'monthly'" v-model="form.day" class="w-24">
              <el-option v-for="d in 31" :key="d" :label="$t('{0} 号', [d])" :value="d" />
            </el-select>
            <el-time-select v-if="['daily', 'weekly', 'monthly'].includes(form.scheduleKind)" v-model="form.at" class="!w-28" start="00:00" step="00:15" end="23:45" :clearable="false" />
            <el-select v-if="form.scheduleKind === 'after_sync'" v-model="form.connectorId" class="flex-1" :placeholder="$t('选择数据连接')" :no-data-text="$t('还没有数据连接')">
              <el-option v-for="c in connectors" :key="c.id" :label="c.name" :value="c.id" />
            </el-select>
            <el-select v-if="form.scheduleKind === 'after_data'" v-model="form.triggerDatasetId" filterable class="flex-1" :placeholder="$t('选择数据集')">
              <el-option v-for="d in datasets" :key="d.id" :label="d.name" :value="d.id" />
            </el-select>
          </div>
          <div v-if="form.scheduleKind === 'monthly' && form.day > 28" class="hint">
            {{ $t('没有 {0} 号的月份在月末发送', [form.day]) }}
          </div>
          <div v-if="form.scheduleKind === 'after_sync'" class="hint">
            {{ $t('该连接每次同步到新数据后推送（没有新数据不推送）') }}
          </div>
          <div v-if="form.scheduleKind === 'after_data'" class="hint">
            {{ $t('这个数据集导入了新数据、数据连接同步完成、外部文件有变化，或 SQL 查询重算写入后推送（填报提交不触发）') }}
          </div>
        </el-form-item>
        <el-form-item :label="$t('推送到')" required>
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
        <el-form-item :label="$t('附件')">
          <el-checkbox v-model="form.attachExcel">
            {{ $t('附带 Excel（指标与明细的原始数值）') }}
          </el-checkbox>
          <div v-if="attachUnsupported" class="hint">
            {{ $t('钉钉、飞书群机器人不支持文件，只有企业微信和邮件会带附件') }}
          </div>
        </el-form-item>
        <el-form-item :label="$t('状态')">
          <el-switch v-model="form.enabled" :active-text="$t('启用')" :inactive-text="$t('停用')" inline-prompt />
        </el-form-item>
      </el-form>

      <div class="min-w-0 flex flex-1 flex-col border border-[#e5e7eb] rounded-md border-solid">
        <div class="flex items-center gap-2 border-b border-b-[#e5e7eb] border-b-solid px-3 py-2 text-[13px]">
          <span class="font-medium">{{ $t('内容预览') }}</span>
          <el-radio-group v-model="previewTab" size="small">
            <el-radio-button value="email">
              {{ $t('邮件') }}
            </el-radio-button>
            <el-radio-button value="bot">
              {{ $t('群机器人') }}
            </el-radio-button>
          </el-radio-group>
          <span class="flex-1 truncate text-xs text-gray-400">{{ $t('按当前数据计算{0}', [preview ? ` · ${preview.content.generatedAt.slice(11, 16)}` : '']) }}</span>
          <el-button link size="small" :loading="previewing" @click="refreshPreview">
            {{ $t('刷新') }}
          </el-button>
        </div>
        <div v-loading="previewing && !preview" class="relative h-[460px] min-h-0">
          <div v-if="previewError" class="p-4 text-[13px] text-[#B91C1C]">
            {{ previewError }}
          </div>
          <div v-else-if="!preview" class="p-4 text-[13px] text-gray-400">
            {{ $t('选择仪表盘页后显示推送内容') }}
          </div>
          <template v-else>
            <iframe v-if="previewTab === 'email'" class="h-full w-full border-0" sandbox="allow-same-origin" :srcdoc="preview.html" :title="$t('邮件预览')" />
            <pre v-else class="m-0 h-full overflow-auto whitespace-pre-wrap bg-[#f9f9f7] p-3 text-xs leading-5">{{ preview.markdown }}</pre>
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
        {{ $t('保存并立即发送') }}
      </el-button>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.hint {
  @apply mt-1 w-full text-xs text-gray-500 leading-5;
}
</style>
