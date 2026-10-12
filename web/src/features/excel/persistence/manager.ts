/**
 * 持久化管理器 — 工作簿保存到服务端（gridlake）
 *
 * - 有修改才保存：监听表格变更（sheet.mutation.*），停止编辑 3 秒后保存，连续编辑时最多 30 秒保存一次
 * - 乐观并发：保存时带「基于的版本号」，期间别人保存过则返回冲突，交给界面让用户选择
 * - 断网兜底：保存失败时把内容存为本地草稿（IndexedDB），下次打开该工作簿时提示恢复
 * - 不再限制行数（原本地保存超过 5 万行就跳过）；明细级大数据请导入为数据集
 */

import type { FUniver } from '@univerjs/core/facade'
import type { SaveWorkbookResult, WorkbookMeta } from '@/api/workbook'
import { isAxiosError } from 'axios'
import { saveWorkbookApi } from '@/api/workbook'
import { deleteWorkbook, getWorkbook, saveWorkbook } from './storage'

const IDLE_DELAY = 3_000
const MAX_DELAY = 30_000
const DRAFT_PREFIX = 'draft:'

/** 不算「用户修改」的变更：筛选标记、空操作、权限点 */
const IGNORED_MUTATIONS = new Set([
  'sheet.mutation.mark-dirty-filter-change',
  'sheet.mutation.empty',
  'sheet.mutation.set-worksheet-permission-points',
])

export type SaveState = 'idle' | 'dirty' | 'saving' | 'saved' | 'error' | 'conflict' | 'offline'

export interface SaveOutcome {
  state: 'saved' | 'conflict' | 'error' | 'unchanged' | 'detached'
  conflict?: WorkbookMeta | null
  error?: unknown
}

export interface LocalDraft {
  workbookId: string
  baseVersion: number
  json: string
  savedAt: number
}

type Listener = (state: SaveState, meta: WorkbookMeta | null) => void

export class PersistenceManager {
  private api: FUniver | null = null
  private meta: WorkbookMeta | null = null
  private dirty = false
  private saving: Promise<SaveOutcome> | null = null
  private idleTimer: ReturnType<typeof setTimeout> | null = null
  private firstDirtyAt = 0
  private suspended = 0
  private autoSave = true
  private state: SaveState = 'idle'
  private listeners = new Set<Listener>()
  private eventDisposer: { dispose: () => void } | null = null
  private operator: () => string = () => '当前用户'
  private conflict: WorkbookMeta | null = null

  /** 初始化：绑定 Univer，开始监听变更 */
  init(api: FUniver, operator?: () => string): void {
    this.api = api
    if (operator)
      this.operator = operator
    this.eventDisposer?.dispose()
    this.eventDisposer = api.addEvent(api.Event.CommandExecuted, (event) => {
      const id = String(event.id)
      // 协作中别人的修改（fromCollab）不是我的修改：谁改的谁保存
      if (this.suspended > 0 || !id.startsWith('sheet.mutation.') || IGNORED_MUTATIONS.has(id) || (event.options as { fromCollab?: boolean } | undefined)?.fromCollab)
        return
      this.markDirty()
    })
  }

  dispose(): void {
    this.eventDisposer?.dispose()
    this.eventDisposer = null
    this.clearTimers()
  }

  /** 当前绑定的服务端工作簿 */
  getMeta(): WorkbookMeta | null {
    return this.meta
  }

  getState(): SaveState {
    return this.state
  }

  isDirty(): boolean {
    return this.dirty
  }

  /** 最近一次保存冲突时服务端的版本信息 */
  getConflict(): WorkbookMeta | null {
    return this.conflict
  }

  onChange(listener: Listener): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  private setState(state: SaveState) {
    this.state = state
    this.listeners.forEach(l => l(state, this.meta))
  }

  /** 绑定（或切换到）某个服务端工作簿；之后的保存基于 meta.version */
  attach(meta: WorkbookMeta): void {
    this.meta = meta
    this.conflict = null
    this.dirty = false
    this.clearTimers()
    this.autoSave = true
    this.setState('saved')
  }

  detach(): void {
    this.meta = null
    this.dirty = false
    this.clearTimers()
    this.setState('idle')
  }

  /** 程序在重建工作簿（打开、恢复版本等）：这期间的变更不是用户修改 */
  isSuspended(): boolean {
    return this.suspended > 0
  }

  /** 别人（协作中）保存了新版本：把基准版本更新到它，之后自己的保存才不会冲突 */
  adoptVersion(version: number): void {
    if (this.meta && version > this.meta.version)
      this.meta = { ...this.meta, version }
  }

  private collab: { before: () => Promise<void>, seq: () => number } | null = null

  /** 协作：保存前等自己的修改都同步出去，并带上快照已包含的序号 */
  setCollab(hook: { before: () => Promise<void>, seq: () => number } | null): void {
    this.collab = hook
  }

