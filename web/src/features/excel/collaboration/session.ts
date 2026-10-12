import type { FUniver } from '@univerjs/core/facade'
import type { Op } from './ops'
import type { CollabEvent, CollabOpsMessage, CollabPeerInfo } from '@/api/collab'
import { shallowRef } from 'vue'
import { CollabRefused, connectCollab, sendCollab } from '@/api/collab'
import { t as tr } from '@/i18n'
import { cellKeys, retarget, shouldRelay, withoutPending } from './ops'

/**
 * 实时协作会话：打开服务端工作簿后加入它的协作房间，把自己的修改（Univer 的 mutation）成批发给服务端，
 * 按服务端的顺序应用别人的修改，显示谁在线、光标在哪。服务端的约定见 server/src/collab.rs。
 *
 * - 自己的修改立刻在本地生效，同时发出；服务端把每一批按全局序号回显给所有人（包括自己）。同一个格子被同时修改时以服务端顺序为准：
 *   收到别人的修改时，跳过我已经发出、还没收到回显的格子（那些格子最终是我的值）
 * - 别人的修改应用时不记为我的修改（不会让我的工作簿显示「未保存」）：谁改的谁保存，保存后通知所有人新的版本号
 * - 加入时若工作簿版本与房间的基准版本不同，先重新载入再加入；之后按日志补齐还没保存的修改
 * - 断线重连后如果错过的修改还在日志里就补齐，否则提示「与服务器失去同步」，由用户重新载入
 */

export type CollabStatus = 'off' | 'connecting' | 'online' | 'reconnecting' | 'waiting' | 'diverged'

export interface Peer extends CollabPeerInfo {
  cursor?: { sheetId: string, row: number, col: number }
  lastActive: number
}

/** 会话需要页面提供的能力 */
export interface CollabHooks {
  getVersion: () => number
  /** 别人保存了新版本：把自己的基准版本更新到它 */
  adoptVersion: (version: number) => void
  /** 重新载入服务端最新版本 */
  reload: () => Promise<void>
  /** 应用别人的修改期间不记为用户修改 */
  track: <T>(fn: () => Promise<T> | T) => Promise<T>
  /** 程序重建工作簿期间（打开、恢复版本等）的变更不转发 */
  isSuspended: () => boolean
  /** 服务端要求保存一次（日志太大 / 有人在等） */
  save: () => Promise<unknown>
  toast: (message: string, level: 'info' | 'warning' | 'success') => void
  /** 没开登录校验时的显示名 */
  displayName: () => string
}

const FLUSH_MS = 40
const MAX_BATCH_BYTES = 900 * 1024
const PRESENCE_MS = 150

function newClientId() {
  return `c${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`
}

export class CollabSession {
  readonly status = shallowRef<CollabStatus>('off')
  readonly peers = shallowRef<Peer[]>([])
  readonly self = shallowRef<CollabPeerInfo | null>(null)
  /** 有人做了很大的修改没有实时同步：需要重新载入才能看到 */
  readonly staleBy = shallowRef<string | null>(null)

  private api: FUniver | null = null
  private hooks: CollabHooks | null = null
  private wb: string | null = null
  private clientId = newClientId()
  private ctrl: AbortController | null = null
  private generation = 0
  private joined = false
  private applied = 0
  private waitSave = false
  private attempts = 0
  private chain: Promise<unknown> = Promise.resolve()
  private eventOff: { dispose: () => void } | null = null

  private buffer: Op[] = []
  private flushTimer: ReturnType<typeof setTimeout> | null = null
  private sending = false
  private pending = new Map<string, string[]>()
  private waiters: (() => void)[] = []

  private presenceTimer: ReturnType<typeof setTimeout> | null = null
  private lastPresence: { sheetId: string, row: number, col: number } | null = null

  attach(api: FUniver, hooks: CollabHooks) {
    this.api = api
    this.hooks = hooks
    this.eventOff?.dispose()
    // 本地用户操作产生的变更：成批发给服务端
    this.eventOff = api.addEvent(api.Event.CommandExecuted, (e) => {
      if (!this.wb || this.status.value === 'off' || this.self.value?.canEdit === false || this.hooks?.isSuspended())
        return
      if (!shouldRelay(e.id, e.options as { onlyLocal?: boolean, fromFormula?: boolean, fromCollab?: boolean } | undefined))
        return
      this.buffer.push({ id: e.id, params: JSON.parse(JSON.stringify(e.params ?? {})) as Record<string, unknown> })
      this.scheduleFlush()
    })
  }

