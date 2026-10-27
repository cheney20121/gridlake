<script setup lang="ts">
import type { Webhook, WebhookDelivery, WebhookEvent } from '@/api/webhook'
import { computed, reactive, ref, watch } from 'vue'
import { createWebhookApi, deleteWebhookApi, EVENT_LABEL, listWebhooksApi, redeliverWebhookApi, rotateWebhookSecretApi, testWebhookApi, updateWebhookApi, webhookDeliveriesApi } from '@/api/webhook'
import { t as tr } from '@/i18n'
import { errorText } from '@/utils/busy-hint'

/** 事件 Webhook：登记接收地址与订阅的事件、测试、看发送记录与重发、重置签名密钥 */
const props = defineProps<{
  visible: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
}>()

const hooks = ref<Webhook[]>([])
const events = ref<WebhookEvent[]>([])
const loading = ref(false)
/** 刚创建 / 重置的签名密钥（只在本次打开期间显示） */
const secret = ref('')
const secretName = ref('')
const showGuide = ref(false)
/** 正在看哪个 Webhook 的发送记录（null：列表） */
const viewing = ref<Webhook | null>(null)
const deliveries = ref<WebhookDelivery[]>([])
const deliveriesLoading = ref(false)

async function load() {
  loading.value = true
  try {
    const r = await listWebhooksApi()
    hooks.value = r.hooks
    events.value = r.events
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, (v) => {
  if (v) {
    secret.value = ''
    viewing.value = null
    load()
  }
}, { immediate: true })

const label = (e: string) => EVENT_LABEL[e] ?? e
const eventsText = (h: Webhook) => h.events.map(label).join('、')

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    ElMessage.success(tr('已复制'))
  }
  catch {
    ElMessage.warning(tr('浏览器不允许自动复制，请手动选中复制'))
  }
}

// ── 新建 / 修改 ──

const editVisible = ref(false)
const editing = ref<Webhook | null>(null)
const saving = ref(false)
const form = reactive({ name: '', url: '', all: false, events: [] as string[], enabled: true })

function openEdit(h: Webhook | null) {
  editing.value = h
  const all = h?.events.includes('*') ?? false
  Object.assign(form, { name: h?.name ?? '', url: '', all, events: all ? [] : [...(h?.events ?? [])], enabled: h?.enabled ?? true })
  editVisible.value = true
}

async function save() {
  if (!form.name.trim())
    return ElMessage.warning(tr('请填写名称，如「数据管道」'))
  if (!editing.value && !form.url.trim())
    return ElMessage.warning(tr('请填写接收地址'))
  if (!form.all && !form.events.length)
    return ElMessage.warning(tr('请至少选择一个事件'))
  const input = { name: form.name.trim(), url: form.url.trim() || undefined, events: form.all ? ['*'] : form.events, enabled: form.enabled }
  saving.value = true
  try {
    if (editing.value) {
      await updateWebhookApi(editing.value.id, input)
      ElMessage.success(tr('已保存'))
    }
    else {
      const r = await createWebhookApi(input)
      secret.value = r.secret
      secretName.value = input.name
      showGuide.value = true
    }
    editVisible.value = false
    load()
  }
  finally {
    saving.value = false
  }
}

async function remove(h: Webhook) {
  const ok = await ElMessageBox.confirm(tr('删除 Webhook「{0}」？排队中的事件不再发送，发送记录一起删除。', [h.name]), tr('删除 Webhook'), { type: 'warning', confirmButtonText: tr('删除') }).then(() => true, () => false)
  if (!ok)
    return
  await deleteWebhookApi(h.id)
  ElMessage.success(tr('已删除'))
  load()
}

async function rotate(h: Webhook) {
  const ok = await ElMessageBox.confirm(tr('重置「{0}」的签名密钥？旧密钥立即失效，接收方要换成新密钥才能验证通过。', [h.name]), tr('重置签名密钥'), { type: 'warning', confirmButtonText: tr('重置') }).then(() => true, () => false)
  if (!ok)
    return
  secret.value = await rotateWebhookSecretApi(h.id)
  secretName.value = h.name
}

const testing = ref('')
async function test(h: Webhook) {
  testing.value = h.id
  try {
    const d = await testWebhookApi(h.id)
    if (d.status === 'success')
      ElMessage.success(tr('测试成功：对方返回 HTTP {0}', [d.httpStatus ?? '']))
    else
      ElMessage.error(tr('测试失败：{0}', [d.error ?? '']))
    load()
  }
  catch (e) {
    ElMessage.error(errorText(e))
  }
  finally {
    testing.value = ''
  }
}

