<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { AiAction, AiContext, AiEvent, AiItem, AiStatus, AiThread } from '@/api/ai'
import type { UndoStep } from '@/features/excel/ai/workbook'
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { aiAttachStream, aiChatStream, aiCreateThreadApi, aiDecideApi, aiDeleteThreadApi, aiStatusApi, aiStopApi, aiThreadApi, aiThreadsApi } from '@/api/ai'
import { renderMarkdown } from '@/features/excel/ai/markdown'
import { applyAction, collectContext, undoApplied } from '@/features/excel/ai/workbook'
import { useExcelUIStore } from '@/stores/excel-ui'
import AiActionCard from './AiActionCard.vue'
import AiSummaryBlock from './AiSummaryBlock.vue'
import AiThreadBar from './AiThreadBar.vue'
import { EXAMPLES, TOOL_LABEL } from './constants'

/**
 * AI 分析助手面板：按工作簿保存会话；提问时附带当前工作表与选区；
 * 回答中的透视表 / 图表 / 公式 / 计算列以建议卡片出现，点「应用」才创建（记录谁在何时应用）。
 * 回答在服务端后台运行：关掉面板或页面后继续，回来时自动接上；较早的对话由服务端整理成摘要
 */
const props = defineProps<{
  univerApi: FUniver | null
  /** 当前工作簿（服务端 ID）；未保存时为空 */
  workbookId: string | null
  operator?: string
  /** 打开时选中的对话（会话链接 ?ai=） */
  initialThreadId?: string | null
}>()

const emit = defineEmits<{
  /** 工作簿有修改（建议已应用） */
  (e: 'changed'): void
  /** 数据集结构有变化（加了计算列） */
  (e: 'datasetsChanged'): void
}>()

const status = ref<AiStatus | null>(null)
const threads = ref<AiThread[]>([])
const threadId = ref<string | null>(null)
const items = ref<AiItem[]>([])
const actions = ref<Record<string, AiAction>>({})
const input = ref('')
const sending = ref(false)
const withContext = ref(true)
const applying = ref<string | null>(null)
const listRef = ref<HTMLElement>()
/** 当前连接（提问或重新连上）；中断它只是不再接收，回答在服务端继续 */
let controller: AbortController | null = null
/** 进行中的状态说明（如「正在整理较早的对话…」） */
const statusText = ref('')
/** 较早轮次的摘要；summarizedUntil 之后显示分隔线 */
const summary = ref<string | null>(null)
const summarizedUntil = ref<string | null>(null)
const stopping = ref(false)
/** 上一次回答被服务重启打断了 */
const interrupted = ref(false)
let initialPending = true

const contextPreview = ref<AiContext>({})
function refreshContext() {
  contextPreview.value = collectContext(props.univerApi)
}

const contextText = computed(() => {
  const c = contextPreview.value
  const parts: string[] = []
  if (c.sheetName)
    parts.push(`工作表「${c.sheetName}」`)
  if (c.pivot)
    parts.push('透视表定义')
  if (c.selection && c.selectionValues)
    parts.push(`选区 ${c.selection}`)
  return parts.join(' · ')
})

/** 断开当前连接（回答在服务端继续，切回这个对话时会接上） */
function detach() {
  controller?.abort()
  controller = null
  sending.value = false
  stopping.value = false
  statusText.value = ''
}

async function scrollDown() {
  await nextTick()
  listRef.value?.scrollTo({ top: listRef.value.scrollHeight })
}

async function loadThread(id: string, attach = true) {
  const d = await aiThreadApi(id)
  if (threadId.value !== id)
    return
  items.value = d.items
  actions.value = Object.fromEntries(d.actions.map(a => [a.id, a]))
  summary.value = d.summary
  summarizedUntil.value = d.summarizedUntil
  interrupted.value = d.interrupted
  const t = threads.value.find(x => x.id === id)
  if (t)
    t.running = d.running
  scrollDown()
  // 关掉面板 / 页面期间还在回答：接上继续显示
  if (attach && d.running && !sending.value)
    attachRun(id)
}

