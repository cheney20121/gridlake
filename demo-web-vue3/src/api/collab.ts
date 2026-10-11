import { LEDGER_BASE } from '@/api/ledger-base'
import { getToken } from '@/utils/auth'
import request from '@/utils/request'

/**
 * 实时协作：下行用 SSE（fetch 流式读取，带登录头），上行用 POST。服务端只中转，不解释变更的内容
 */
const url = (workbookId: string, path: string) => `${LEDGER_BASE}/api/ledger/collab/${workbookId}/${path}`

export interface CollabPeerInfo {
  id: string
  name: string
  color: string
  canEdit: boolean
}

export interface CollabOpsMessage {
  type: 'ops'
  from: string
  seq: number
  batchId: string
  ops: { id: string, params: Record<string, unknown> }[]
}

export type CollabEvent
  = | { type: 'hello', self: CollabPeerInfo, peers: CollabPeerInfo[], baseVersion: number, seq: number, log: CollabOpsMessage[], needSave: boolean }
    | { type: 'join', peer: CollabPeerInfo }
    | { type: 'leave', id: string }
    | { type: 'presence', from: string, presence: { sheetId?: string, row?: number, col?: number } }
    | CollabOpsMessage
    | { type: 'saved', version: number, by: string, seq: number }
    | { type: 'replaced', version: number, by: string }
    | { type: 'request-save' }
    | { type: 'stale', from: string }

/** 连接失败且不应重试（没有权限、客户端标识不合法等）时抛出 */
export class CollabRefused extends Error {}

/** 加入协作并持续读取事件；流结束（断线）时 resolve，被拒绝时抛 CollabRefused */
export async function connectCollab(workbookId: string, client: string, version: number, name: string, onEvent: (e: CollabEvent) => void, signal: AbortSignal) {
  const token = getToken()
  const q = new URLSearchParams({ client, version: String(version), name })
  const resp = await fetch(`${url(workbookId, 'events')}?${q}`, { headers: { Accept: 'text/event-stream', ...(token ? { 'x-user-token': token } : {}) }, signal })
  if (!resp.ok || !resp.body) {
    let msg = `连接失败（${resp.status}）`
    try {
      const j = await resp.json() as { msg?: string }
      if (j.msg)
        msg = j.msg
    }
    catch {}
    if (resp.status >= 400 && resp.status < 500)
      throw new CollabRefused(msg)
    throw new Error(msg)
  }
  const reader = resp.body.getReader()
  const decoder = new TextDecoder()
  let buf = ''
  while (true) {
    const { value, done } = await reader.read()
    if (done)
      return
    buf += decoder.decode(value, { stream: true })
    let idx = buf.indexOf('\n\n')
    while (idx >= 0) {
      const block = buf.slice(0, idx)
      buf = buf.slice(idx + 2)
      const data = block.split('\n').filter(l => l.startsWith('data:')).map(l => l.slice(5).trim()).join('\n')
      if (data) {
        try {
          onEvent(JSON.parse(data) as CollabEvent)
        }
        catch {}
      }
      idx = buf.indexOf('\n\n')
    }
  }
}

export function sendCollab(workbookId: string, body: { client: string, kind: 'ops' | 'presence' | 'stale', batchId?: string, ops?: unknown[], presence?: unknown }) {
  return request<{ seq: number }>({ url: url(workbookId, 'send'), method: 'post', data: body, silent: true })
}
