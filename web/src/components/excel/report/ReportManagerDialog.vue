<script setup lang="ts">
import type { ChannelKind, Report, ReportChannel, ReportRun } from '@/api/report'
import { useIntervalFn } from '@vueuse/core'
import { reactive, ref, watch } from 'vue'
import {
  CHANNEL_LABEL,
  createChannelApi,
  deleteChannelApi,
  deleteReportApi,
  listReportsApi,
  reportRunsApi,
  sendReportApi,
  testChannelApi,
  updateChannelApi,
  updateReportApi,
} from '@/api/report'
import ReportEditDialog from './ReportEditDialog.vue'

/** 定时推送管理：推送任务（立即发送、记录、启停）与推送渠道（群机器人 / 邮箱） */
const props = defineProps<{
  visible: boolean
  /** 当前打开的工作簿 / 工作表：新建任务时默认选中 */
  workbookId?: string | null
  sheetId?: string | null
  /** 打开后直接新建任务 */
  createOnOpen?: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 打开任务对应的仪表盘页 */
  (e: 'open', workbookId: string, sheetId: string): void
}>()

const WEEKDAYS = ['周一', '周二', '周三', '周四', '周五', '周六', '周日']

const tab = ref<'reports' | 'channels'>('reports')
const items = ref<Report[]>([])
const channels = ref<ReportChannel[]>([])
const smtpConfigured = ref(false)
const publicUrl = ref<string | null>(null)
const loading = ref(false)
const sending = ref<Set<string>>(new Set())
const editVisible = ref(false)
const editing = ref<Report | null>(null)
const expanded = ref<string | null>(null)
const runs = ref<ReportRun[]>([])
const runsLoading = ref(false)