/** 链接里的对话不在列表里：服务端会说明原因（别人的对话只有本人和管理员能看、对话已删除），能读到但不在列表里的是别的工作簿的 */
async function explainMissingLink(id: string) {
  const other = await aiThreadApi(id).then(d => d.thread.workbookId !== props.workbookId, () => false)
  if (other)
    ElMessage.warning('链接里的对话不属于这个工作簿')
}

async function loadThreads() {
  if (!props.workbookId) {
    threads.value = []
    threadId.value = null
    items.value = []
    return
  }
  threads.value = await aiThreadsApi(props.workbookId)
  // 链接指定的对话只在第一次加载时选中
  const linked = initialPending ? props.initialThreadId : null
  initialPending = false
  const wanted = linked && threads.value.find(t => t.id === linked)
  if (linked && !wanted)
    await explainMissingLink(linked)
  threadId.value = (wanted || threads.value[0])?.id ?? null
  if (threadId.value)
    await loadThread(threadId.value)
  else
    items.value = []
}

// 其他功能（异常发现）请求分析：把问题预填到输入框，由用户确认后发送
const uiStore = useExcelUIStore()
watch(() => uiStore.pendingAiQuestion, (q) => {
  if (q) {
    input.value = q
    uiStore.pendingAiQuestion = null
  }
}, { immediate: true })

watch(() => props.workbookId, async () => {
  detach()
  status.value = await aiStatusApi().catch(() => null)
  await loadThreads()
}, { immediate: true })

async function selectThread(id: string) {
  detach()
  threadId.value = id
  await loadThread(id)
}

async function newThread() {
  if (!props.workbookId)
    return
  // 当前对话还没内容时直接复用
  if (threadId.value && !items.value.length)
    return
  detach()
  const t = await aiCreateThreadApi(props.workbookId, props.operator)
  threads.value.unshift(t)
  threadId.value = t.id
  items.value = []
  actions.value = {}
  summary.value = null
  summarizedUntil.value = null
}

async function removeThread() {
  const t = threads.value.find(x => x.id === threadId.value)
  if (!t)
    return
  const ok = await ElMessageBox.confirm(`删除对话「${t.title}」？已应用到工作簿的透视表、图表不受影响。`, '删除对话', { type: 'warning', confirmButtonText: '删除' }).then(() => true, () => false)
  if (!ok)
    return
  await aiDeleteThreadApi(t.id)
  await loadThreads()
}

function lastAssistant(): Extract<AiItem, { role: 'assistant' }> {
  const last = items.value[items.value.length - 1]
  if (last?.role === 'assistant')
    return last
  const item: AiItem = { role: 'assistant', id: `live_${Date.now()}`, text: '', createdAt: '' }
  items.value.push(item)
  return items.value[items.value.length - 1] as Extract<AiItem, { role: 'assistant' }>
}

/** 处理回答过程中的一个事件（提问、重新连上共用） */
function handleEvent(tid: string, e: AiEvent, onError: () => void) {
  if (threadId.value !== tid)
    return
  switch (e.type) {
    case 'start': {
      // 重新连上时会从本次回答的开头补发事件：去掉已显示的本次回答部分，按补发的事件重新显示
      const i = items.value.findIndex(x => x.role === 'user' && x.id === e.userMessageId)
      if (i >= 0)
        items.value = items.value.slice(0, i + 1)
      break
    }
    case 'status':
      statusText.value = e.text
      break
    case 'summary':
      summary.value = e.summary
      break
    case 'text':
      statusText.value = ''
      lastAssistant().text += e.delta
      break
    case 'break':
      items.value.push({ role: 'assistant', id: `live_b_${Date.now()}`, text: '', createdAt: '' })
      break
    case 'tool': {
      statusText.value = ''
      const existing = items.value.find(x => x.role === 'tool' && x.id === e.id)
      if (existing && existing.role === 'tool') {
        existing.ok = e.status !== 'error'
        existing.summary = e.summary ?? existing.summary
        existing.actionId = e.actionId ?? null
        ;(existing as { running?: boolean }).running = false
      }
      else {
        items.value = items.value.filter(x => !(x.role === 'assistant' && !x.text.trim()))
        items.value.push({ role: 'tool', id: e.id, name: e.name, summary: '', ok: true, actionId: null, createdAt: '', running: e.status === 'running' } as AiItem)
      }
      break
    }
    case 'action':
      actions.value[e.action.id] = e.action
      break
    case 'error':
      onError()
      ElMessage.error({ message: e.message, duration: 6000 })
      break
  }
  scrollDown()
}