  /** 加入某个工作簿的协作（已在这个工作簿里则不重复） */
  start(workbookId: string) {
    if (this.wb === workbookId && this.status.value !== 'off')
      return
    this.stop()
    this.wb = workbookId
    this.clientId = newClientId()
    this.joined = false
    this.applied = 0
    this.attempts = 0
    this.status.value = 'connecting'
    this.connect()
  }

  stop() {
    this.generation++
    this.ctrl?.abort()
    this.ctrl = null
    this.wb = null
    this.joined = false
    this.waitSave = false
    this.buffer = []
    this.pending.clear()
    this.waiters.splice(0).forEach(w => w())
    this.peers.value = []
    this.self.value = null
    this.staleBy.value = null
    this.status.value = 'off'
    if (this.flushTimer)
      clearTimeout(this.flushTimer)
    this.flushTimer = null
  }

  /** 保存快照已包含的最后一个序号（保存时带给服务端） */
  appliedSeq() {
    return this.applied
  }

  get online() {
    return this.status.value === 'online'
  }

  // ── 连接 ──

  private async connect() {
    const gen = this.generation
    const wb = this.wb
    const hooks = this.hooks
    if (!wb || !hooks)
      return
    const ctrl = new AbortController()
    this.ctrl = ctrl
    try {
      await connectCollab(wb, this.clientId, hooks.getVersion(), hooks.displayName(), e => this.enqueue(gen, e), ctrl.signal)
    }
    catch (e) {
      if (gen !== this.generation || ctrl.signal.aborted)
        return
      if (e instanceof CollabRefused) {
        // 没有权限 / 服务端不支持：不再重试
        this.stop()
        return
      }
    }
    if (gen !== this.generation || ctrl.signal.aborted)
      return
    // 断线：稍后重连
    this.status.value = this.joined ? 'reconnecting' : 'connecting'
    setTimeout(() => gen === this.generation && this.connect(), Math.min(10_000, 1000 * 2 ** Math.min(this.attempts++, 4)))
  }

  /** 重新加入（重新载入工作簿之后） */
  private restart() {
    const wb = this.wb
    if (!wb)
      return
    this.generation++
    this.ctrl?.abort()
    this.joined = false
    this.applied = 0
    this.buffer = []
    this.pending.clear()
    this.waiters.splice(0).forEach(w => w())
    this.status.value = 'connecting'
    this.connect()
  }

  private enqueue(gen: number, e: CollabEvent) {
    // 事件按到达顺序逐个处理（应用变更是异步的）
    this.chain = this.chain.then(() => (gen === this.generation ? this.handle(e) : undefined)).catch(() => {})
  }

  // ── 事件 ──

  private async handle(e: CollabEvent) {
    const hooks = this.hooks!
    switch (e.type) {
      case 'hello': {
        this.self.value = e.self
        this.peers.value = e.peers.map(p => ({ ...p, lastActive: Date.now() }))
        if (!this.joined) {
          if (e.needSave) {
            // 日志太大没法补齐：等在线的人保存一次，保存完成时重新加入
            this.waitSave = true
            this.status.value = 'waiting'
            return
          }
          if (e.baseVersion !== hooks.getVersion()) {
            if (++this.attempts > 3) {
              this.status.value = 'diverged'
              return
            }
            await hooks.reload()
            this.restart()
            return
          }
          this.attempts = 0
          for (const entry of e.log)
            await this.applyEntry(entry)
          this.applied = e.seq
          this.joined = true
          this.status.value = 'online'
          this.flush()
          return
        }
        // 断线重连：错过的修改还在日志里就补齐，否则无法判断状态是否一致
        const first = e.log[0]?.seq
        const contiguous = e.seq === this.applied || (first !== undefined && first <= this.applied + 1)
        if (!contiguous) {
          this.status.value = 'diverged'
          return
        }
        for (const entry of e.log.filter(x => x.seq > this.applied))
          await this.applyEntry(entry)
        this.applied = Math.max(this.applied, e.seq)
        hooks.adoptVersion(e.baseVersion)
        this.status.value = 'online'
        this.flush()
        return
      }
      case 'join':
        this.peers.value = [...this.peers.value.filter(p => p.id !== e.peer.id), { ...e.peer, lastActive: Date.now() }]
        return
      case 'leave':
        this.peers.value = this.peers.value.filter(p => p.id !== e.id)
        return
      case 'presence':
        this.peers.value = this.peers.value.map(p => (p.id === e.from && e.presence.sheetId != null ? { ...p, cursor: { sheetId: e.presence.sheetId, row: e.presence.row ?? 0, col: e.presence.col ?? 0 }, lastActive: Date.now() } : p))
        return
      case 'ops':
        await this.applyEntry(e)
        return
      case 'saved':
        if (this.waitSave) {
          this.waitSave = false
          this.restart()
          return
        }
        hooks.adoptVersion(e.version)
        return
      case 'replaced':
        hooks.toast(`${e.by || '有人'} 用别的方式更新了这个工作簿（如恢复历史版本），已重新载入`, 'warning')
        await hooks.reload()
        this.restart()
        return
      case 'request-save':
        hooks.save().catch(() => {})
        return
      case 'stale':
        this.staleBy.value = this.peers.value.find(p => p.id === e.from)?.name ?? '有人'
    }
  }