async function toggle(h: Webhook, enabled: boolean) {
  await updateWebhookApi(h.id, { name: h.name, events: h.events, enabled })
  ElMessage.success(enabled ? tr('已启用') : tr('已停用，排队中的事件不再发送'))
  load()
}

// ── 发送记录 ──

async function openDeliveries(h: Webhook) {
  viewing.value = h
  deliveriesLoading.value = true
  try {
    deliveries.value = await webhookDeliveriesApi(h.id)
  }
  finally {
    deliveriesLoading.value = false
  }
}

async function redeliver(d: WebhookDelivery) {
  await redeliverWebhookApi(d.id)
  ElMessage.success(tr('已放回队列，马上重新发送'))
  // 发送是异步的：稍等一下再刷新
  setTimeout(() => viewing.value && openDeliveries(viewing.value), 1500)
}

const STATUS: Record<WebhookDelivery['status'], { text: string, cls: string }> = {
  success: { text: tr('成功'), cls: 'text-[#0B5A2E]' },
  failed: { text: tr('失败'), cls: 'text-[#B91C1C]' },
  pending: { text: tr('等待重试'), cls: 'text-[#92400E]' },
  cancelled: { text: tr('已取消'), cls: 'text-gray-400' },
}

const pretty = (v: unknown) => JSON.stringify(v, null, 2)

// ── 验证签名的示例 ──

const verifySnippet = computed(() => `# Python: verify X-Gridlake-Signature (t=<unix seconds>,v1=<hex HMAC-SHA256>)
import hmac, hashlib, time

def verify(secret: str, header: str, body: bytes) -> bool:
    parts = dict(p.split("=", 1) for p in header.split(","))
    signed = parts["t"].encode() + b"." + body
    want = hmac.new(secret.encode(), signed, hashlib.sha256).hexdigest()
    fresh = abs(time.time() - int(parts["t"])) < 300   # reject stale requests (replay)
    return fresh and hmac.compare_digest(want, parts["v1"])

# ${tr('用原始请求体（不要先解析成 JSON 再序列化）；按 X-Gridlake-Delivery 去重')}`)
</script>