/** 连接结束后：以服务端保存的内容为准（含错误说明、摘要分隔线） */
async function afterRun(tid: string) {
  sending.value = false
  stopping.value = false
  statusText.value = ''
  controller = null
  const t = threads.value.find(x => x.id === tid)
  if (t)
    t.running = false
  if (threadId.value === tid)
    await loadThread(tid, false)
}

async function send(text?: string) {
  const message = (text ?? input.value).trim()
  if (!message || sending.value || !props.workbookId)
    return
  if (!threadId.value) {
    const t = await aiCreateThreadApi(props.workbookId, props.operator)
    threads.value.unshift(t)
    threadId.value = t.id
  }
  const tid = threadId.value!
  const context = withContext.value ? collectContext(props.univerApi) : {}
  input.value = ''
  sending.value = true
  items.value.push({ role: 'user', id: `live_u_${Date.now()}`, text: message, createdAt: '' })
  scrollDown()
  const ctrl = new AbortController()
  controller = ctrl
  let failed = false
  try {
    await aiChatStream(tid, { message, context }, e => handleEvent(tid, e, () => failed = true), ctrl.signal)
  }
  catch (e) {
    if (!ctrl.signal.aborted) {
      failed = true
      ElMessage.error(e instanceof Error ? e.message : String(e))
    }
  }
  // 连接被切走（换对话 / 关面板）：回答还在服务端继续，不刷新
  if (ctrl.signal.aborted)
    return
  await afterRun(tid)
  const t = threads.value.find(x => x.id === tid)
  if (t && t.title === '新对话')
    t.title = message.slice(0, 30)
  if (failed)
    input.value = input.value || message
}

function onSummarySaved(text: string | null) {
  if (text === null) {
    // 清除后分隔线也没有了，重新载入
    if (threadId.value)
      loadThread(threadId.value, false)
    return
  }
  summary.value = text
}

/** 接着回答被服务重启打断的上一个问题：不新增提问，从已有的对话继续 */
async function resume() {
  const tid = threadId.value
  if (!tid || sending.value)
    return
  interrupted.value = false
  sending.value = true
  const ctrl = new AbortController()
  controller = ctrl
  let failed = false
  try {
    // start 事件用于重新连上时去掉已显示的部分；这里已有的内容要保留，忽略它
    await aiChatStream(tid, { resume: true, context: {} }, e => e.type !== 'start' && handleEvent(tid, e, () => failed = true), ctrl.signal)
  }
  catch (e) {
    if (!ctrl.signal.aborted) {
      failed = true
      ElMessage.error(e instanceof Error ? e.message : String(e))
    }
  }
  if (ctrl.signal.aborted)
    return
  await afterRun(tid)
  if (failed)
    interrupted.value = true
}

/** 接上正在进行的回答 */
async function attachRun(tid: string) {
  const ctrl = new AbortController()
  controller = ctrl
  sending.value = true
  try {
    await aiAttachStream(tid, e => handleEvent(tid, e, () => {}), ctrl.signal)
  }
  catch (e) {
    if (!ctrl.signal.aborted)
      ElMessage.error(e instanceof Error ? e.message : String(e))
  }
  if (!ctrl.signal.aborted)
    await afterRun(tid)
}

/** 停止：通知服务端（正在调用模型时立即中断，已产生的文字保留） */
async function stop() {
  if (!threadId.value)
    return
  stopping.value = true
  await aiStopApi(threadId.value).catch(() => {})
}

/** 复制会话链接：打开后进入这个工作簿并选中这个对话 */
async function copyLink() {
  if (!threadId.value || !props.workbookId)
    return
  const u = new URL(window.location.href)
  u.search = ''
  u.searchParams.set('wb', props.workbookId)
  u.searchParams.set('ai', threadId.value)
  await navigator.clipboard.writeText(u.toString())
  ElMessage.success('已复制对话链接')
}

function onKeydown(e: Event | KeyboardEvent) {
  if (e instanceof KeyboardEvent && e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault()
    send()
  }
}