async function load(silent = false) {
  if (!silent)
    loading.value = true
  try {
    const r = await listReportsApi()
    items.value = r.items
    channels.value = r.channels
    smtpConfigured.value = r.smtpConfigured
    publicUrl.value = r.publicUrl
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, async (v) => {
  if (!v)
    return
  await load()
  if (props.createOnOpen)
    openCreate()
}, { immediate: true })

const { pause, resume } = useIntervalFn(() => load(true), 30000, { immediate: false })
watch(() => props.visible, v => v ? resume() : pause(), { immediate: true })

function openCreate() {
  if (!channels.value.length) {
    tab.value = 'channels'
    openChannel(null)
    ElMessage.info('先添加一个推送渠道（群机器人或邮箱）')
    return
  }
  editing.value = null
  editVisible.value = true
}

function openEdit(r: Report) {
  editing.value = r
  editVisible.value = true
}

function resultText(run: ReportRun) {
  if (!run.results.length)
    return run.message ?? ''
  const failed = run.results.filter(r => !r.ok)
  if (!failed.length)
    return `已发送到 ${run.results.map(r => r.channelName).join('、')}${run.message ? `；${run.message}` : ''}`
  return failed.map(r => `${r.channelName}：${r.message}`).join('；')
}

async function sendNow(r: Report) {
  sending.value.add(r.id)
  try {
    const run = await sendReportApi(r.id)
    if (run.status === 'success')
      ElMessage.success(`「${r.name}」已发送`)
    else if (run.status === 'partial')
      ElMessage.warning({ message: `「${r.name}」部分发送失败：${resultText(run)}`, duration: 6000 })
    else
      ElMessage.error({ message: `「${r.name}」发送失败：${resultText(run)}`, duration: 6000 })
    await load(true)
    if (expanded.value === r.id)
      loadRuns(r.id)
  }
  finally {
    sending.value.delete(r.id)
  }
}

async function onSaved(id: string, send: boolean) {
  await load(true)
  const r = items.value.find(x => x.id === id)
  if (send && r)
    await sendNow(r)
}

async function toggle(r: Report) {
  const { name, workbookId, sheetId, title, schedule, channelIds, attachExcel } = r
  await updateReportApi(r.id, { name, workbookId, sheetId, title, schedule, channelIds, attachExcel, enabled: !r.enabled })
  ElMessage.success(r.enabled ? `已停用「${r.name}」` : `已启用「${r.name}」`)
  load(true)
}

async function remove(r: Report) {
  const ok = await ElMessageBox.confirm(`删除推送任务「${r.name}」及其推送记录？仪表盘本身不受影响。`, '删除推送任务', { type: 'warning', confirmButtonText: '删除' }).then(() => true, () => false)
  if (!ok)
    return
  await deleteReportApi(r.id)
  ElMessage.success('已删除')
  if (expanded.value === r.id)
    expanded.value = null
  load(true)
}

async function loadRuns(id: string) {
  runsLoading.value = true
  try {
    runs.value = await reportRunsApi(id)
  }
  finally {
    runsLoading.value = false
  }
}

function toggleRuns(r: Report) {
  if (expanded.value === r.id) {
    expanded.value = null
    return
  }
  expanded.value = r.id
  runs.value = []
  loadRuns(r.id)
}

function scheduleText(r: Report) {
  const s = r.schedule
  switch (s.kind) {
    case 'daily': return `每天 ${s.at}`
    case 'weekly': return `每${WEEKDAYS[s.weekday - 1]} ${s.at}`
    case 'monthly': return `每月 ${s.day} 号 ${s.at}`
    case 'after_sync': return '数据同步后'
    default: return '仅手动'
  }
}

function channelNames(r: Report) {
  return r.channelIds.map(id => channels.value.find(c => c.id === id)?.name ?? '（已删除）').join('、')
}

const STATUS: Record<ReportRun['status'], { text: string, cls: string }> = {
  running: { text: '发送中', cls: 'text-[#1D4ED8]' },
  success: { text: '成功', cls: 'text-[#0B5A2E]' },
  partial: { text: '部分失败', cls: 'text-[#92400E]' },
  failed: { text: '失败', cls: 'text-[#B91C1C]' },
}
const TRIGGER: Record<ReportRun['trigger'], string> = { manual: '手动', schedule: '定时', after_sync: '同步后' }

// ── 渠道 ──

const channelVisible = ref(false)
const channelEditing = ref<ReportChannel | null>(null)
const channelSaving = ref(false)
const testing = ref<string | null>(null)
const channelForm = reactive({ name: '', kind: 'wecom' as ChannelKind, target: '', secret: '', clearSecret: false })

const KIND_HINT: Record<ChannelKind, { placeholder: string, hint: string }> = {
  wecom: { placeholder: 'https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=…', hint: '群设置 → 群机器人 → 添加机器人，复制 Webhook 地址；支持附带 Excel' },
  dingtalk: { placeholder: 'https://oapi.dingtalk.com/robot/send?access_token=…', hint: '群设置 → 机器人 → 自定义机器人；安全设置建议选「加签」并填写下方密钥' },
  feishu: { placeholder: 'https://open.feishu.cn/open-apis/bot/v2/hook/…', hint: '群设置 → 群机器人 → 自定义机器人；开启「签名校验」时填写下方密钥' },
  email: { placeholder: 'cfo@example.com, finance@example.com', hint: '多个收件人用逗号分隔；发件服务器由管理员在服务端配置' },
}

function openChannel(c: ReportChannel | null) {
  channelEditing.value = c
  Object.assign(channelForm, { name: c?.name ?? '', kind: c?.kind ?? 'wecom', target: '', secret: '', clearSecret: false })
  channelVisible.value = true
}

async function saveChannel() {
  if (!channelForm.name.trim())
    return ElMessage.warning('请填写渠道名称')
  if (!channelEditing.value && !channelForm.target.trim())
    return ElMessage.warning(channelForm.kind === 'email' ? '请填写收件人' : '请填写 Webhook 地址')
  const withSecret = channelForm.kind === 'dingtalk' || channelForm.kind === 'feishu'
  const input = {
    name: channelForm.name.trim(),
    kind: channelForm.kind,
    target: channelForm.target.trim() || undefined,
    secret: !withSecret || channelForm.clearSecret ? '' : channelForm.secret.trim() || (channelEditing.value ? undefined : ''),
  }
  channelSaving.value = true
  try {
    if (channelEditing.value)
      await updateChannelApi(channelEditing.value.id, input)
    else
      await createChannelApi(input)
    ElMessage.success('已保存')
    channelVisible.value = false
    await load(true)
  }
  finally {
    channelSaving.value = false
  }
}

async function testChannel(c: ReportChannel) {
  testing.value = c.id
  try {
    await testChannelApi(c.id)
    ElMessage.success(`已向「${c.name}」发送测试消息，请到${c.kind === 'email' ? '邮箱' : '群里'}查看`)
  }
  finally {
    testing.value = null
  }
}

async function removeChannel(c: ReportChannel) {
  const ok = await ElMessageBox.confirm(`删除推送渠道「${c.name}」？`, '删除渠道', { type: 'warning', confirmButtonText: '删除' }).then(() => true, () => false)
  if (!ok)
    return
  await deleteChannelApi(c.id)
  ElMessage.success('已删除')
  load(true)
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    title="定时推送"
    width="1040px"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="mb-3 flex items-center gap-3 text-[13px]">
      <el-radio-group v-model="tab">
        <el-radio-button value="reports">
          推送任务
        </el-radio-button>
        <el-radio-button value="channels">
          推送渠道 {{ channels.length || '' }}
        </el-radio-button>
      </el-radio-group>
      <span class="flex-1 text-xs text-gray-500 leading-5">
        把仪表盘的指标、排行按计划发到企业微信 / 钉钉 / 飞书群或邮箱
        <template v-if="!publicUrl">；配置 LEDGER_PUBLIC_URL 后消息里会带「查看完整看板」链接</template>
      </span>
      <el-button v-if="tab === 'reports'" type="primary" @click="openCreate">
        + 新建推送
      </el-button>
      <el-button v-else type="primary" @click="openChannel(null)">
        + 添加渠道
      </el-button>
    </div>

    <div v-loading="loading" class="min-h-40">
      <template v-if="tab === 'reports'">
        <div v-if="!items.length && !loading" class="empty">
          <div class="text-[15px] text-gray-800 font-medium">
            还没有推送任务
          </div>
          <div class="text-xs text-gray-500 leading-5">
            例：每天 9:00 把「经营日报」仪表盘发到老板群；每周一发周报邮件并附 Excel；订单数据同步完成后立即推送
          </div>
          <el-button type="primary" class="mt-2" @click="openCreate">
            新建推送
          </el-button>
        </div>

        <div v-for="r in items" :key="r.id" class="card" :class="{ 'opacity-60': !r.enabled }">
          <div class="flex items-center gap-3">
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2">
                <span class="text-gray-900 font-medium">{{ r.name }}</span>
                <el-tag v-if="!r.enabled" size="small" type="info">
                  已停用
                </el-tag>
                <el-button v-if="r.workbookName && r.sheetName" link type="primary" size="small" class="!p-0 !text-xs" @click="emit('open', r.workbookId, r.sheetId)">
                  {{ r.workbookName }} / {{ r.sheetName }}
                </el-button>
                <span v-else class="text-xs text-[#B91C1C]">仪表盘页已删除</span>
              </div>
              <div class="mt-0.5 flex flex-wrap items-center gap-x-3 text-xs text-gray-500">
                <span>{{ scheduleText(r) }}</span>
                <span>→ {{ channelNames(r) }}</span>
                <span v-if="r.attachExcel">附 Excel</span>
                <span v-if="r.nextRun">下次 {{ r.nextRun.slice(5, 16) }}</span>
              </div>
            </div>
            <div class="w-72 text-right text-xs leading-5">
              <template v-if="r.running || sending.has(r.id)">
                <span class="text-[#1D4ED8]">发送中…</span>
              </template>
              <template v-else-if="r.lastRun">
                <div>
                  <span :class="STATUS[r.lastRun.status].cls">{{ STATUS[r.lastRun.status].text }}</span>
                  <span class="ml-1 text-gray-400">{{ TRIGGER[r.lastRun.trigger] }} {{ r.lastRun.startedAt.slice(5, 16) }}</span>
                </div>
                <div class="truncate text-gray-500" :title="resultText(r.lastRun)">
                  {{ resultText(r.lastRun) }}
                </div>
              </template>
              <span v-else class="text-gray-400">尚未发送</span>
            </div>
            <div class="flex shrink-0 items-center">
              <el-button size="small" type="primary" plain :loading="sending.has(r.id)" @click="sendNow(r)">
                立即发送
              </el-button>
              <el-button link size="small" @click="toggleRuns(r)">
                记录
              </el-button>
              <el-button link size="small" @click="openEdit(r)">
                修改
              </el-button>
              <el-button link size="small" @click="toggle(r)">
                {{ r.enabled ? '停用' : '启用' }}
              </el-button>
              <el-button link size="small" type="danger" :disabled="sending.has(r.id)" @click="remove(r)">
                删除
              </el-button>
            </div>
          </div>

          <div v-if="expanded === r.id" v-loading="runsLoading" class="mt-2 border-t border-[#eef0f3] border-t-solid pt-2">
            <el-table :data="runs" size="small" max-height="280" empty-text="暂无推送记录">
              <el-table-column label="时间" prop="startedAt" width="150" />
              <el-table-column label="方式" width="66">
                <template #default="{ row }">
                  {{ TRIGGER[row.trigger as ReportRun['trigger']] }}
                </template>
              </el-table-column>
              <el-table-column label="结果" width="80">
                <template #default="{ row }">
                  <span :class="STATUS[row.status as ReportRun['status']].cls">{{ STATUS[row.status as ReportRun['status']].text }}</span>
                </template>
              </el-table-column>
              <el-table-column label="各渠道" min-width="420">
                <template #default="{ row }">
                  <div v-if="!row.results.length" class="text-[#B91C1C]">
                    {{ row.message }}
                  </div>
                  <div v-for="x in (row as ReportRun).results" :key="x.channelId" class="leading-5">
                    <span :class="x.ok ? 'text-[#0B5A2E]' : 'text-[#B91C1C]'">{{ x.ok ? '✓' : '✗' }}</span>
                    {{ x.channelName }}
                    <span v-if="x.message" class="text-[#B91C1C]">：{{ x.message }}</span>
                  </div>
                  <div v-if="row.results.length && row.message" class="text-[#92400E]">
                    {{ row.message }}
                  </div>
                </template>
              </el-table-column>
            </el-table>
          </div>
        </div>
      </template>

      <template v-else>
        <div v-if="!channels.length && !loading" class="empty">
          <div class="text-[15px] text-gray-800 font-medium">
            还没有推送渠道
          </div>
          <div class="text-xs text-gray-500 leading-5">
            在企业微信 / 钉钉 / 飞书群里添加「自定义机器人」，把 Webhook 地址填到这里；或填写收件人邮箱
          </div>
        </div>
        <div v-for="c in channels" :key="c.id" class="card flex items-center gap-3">
          <span class="kind">{{ CHANNEL_LABEL[c.kind] }}</span>
          <div class="min-w-0 flex-1">
            <div class="text-gray-900 font-medium">
              {{ c.name }}
            </div>
            <div class="truncate text-xs text-gray-500">
              {{ c.target }}<span v-if="c.hasSecret"> · 已设置加签</span>
            </div>
          </div>
          <el-button size="small" plain :loading="testing === c.id" @click="testChannel(c)">
            发送测试
          </el-button>
          <el-button link size="small" @click="openChannel(c)">
            修改
          </el-button>
          <el-button link size="small" type="danger" @click="removeChannel(c)">
            删除
          </el-button>
        </div>
        <div v-if="!smtpConfigured" class="mt-2 text-xs text-gray-500">
          邮件渠道需要管理员在服务端配置发件服务器（LEDGER_SMTP_HOST / PORT / USER / PASSWORD / FROM），见部署手册
        </div>
      </template>
    </div>

    <ReportEditDialog
      v-model:visible="editVisible"
      :report="editing"
      :channels="channels"
      :smtp-configured="smtpConfigured"
      :default-workbook-id="workbookId"
      :default-sheet-id="sheetId"
      @saved="onSaved"
      @add-channel="openChannel(null)"
    />

    <el-dialog v-model="channelVisible" :title="channelEditing ? `修改渠道 · ${channelEditing.name}` : '添加推送渠道'" width="560px" append-to-body :close-on-click-modal="false">
      <el-form label-width="84px" @submit.prevent>
        <el-form-item label="类型">
          <el-radio-group v-model="channelForm.kind">
            <el-radio-button v-for="(label, k) in CHANNEL_LABEL" :key="k" :value="k">
              {{ label }}
            </el-radio-button>
          </el-radio-group>
        </el-form-item>
        <el-form-item label="名称" required>
          <el-input v-model="channelForm.name" maxlength="30" placeholder="如：财务群、老板邮箱" />
        </el-form-item>
        <el-form-item :label="channelForm.kind === 'email' ? '收件人' : 'Webhook'" :required="!channelEditing">
          <el-input
            v-model="channelForm.target"
            :type="channelForm.kind === 'email' ? 'textarea' : 'text'"
            :rows="2"
            :placeholder="channelEditing ? `留空沿用：${channelEditing.target}` : KIND_HINT[channelForm.kind].placeholder"
          />
          <div class="mt-1 text-xs text-gray-500 leading-5">
            {{ KIND_HINT[channelForm.kind].hint }}
          </div>
        </el-form-item>
        <el-form-item v-if="channelForm.kind === 'dingtalk' || channelForm.kind === 'feishu'" label="加签密钥">
          <el-input
            v-model="channelForm.secret"
            type="password"
            show-password
            :disabled="channelForm.clearSecret"
            :placeholder="channelEditing?.hasSecret ? '已设置，留空沿用' : '未开启加签可不填'"
          />
          <el-checkbox v-if="channelEditing?.hasSecret" v-model="channelForm.clearSecret" class="mt-1">
            清除已保存的密钥
          </el-checkbox>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="channelVisible = false">
          取消
        </el-button>
        <el-button type="primary" :loading="channelSaving" @click="saveChannel">
          保存
        </el-button>
      </template>
    </el-dialog>
  </el-dialog>
</template>

<style scoped lang="scss">
.card {
  @apply mb-2 rounded-md border border-[#e5e7eb] border-solid px-3 py-2.5 text-[13px];
}

.kind {
  @apply w-16 shrink-0 rounded bg-[#E7F4EC] py-1 text-center text-xs text-[#217346];
}

.empty {
  @apply flex flex-col items-center gap-2 rounded-md border border-dashed border-[#c8c8c8] px-6 py-10 text-center;
}
</style>