<template>
  <el-dialog :model-value="visible" :title="$t('事件 Webhook')" width="980px" append-to-body @update:model-value="emit('update:visible', $event)">
    <template v-if="!viewing">
      <div class="mb-3 flex items-center gap-3 text-[13px]">
        <span class="flex-1 text-xs text-gray-500 leading-5">
          {{ $t('数据集导入、数据连接同步、填报提交、审批、告警等事件发生时，把一份 JSON POST 到你的地址：触发数据管道、通知工单系统、跑自己的脚本。只发元信息（id、名称、行数、操作人），不发数据行；失败自动重试，带签名可验证。') }}
        </span>
        <el-button @click="showGuide = !showGuide">
          {{ $t('如何接收') }}
        </el-button>
        <el-button type="primary" data-testid="webhook-new" @click="openEdit(null)">
          {{ $t('+ 新建 Webhook') }}
        </el-button>
      </div>

      <div v-if="secret" class="secret" data-testid="webhook-secret">
        <div class="mb-1 font-medium">
          {{ $t('「{0}」的签名密钥，请立即复制给接收方——关闭后无法再次查看', [secretName]) }}
        </div>
        <div class="flex items-center gap-2">
          <code class="min-w-0 flex-1 truncate">{{ secret }}</code>
          <el-button size="small" type="primary" @click="copy(secret)">
            {{ $t('复制密钥') }}
          </el-button>
        </div>
      </div>

      <div v-if="showGuide" class="guide">
        <div class="mb-1 text-xs text-gray-600 leading-5">
          {{ $t('请求：POST，Content-Type: application/json；请求头 X-Gridlake-Event（事件名）、X-Gridlake-Delivery（投递 id，重发时不变）、X-Gridlake-Signature。请求体：') }}
          <code>{ id, event, occurredAt, actor: { id, name } | null, data: { … } }</code>
          {{ $t('。2xx 算成功；否则 1 分钟、5 分钟、30 分钟、2 小时、6 小时后重试，共 6 次。不保证顺序。') }}
        </div>
        <div class="relative">
          <pre class="snippet">{{ verifySnippet }}</pre>
          <el-button size="small" class="!absolute right-2 top-2" @click="copy(verifySnippet)">
            {{ $t('复制') }}
          </el-button>
        </div>
      </div>

      <div v-loading="loading" class="min-h-48">
        <div v-if="!hooks.length && !loading" class="empty">
          <div class="text-[15px] text-gray-800 font-medium">
            {{ $t('还没有 Webhook') }}
          </div>
          <div class="text-xs text-gray-500 leading-5">
            {{ $t('例如：数据连接每天同步完成后通知数据管道开始跑；有人提交填报后在工单系统里建一条待办') }}
          </div>
          <el-button type="primary" class="mt-2" @click="openEdit(null)">
            {{ $t('新建 Webhook') }}
          </el-button>
        </div>
        <div v-for="h in hooks" :key="h.id" class="card" :data-testid="`webhook-${h.name}`">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="text-gray-900 font-medium">{{ h.name }}</span>
              <el-tag v-if="!h.enabled" size="small" type="info" effect="plain">
                {{ $t('已停用') }}
              </el-tag>
              <el-tag v-else-if="h.lastStatus === 'failed'" size="small" type="danger" effect="plain">
                {{ $t('最近一次失败') }}
              </el-tag>
              <el-tag v-if="h.pending" size="small" type="warning" effect="plain">
                {{ $t('{0} 条等待重试', [h.pending]) }}
              </el-tag>
              <code class="min-w-0 truncate text-xs text-gray-500">{{ h.url }}</code>
            </div>
            <div class="mt-0.5 text-xs text-gray-500">
              {{ $t('事件：{0}', [eventsText(h)]) }}
            </div>
            <div class="mt-0.5 text-xs text-gray-400">
              {{ $t('创建于 {0}{1}', [h.createdAt.slice(0, 16), h.createdBy ? `（${h.createdBy}）` : '']) }}
              <template v-if="h.lastAt">
                · {{ $t('最近发送 {0}', [h.lastAt.slice(5, 16)]) }}
                <span v-if="h.lastStatus === 'failed'" class="text-[#B91C1C]">{{ h.lastError }}</span>
              </template>
            </div>
          </div>
          <el-switch :model-value="h.enabled" size="small" @update:model-value="toggle(h, !!$event)" />
          <el-button link size="small" :loading="testing === h.id" data-testid="webhook-test" @click="test(h)">
            {{ $t('测试') }}
          </el-button>
          <el-button link size="small" data-testid="webhook-deliveries" @click="openDeliveries(h)">
            {{ $t('发送记录') }}
          </el-button>
          <el-button link size="small" @click="openEdit(h)">
            {{ $t('修改') }}
          </el-button>
          <el-button link size="small" @click="rotate(h)">
            {{ $t('重置密钥') }}
          </el-button>
          <el-button link size="small" type="danger" @click="remove(h)">
            {{ $t('删除') }}
          </el-button>
        </div>
      </div>
    </template>

    <template v-else>
      <div class="mb-3 flex items-center gap-2 text-[13px]">
        <el-button size="small" @click="viewing = null; load()">
          {{ $t('← 返回') }}
        </el-button>
        <span class="text-gray-900 font-medium">{{ $t('「{0}」的发送记录', [viewing.name]) }}</span>
        <span class="flex-1 text-xs text-gray-400">{{ $t('最近 50 条；保留 7 天') }}</span>
        <el-button size="small" @click="openDeliveries(viewing)">
          {{ $t('刷新') }}
        </el-button>
      </div>
      <el-table v-loading="deliveriesLoading" :data="deliveries" size="small" max-height="520" :empty-text="$t('还没有发送记录')" data-testid="webhook-delivery-table">
        <el-table-column type="expand">
          <template #default="{ row }">
            <div class="grid grid-cols-2 gap-3 px-3">
              <div>
                <div class="mb-1 text-xs text-gray-500">
                  {{ $t('请求体') }}
                </div>
                <pre class="snippet">{{ pretty((row as WebhookDelivery).payload) }}</pre>
              </div>
              <div>
                <div class="mb-1 text-xs text-gray-500">
                  {{ $t('对方响应') }}
                </div>
                <pre class="snippet">{{ (row as WebhookDelivery).response ?? $t('（无）') }}</pre>
              </div>
            </div>
          </template>
        </el-table-column>
        <el-table-column :label="$t('时间')" prop="createdAt" width="170" />
        <el-table-column :label="$t('事件')" width="170">
          <template #default="{ row }">
            {{ label((row as WebhookDelivery).event) }}
          </template>
        </el-table-column>
        <el-table-column :label="$t('结果')" width="90">
          <template #default="{ row }">
            <span :class="STATUS[(row as WebhookDelivery).status].cls">{{ STATUS[(row as WebhookDelivery).status].text }}</span>
          </template>
        </el-table-column>
        <el-table-column :label="$t('次数')" prop="attempts" width="56" />
        <el-table-column :label="$t('说明')" min-width="260" show-overflow-tooltip>
          <template #default="{ row }">
            <span v-if="(row as WebhookDelivery).error" class="text-[#B91C1C]">{{ (row as WebhookDelivery).error }}</span>
            <span v-else-if="(row as WebhookDelivery).httpStatus" class="text-gray-500">HTTP {{ (row as WebhookDelivery).httpStatus }}</span>
            <span v-if="(row as WebhookDelivery).nextAt" class="ml-2 text-gray-400">{{ $t('下次重试 {0}', [(row as WebhookDelivery).nextAt!.slice(5, 16)]) }}</span>
          </template>
        </el-table-column>
        <el-table-column width="80">
          <template #default="{ row }">
            <el-button v-if="(row as WebhookDelivery).status !== 'pending' && viewing.enabled" link size="small" @click="redeliver(row as WebhookDelivery)">
              {{ $t('重发') }}
            </el-button>
          </template>
        </el-table-column>
      </el-table>
    </template>

    <el-dialog v-model="editVisible" :title="editing ? $t('修改 Webhook · {0}', [editing.name]) : $t('新建 Webhook')" width="620px" append-to-body :close-on-click-modal="false">
      <el-form label-width="88px" @submit.prevent>
        <el-form-item :label="$t('名称')" required>
          <el-input v-model="form.name" maxlength="50" :placeholder="$t('如「数据管道」「工单系统」')" data-testid="webhook-name" />
        </el-form-item>
        <el-form-item :label="$t('接收地址')" :required="!editing">
          <el-input v-model="form.url" :placeholder="editing ? $t('不填表示不变（{0}）', [editing.url]) : 'https://example.com/gridlake/webhook'" data-testid="webhook-url" />
        </el-form-item>
        <el-form-item :label="$t('事件')" required>
          <div class="w-full">
            <el-checkbox v-model="form.all">
              {{ $t('全部事件（包括以后新增的）') }}
            </el-checkbox>
            <el-checkbox-group v-if="!form.all" v-model="form.events" class="events">
              <el-checkbox v-for="e in events" :key="e.name" :value="e.name" :data-testid="`webhook-event-${e.name}`">
                <span>{{ label(e.name) }}</span>
                <code class="ml-1 text-[11px] text-gray-400">{{ e.name }}</code>
              </el-checkbox>
            </el-checkbox-group>
          </div>
        </el-form-item>
        <el-form-item :label="$t('启用')">
          <el-switch v-model="form.enabled" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="editVisible = false">
          {{ $t('取消') }}
        </el-button>
        <el-button type="primary" :loading="saving" data-testid="webhook-save" @click="save">
          {{ editing ? $t('保存') : $t('创建') }}
        </el-button>
      </template>
    </el-dialog>
  </el-dialog>
</template>

<style scoped lang="scss">
.card {
  @apply mb-2 flex items-center gap-2 rounded-md border border-[#e5e7eb] border-solid px-3 py-2.5 text-[13px];
}

.empty {
  @apply flex flex-col items-center gap-2 rounded-md border border-dashed border-[#c8c8c8] px-6 py-10 text-center;
}

.secret {
  @apply mb-3 rounded-md border border-[#F59E0B] border-solid bg-[#FFF7E6] px-3 py-2 text-[13px] text-[#92400E];

  code {
    @apply rounded bg-white px-2 py-1 text-xs text-gray-800 font-mono;
  }
}

.guide {
  @apply mb-3 rounded-md bg-[#f9f9f7] px-3 py-2;

  code {
    @apply rounded bg-white px-1 text-xs font-mono;
  }
}

.events {
  @apply grid grid-cols-2 gap-x-3;
}

.snippet {
  @apply m-0 max-h-72 overflow-auto whitespace-pre rounded-md bg-[#f9f9f7] p-3 pr-16 text-xs leading-5 font-mono;
}
</style>