async function apply(a: AiAction) {
  const api = props.univerApi
  if (!api)
    return
  applying.value = a.id
  try {
    const { message, undo: undoInfo } = await applyAction(api, a, () => emit('datasetsChanged'), props.operator)
    await aiDecideApi(a.id, 'applied', message, props.operator, undoInfo)
    actions.value[a.id] = { ...a, status: 'applied', result: message, undo: undoInfo, decidedBy: props.operator ?? null, decidedAt: new Date().toLocaleString() }
    emit('changed')
    ElMessage.success(message)
  }
  catch (e) {
    const msg = e instanceof Error ? e.message : String(e)
    if (msg !== '已取消')
      ElMessage.error(`应用失败：${msg}`)
  }
  finally {
    applying.value = null
  }
}

async function dismiss(a: AiAction) {
  await aiDecideApi(a.id, 'dismissed', undefined, props.operator)
  actions.value[a.id] = { ...a, status: 'dismissed' }
}

/** 撤销已应用的建议 / 方案（删除新建的页签、图表、计算列，恢复被覆盖的单元格） */
async function undo(a: AiAction) {
  const api = props.univerApi
  if (!api || !a.undo)
    return
  applying.value = a.id
  try {
    const note = await undoApplied(api, a.undo as UndoStep, () => emit('datasetsChanged'), props.operator)
    await aiDecideApi(a.id, 'undone', note, props.operator)
    actions.value[a.id] = { ...a, status: 'undone', result: note }
    emit('changed')
    ElMessage.success(note || '已撤销')
  }
  catch (e) {
    ElMessage.error(`撤销失败：${e instanceof Error ? e.message : String(e)}`)
  }
  finally {
    applying.value = null
  }
}

// 关掉面板只断开连接，回答在服务端继续
onBeforeUnmount(() => controller?.abort())
</script>