  /** 程序重建工作簿（打开、恢复版本、重建汇总表）期间不记为用户修改 */
  async withoutTracking<T>(fn: () => Promise<T> | T): Promise<T> {
    this.suspended++
    try {
      return await fn()
    }
    finally {
      // 变更事件可能在同一帧稍后派发，延后解除
      setTimeout(() => this.suspended--, 0)
    }
  }

  markDirty(): void {
    if (!this.meta)
      return
    if (!this.dirty)
      this.firstDirtyAt = Date.now()
    this.dirty = true
    if (this.state !== 'conflict' && this.state !== 'saving')
      this.setState('dirty')
    if (!this.autoSave)
      return
    if (this.idleTimer)
      clearTimeout(this.idleTimer)
    const wait = Math.max(0, Math.min(IDLE_DELAY, this.firstDirtyAt + MAX_DELAY - Date.now()))
    this.idleTimer = setTimeout(() => {
      this.idleTimer = null
      this.save({ auto: true }).catch(console.error)
    }, wait)
  }

  private clearTimers() {
    if (this.idleTimer)
      clearTimeout(this.idleTimer)
    this.idleTimer = null
  }

  /** 当前工作簿 JSON */
  serialize(): string | null {
    const wb = this.api?.getActiveWorkbook()
    return wb ? JSON.stringify(wb.save()) : null
  }

  /**
   * 保存到服务端。auto：自动保存（无修改时跳过）；force：冲突时仍覆盖（对方内容保留在版本历史里）
   */
  async save(opts: { auto?: boolean, force?: boolean, description?: string } = {}): Promise<SaveOutcome> {
    if (!this.meta)
      return { state: 'detached' }
    if (opts.auto && (!this.dirty || this.state === 'conflict'))
      return { state: 'unchanged' }
    // 已有保存在进行：等它结束再判断是否还需要保存
    if (this.saving) {
      await this.saving
      if (opts.auto && !this.dirty)
        return { state: 'unchanged' }
    }
    this.saving = this.doSave(opts)
    try {
      return await this.saving
    }
    finally {
      this.saving = null
    }
  }

  private async doSave(opts: { auto?: boolean, force?: boolean, description?: string }): Promise<SaveOutcome> {
    const meta = this.meta!
    // 协作中：先等自己发出的修改收到回显，序列化与读序号之间不能有 await
    await this.collab?.before().catch(() => {})
    const json = this.serialize()
    const collabSeq = this.collab?.seq()
    if (!json)
      return { state: 'detached' }
    this.clearTimers()
    // 保存期间的新修改要能再次触发保存
    const wasDirty = this.dirty
    this.dirty = false
    this.setState('saving')
    let result: SaveWorkbookResult
    try {
      result = await saveWorkbookApi(meta.id, json, {
        baseVersion: meta.version,
        force: opts.force,
        auto: opts.auto,
        description: opts.description,
        operator: this.operator(),
      }, collabSeq)
    }
    catch (error) {
      this.dirty = this.dirty || wasDirty
      // 网络不通 / 服务不可用（代理返回 502、503、504 等 5xx）：存本地草稿；
      // 服务端明确拒绝（4xx，如内容过大、工作簿已删除）：不存草稿，直接报错
      const status = isAxiosError(error) ? error.response?.status : undefined
      const offline = isAxiosError(error) && (status === undefined || status >= 500)
      if (offline)
        await this.saveDraft({ workbookId: meta.id, baseVersion: meta.version, json, savedAt: Date.now() }).catch(() => {})
      this.setState(offline ? 'offline' : 'error')
      return { state: 'error', error }
    }
    if (!result.saved) {
      this.dirty = true
      this.autoSave = false
      this.conflict = result.conflict
      this.setState('conflict')
      return { state: 'conflict', conflict: result.conflict }
    }
    this.meta = result.meta ?? { ...meta, version: result.version }
    this.conflict = null
    await this.clearDraft(meta.id).catch(() => {})
    this.autoSave = true
    if (this.dirty) {
      this.setState('dirty')
      this.markDirty()
    }
    else {
      this.setState('saved')
    }
    return { state: 'saved' }
  }

  // ── 本地草稿（断网兜底）──────────────────────────────
  private async saveDraft(draft: LocalDraft) {
    await saveWorkbook({
      id: DRAFT_PREFIX + draft.workbookId,
      name: String(draft.baseVersion),
      data: draft.json,
      lastModified: draft.savedAt,
      createdAt: draft.savedAt,
      author: this.operator(),
    })
  }

  async getDraft(workbookId: string): Promise<LocalDraft | null> {
    const d = await getWorkbook(DRAFT_PREFIX + workbookId)
    return d ? { workbookId, baseVersion: Number(d.name), json: d.data, savedAt: d.lastModified } : null
  }

  async clearDraft(workbookId: string) {
    await deleteWorkbook(DRAFT_PREFIX + workbookId)
  }
}

/** 单例 */
export const persistenceManager = new PersistenceManager()
