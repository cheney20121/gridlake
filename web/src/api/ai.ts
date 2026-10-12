import type { DatasetFilter, DatasetSort } from '@/api/dataset'
import type { ColumnStyle } from '@/components/excel/big-grid/column-style'
import { LEDGER_BASE } from '@/api/ledger-base'
import { t } from '@/i18n'
import { getToken } from '@/utils/auth'
import request from '@/utils/request'

/**
 * AI 分析助手：会话按工作簿保存；提问以 SSE 流式返回。
 * 回答在服务端后台运行：连接断开不影响，回来后用 aiAttachStream 重新连上；停止要调用 aiStopApi
 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger/ai${path}`

export interface AiStatus {
  configured: boolean
  /** 模型 @ 主机 */
  model: string | null
}

export interface AiThread {
  id: string
  workbookId: string
  title: string
  createdBy: string | null
  createdAt: string
  updatedAt: string
  /** 正在后台回答 */
  running: boolean
}

export type AiItem
  = | { role: 'user', id: string, text: string, createdAt: string }
    | { role: 'assistant', id: string, text: string, createdAt: string }
    | { role: 'tool', id: string, name: string, summary: string, ok: boolean, actionId: string | null, createdAt: string }

export interface AiPreview {
  header: string[]
  rows: string[][]
  note: string
}

export type AiActionKind = 'pivot' | 'chart' | 'formula' | 'calc' | 'table_view' | 'column_format' | 'table_create' | 'dataset_save' | 'plan'

export interface AiAction {
  id: string
  threadId: string
  kind: AiActionKind
  title: string
  payload: Record<string, any>
  status: 'proposed' | 'applied' | 'dismissed' | 'undone'
  result: string | null
  /** 应用时记下的撤销信息（见 features/excel/ai/workbook.ts 的 UndoStep） */
  undo?: Record<string, any> | null
  createdAt: string
  decidedAt: string | null
  decidedBy: string | null
}

export interface AiThreadDetail {
  thread: AiThread
  items: AiItem[]
  actions: AiAction[]
  running: boolean
  /** 较早轮次整理成的摘要；summarizedUntil 为摘要覆盖到的最后一条（界面在它后面显示分隔线） */
  summary: string | null
  summarizedUntil: string | null
  /** 上一次回答没有结束就中断了（服务重启）：可以「继续回答」 */
  interrupted: boolean
}

/** 工作簿里一张数据表当前的视图（让助手知道有哪些表、现在筛了什么） */
export interface AiTableContext {
  name: string
  datasetName: string
  /** float 浮动表格 / cells 铺到单元格 */
  mode: 'float' | 'cells'
  fields: string[]
  filters: DatasetFilter[]
  sort: DatasetSort[]
  keyword?: string
  /** 列样式（数字格式、条件格式），列名 → 样式 */
  columnStyles?: Record<string, ColumnStyle>
}

export interface AiContext {
  sheetName?: string
  selection?: string
  selectionValues?: unknown[][]
  pivot?: unknown
  tables?: AiTableContext[]
}

export type AiEvent
  = | { type: 'start', userMessageId: string }
    | { type: 'status', text: string }
    | { type: 'summary', summary: string }
    | { type: 'idle' }
    | { type: 'text', delta: string }
    | { type: 'break' }
    | { type: 'tool', id: string, name: string, status: 'running' | 'done' | 'error', summary?: string, actionId?: string | null }
    | { type: 'action', action: AiAction }
    | { type: 'done' }
    | { type: 'error', message: string }

export function aiStatusApi() {
  return request<AiStatus>({ url: url('/status'), method: 'get' })
}

export function aiThreadsApi(workbookId: string) {
  return request<AiThread[]>({ url: url('/threads'), method: 'get', params: { workbookId } })
}

export function aiCreateThreadApi(workbookId: string, operator?: string) {
  return request<AiThread>({ url: url('/threads'), method: 'post', data: { workbookId, operator } })
}

export function aiThreadApi(id: string) {
  return request<AiThreadDetail>({ url: url(`/threads/${id}`), method: 'get' })
}

/** 手动编辑会话摘要；null = 清除（之后提问时较早的轮次重新整理） */
export function aiSetSummaryApi(id: string, summary: string | null) {
  return request<boolean>({ url: url(`/threads/${id}/summary`), method: 'put', data: { summary } })
}

export function aiDeleteThreadApi(id: string) {
  return request<boolean>({ url: url(`/threads/${id}`), method: 'delete' })
}

/** 停止正在进行的回答（已产生的文字保留） */
export function aiStopApi(id: string) {
  return request<boolean>({ url: url(`/threads/${id}/stop`), method: 'post' })
}

export function aiDecideApi(id: string, status: 'applied' | 'dismissed' | 'undone', result?: string, operator?: string, undo?: unknown) {
  return request<boolean>({ url: url(`/actions/${id}`), method: 'post', data: { status, result, operator, undo } })
}

/**
 * 读取 SSE 事件流：逐个回调事件，流结束后 resolve。
 * axios 不支持读取流，这里用 fetch；与 request.ts 一致按 { code, msg } 信封解析错误
 */
/** fetch 不经过 request.ts 的拦截器：自己带上登录令牌（网关 / 跨域部署）和防跨站伪造用的 x-requested-with（会话 cookie 登录） */
function authHeaders(): Record<string, string> {
  const token = getToken()
  return { 'x-requested-with': 'XMLHttpRequest', ...(token ? { 'x-user-token': token } : {}) }
}

async function readEvents(resp: Response, onEvent: (e: AiEvent) => void) {
  if (!resp.ok || !resp.body) {
    let msg = t('请求失败（{0}）', [resp.status])
    try {
      const j = await resp.json() as { msg?: string }
      if (j.msg)
        msg = j.msg
    }
    catch {}
    throw new Error(msg)
  }
  const reader = resp.body.getReader()
  const decoder = new TextDecoder()
  let buf = ''
  while (true) {
    const { value, done } = await reader.read()
    if (done)
      break
    buf += decoder.decode(value, { stream: true })
    let idx = buf.indexOf('\n\n')
    while (idx >= 0) {
      const block = buf.slice(0, idx)
      buf = buf.slice(idx + 2)
      const data = block.split('\n').filter(l => l.startsWith('data:')).map(l => l.slice(5).trim()).join('\n')
      if (data) {
        try {
          onEvent(JSON.parse(data) as AiEvent)
        }
        catch {}
      }
      idx = buf.indexOf('\n\n')
    }
  }
}

/** 提问（流式）；resume 为 true 时不带问题，接着回答被中断的上一个问题。中断连接（signal）只是不再接收，回答在服务端继续 */
export async function aiChatStream(threadId: string, body: { message?: string, context: AiContext, resume?: boolean }, onEvent: (e: AiEvent) => void, signal?: AbortSignal) {
  const resp = await fetch(url(`/threads/${threadId}/chat`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json;charset=UTF-8', 'Accept': 'text/event-stream', ...authHeaders() },
    body: JSON.stringify(body),
    signal,
  })
  await readEvents(resp, onEvent)
}

/** 重新连上正在进行的回答：先收到本次回答已产生的事件（从 start 开始），再接着收；没有在回答时只收到 idle */
export async function aiAttachStream(threadId: string, onEvent: (e: AiEvent) => void, signal?: AbortSignal) {
  const resp = await fetch(url(`/threads/${threadId}/events`), { headers: { Accept: 'text/event-stream', ...authHeaders() }, signal })
  await readEvents(resp, onEvent)
}