  /** 一批变更：自己的是回显（确认收到），别人的应用到本地 */
  private async applyEntry(m: CollabOpsMessage) {
    this.applied = Math.max(this.applied, m.seq)
    if (m.from === this.clientId) {
      this.pending.delete(m.batchId)
      this.notifyWaiters()
      return
    }
    const api = this.api
    const unit = api?.getActiveWorkbook()?.getId()
    if (!api || !unit)
      return
    // 我的修改还没收到回显的格子：已经发出的，以及还在等待发出的（先收到别人的修改时，后者同样会排在别人之后）
    const pendingCells = new Set([...this.pending.values()].flat().concat(this.buffer.flatMap(cellKeys)))
    for (const raw of m.ops) {
      const op = withoutPending(retarget(raw, unit), pendingCells)
      if (!op)
        continue
      try {
        await this.hooks!.track(() => api.executeCommand(op.id, op.params, { onlyLocal: true, fromCollab: true } as never))
      }
      catch {
        // 单条失败（如工作表已被删除）不影响后面的；状态是否还一致由用户感知，必要时「重新同步」
      }
    }
  }

  // ── 发送 ──

  private scheduleFlush() {
    if (!this.flushTimer)
      this.flushTimer = setTimeout(() => this.flush(), FLUSH_MS)
  }

  private async flush() {
    this.flushTimer = null
    if (this.sending || !this.wb || !this.buffer.length || !this.joined)
      return
    const wb = this.wb
    const gen = this.generation
    const ops = this.buffer
    this.buffer = []
    const batchId = `${this.clientId}-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 5)}`
    this.sending = true
    try {
      const body = { client: this.clientId, kind: 'ops' as const, batchId, ops }
      if (JSON.stringify(body).length > MAX_BATCH_BYTES) {
        // 太大发不出去：让别人知道需要重新载入（保存之后）
        await sendCollab(wb, { client: this.clientId, kind: 'stale' }).catch(() => {})
        this.hooks?.toast(tr('这次修改太大，没有实时同步给其他人；保存后他们可以点「重新同步」看到'), 'warning')
        return
      }
      this.pending.set(batchId, ops.flatMap(cellKeys))
      await sendCollab(wb, body)
    }
    catch {
      if (gen !== this.generation)
        return
      // 发送失败：放回去，稍后重试
      this.pending.delete(batchId)
      this.buffer = [...ops, ...this.buffer]
      setTimeout(() => gen === this.generation && this.flush(), 2000)
    }
    finally {
      this.sending = false
      if (this.buffer.length && gen === this.generation)
        this.scheduleFlush()
    }
  }

  private notifyWaiters() {
    if (!this.pending.size && !this.buffer.length)
      this.waiters.splice(0).forEach(w => w())
  }

  /** 等自己的修改都发出并收到回显（保存之前调用，保证快照包含的序号是准确的）；最多等 timeoutMs */
  async settle(timeoutMs = 4000) {
    if (!this.wb || this.status.value === 'off')
      return
    if (this.buffer.length)
      this.flush()
    if (!this.pending.size && !this.buffer.length)
      return
    await new Promise<void>((resolve) => {
      const t = setTimeout(resolve, timeoutMs)
      this.waiters.push(() => {
        clearTimeout(t)
        resolve()
      })
    })
  }

  /** 把自己的光标位置告诉别人（节流） */
  broadcastCursor(sheetId: string, row: number, col: number) {
    this.lastPresence = { sheetId, row, col }
    if (this.presenceTimer || !this.wb || !this.joined)
      return
    this.presenceTimer = setTimeout(() => {
      this.presenceTimer = null
      const p = this.lastPresence
      if (p && this.wb)
        sendCollab(this.wb, { client: this.clientId, kind: 'presence', presence: p }).catch(() => {})
    }, PRESENCE_MS)
  }

  /** 与服务器重新同步：重新载入最新保存的版本（本地没保存的修改会丢，调用方先确认） */
  async resync() {
    await this.hooks?.reload()
    this.staleBy.value = null
    this.restart()
  }
}

export const collabSession = new CollabSession()