<template>
  <div class="ai flex flex-col">
    <!-- 未配置 -->
    <div v-if="status && !status.configured" class="m-3 rounded-md bg-[#FFF7E6] p-3 text-xs text-[#92400E] leading-5">
      服务端尚未配置 AI 模型。管理员在服务配置中设置
      <code>LEDGER_AI_BASE_URL</code>（OpenAI 兼容接口地址，可为内网的 DeepSeek / Qwen）、<code>LEDGER_AI_MODEL</code>，需要时设置 <code>LEDGER_AI_API_KEY</code>，重启服务后即可使用。详见部署手册。
    </div>
    <div v-else-if="!workbookId" class="m-3 rounded-md bg-[#f9f9f7] p-3 text-xs text-gray-500">
      工作簿保存到服务端后即可使用 AI 助手（对话按工作簿保存）。
    </div>

    <template v-else>
      <AiThreadBar :threads="threads" :thread-id="threadId" :sending="sending" @select="selectThread" @new="newThread" @link="copyLink" @delete="removeThread" />

      <div ref="listRef" class="min-h-0 flex-1 overflow-y-auto px-3 py-2" @wheel.stop>
        <div v-if="!items.length" class="py-4 text-[13px] text-gray-600">
          <div class="mb-1 font-medium">
            用中文提问，我会查询数据集后回答
          </div>
          <div class="mb-3 text-xs text-gray-500 leading-5">
            数字都来自服务端查询；需要保留的结果会生成透视表、图表、公式建议，确认后才写入工作簿。{{ status?.model ? `模型：${status.model}` : '' }}
          </div>
          <div v-for="q in EXAMPLES" :key="q" class="example" @click="send(q)">
            {{ q }}
          </div>
        </div>

        <template v-for="it in items" :key="it.id">
          <div v-if="it.role === 'user'" class="mb-2 flex justify-end">
            <div class="user">
              {{ it.text }}
            </div>
          </div>
          <!-- eslint-disable-next-line vue/no-v-html 已先转义全部 HTML，仅生成有限标签 -->
          <div v-else-if="it.role === 'assistant' && it.text.trim()" class="answer" v-html="renderMarkdown(it.text)" />
          <div v-else-if="it.role === 'tool'" class="mb-1">
            <div class="step" :class="{ bad: !it.ok }">
              <span class="w-3 shrink-0 text-center">{{ (it as { running?: boolean }).running ? '…' : it.ok ? '✓' : '✗' }}</span>
              <span class="shrink-0 text-gray-500">{{ TOOL_LABEL[it.name] ?? it.name }}</span>
              <span class="min-w-0 truncate" :title="it.summary">{{ it.summary }}</span>
            </div>
            <AiActionCard
              v-if="it.actionId && actions[it.actionId]"
              :action="actions[it.actionId]!"
              :applying="applying === it.actionId"
              @apply="apply(actions[it.actionId]!)"
              @dismiss="dismiss(actions[it.actionId]!)"
              @undo="undo(actions[it.actionId]!)"
            />
          </div>
          <AiSummaryBlock v-if="summary && it.id === summarizedUntil && threadId" :thread-id="threadId" :summary="summary" :busy="sending" @saved="onSummarySaved" />
        </template>
        <div v-if="sending && statusText" class="step">
          <span class="w-3 text-center">…</span><span class="text-gray-500">{{ statusText }}</span>
        </div>
        <div v-else-if="sending && items[items.length - 1]?.role === 'user'" class="step">
          <span class="w-3 text-center">…</span><span class="text-gray-500">思考中</span>
        </div>
        <div v-if="sending" class="mt-1 text-[11px] text-gray-400">
          回答在服务端进行，关掉面板或页面也会继续，回来后自动接上
        </div>
      </div>

      <div v-if="interrupted && !sending" class="interrupted">
        <span>上一次回答被中断了（服务重启），已有的提问和查询结果都保留着。</span>
        <el-button size="small" type="primary" @click="resume">
          继续回答
        </el-button>
      </div>

      <div class="border-t border-t-[#eef0f3] border-t-solid p-2">
        <div class="mb-1 flex items-center gap-2 text-xs text-gray-500">
          <el-checkbox v-model="withContext" size="small" @change="refreshContext">
            附带当前界面
          </el-checkbox>
          <span v-if="withContext" class="min-w-0 truncate" :title="contextText">{{ contextText }}</span>
        </div>
        <div class="flex items-end gap-2">
          <el-input
            v-model="input"
            type="textarea"
            :autosize="{ minRows: 2, maxRows: 6 }"
            resize="none"
            placeholder="问问数据，如：9 月退款为什么变多了？（Enter 发送，Shift+Enter 换行）"
            :disabled="status?.configured === false"
            @keydown="onKeydown"
            @focus="refreshContext"
            @keydown.stop
          />
          <el-button v-if="sending" type="danger" plain :loading="stopping" @click="stop">
            停止
          </el-button>
          <el-button v-else type="primary" :disabled="!input.trim()" @click="send()">
            发送
          </el-button>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped lang="scss">
.ai {
  height: 100%;
  font-size: 13px;
  color: #0b0b0b;
}

.example {
  @apply mb-1.5 cursor-pointer rounded-md border border-[#e5e7eb] border-solid px-2.5 py-1.5 text-xs text-gray-700;

  &:hover {
    @apply border-[#217346] text-[#217346];
  }
}

.user {
  @apply max-w-[85%] whitespace-pre-wrap rounded-lg bg-[#E7F4EC] px-2.5 py-1.5 leading-5;
}

.answer {
  @apply mb-2 leading-6;

  :deep(p) {
    @apply my-1;
  }

  :deep(.h) {
    @apply mb-1 mt-2 font-semibold;
  }

  :deep(ul),
  :deep(ol) {
    @apply my-1 pl-5;
  }

  :deep(code) {
    @apply rounded bg-[#f4f4f1] px-1 text-xs;
  }

  :deep(table) {
    @apply my-1.5 w-full border-collapse text-xs;
  }

  :deep(th) {
    @apply bg-[#f9f9f7] px-1.5 py-1 text-left font-normal text-gray-500;
  }

  :deep(td) {
    @apply border-b border-b-[#f0efec] border-b-solid px-1.5 py-1;
  }

  :deep(td.num) {
    @apply text-right tabular-nums;
  }
}

.interrupted {
  @apply flex items-center justify-between gap-2 border-t border-t-[#fde68a] border-t-solid bg-[#fffbeb] px-3 py-1.5 text-xs text-[#92400e];
}

.step {
  @apply flex items-center gap-1.5 py-0.5 text-xs text-gray-600;

  &.bad {
    @apply text-[#B91C1C];
  }
}
</style>
