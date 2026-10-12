/**
 * 数据持久化层 — 基于 IndexedDB 的工作簿存储
 * 支持自动保存、快照管理、数据恢复
 */

import type { PersistedWorkbook, WorkbookSnapshot } from './types'

const DB_NAME = 'gridlake-db'
const DB_VERSION = 1
const WORKBOOKS_STORE = 'workbooks'
const SNAPSHOTS_STORE = 'snapshots'

let dbInstance: IDBDatabase | null = null

/** 打开 IndexedDB 连接 */
function openDB(): Promise<IDBDatabase> {
  if (dbInstance)
    return Promise.resolve(dbInstance)

  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION)

    request.onupgradeneeded = () => {
      const db = request.result

      if (!db.objectStoreNames.contains(WORKBOOKS_STORE)) {
        db.createObjectStore(WORKBOOKS_STORE, { keyPath: 'id' })
      }

      if (!db.objectStoreNames.contains(SNAPSHOTS_STORE)) {
        db.createObjectStore(SNAPSHOTS_STORE, { keyPath: 'id' })
      }
    }

    request.onsuccess = () => {
      dbInstance = request.result
      resolve(dbInstance)
    }

    request.onerror = () => {
      reject(request.error)
    }
  })
}

/** 生成唯一 ID */
export function generateId(): string {
  return `wb_${Date.now()}_${Math.random().toString(36).slice(2, 9)}`
}

// ─── 工作簿持久化 ───────────────────────────────────────

/** 保存或更新工作簿 */
export async function saveWorkbook(workbook: PersistedWorkbook): Promise<void> {
  const db = await openDB()
  return new Promise((resolve, reject) => {
    const tx = db.transaction(WORKBOOKS_STORE, 'readwrite')
    const store = tx.objectStore(WORKBOOKS_STORE)
    store.put(workbook)
    tx.oncomplete = () => resolve()
    tx.onerror = () => reject(tx.error)
  })
}

/** 获取工作簿 by id */
export async function getWorkbook(id: string): Promise<PersistedWorkbook | null> {
  const db = await openDB()
  return new Promise((resolve, reject) => {
    const tx = db.transaction(WORKBOOKS_STORE, 'readonly')
    const store = tx.objectStore(WORKBOOKS_STORE)
    const request = store.get(id)
    request.onsuccess = () => resolve(request.result ?? null)
    request.onerror = () => reject(request.error)
  })
}

/** 获取所有工作簿列表 */
export async function listWorkbooks(): Promise<PersistedWorkbook[]> {
  const db = await openDB()
  return new Promise((resolve, reject) => {
    const tx = db.transaction(WORKBOOKS_STORE, 'readonly')
    const store = tx.objectStore(WORKBOOKS_STORE)
    const request = store.getAll()
    request.onsuccess = () => resolve(request.result ?? [])
    request.onerror = () => reject(request.error)
  })
}

/** 删除工作簿 */
export async function deleteWorkbook(id: string): Promise<void> {
  const db = await openDB()
  return new Promise((resolve, reject) => {
    const tx = db.transaction(WORKBOOKS_STORE, 'readwrite')
    const store = tx.objectStore(WORKBOOKS_STORE)
    store.delete(id)
    tx.oncomplete = () => resolve()
    tx.onerror = () => reject(tx.error)
  })
}

// ─── 快照管理 ───────────────────────────────────────────

/** 保存快照 */
export async function saveSnapshot(snapshot: WorkbookSnapshot): Promise<void> {
  const db = await openDB()
  return new Promise((resolve, reject) => {
    const tx = db.transaction(SNAPSHOTS_STORE, 'readwrite')
    const store = tx.objectStore(SNAPSHOTS_STORE)
    store.put(snapshot)
    tx.oncomplete = () => resolve()
    tx.onerror = () => reject(tx.error)
  })
}

/** 获取工作簿的所有快照 */
export async function listSnapshots(workbookId: string): Promise<WorkbookSnapshot[]> {
  const db = await openDB()
  return new Promise((resolve, reject) => {
    const tx = db.transaction(SNAPSHOTS_STORE, 'readonly')
    const store = tx.objectStore(SNAPSHOTS_STORE)
    const request = store.getAll()
    request.onsuccess = () => {
      const all: WorkbookSnapshot[] = request.result ?? []
      resolve(all.filter(s => s.workbookId === workbookId).sort((a, b) => b.timestamp - a.timestamp))
    }
    request.onerror = () => reject(request.error)
  })
}

/** 删除快照 */
export async function deleteSnapshot(id: string): Promise<void> {
  const db = await openDB()
  return new Promise((resolve, reject) => {
    const tx = db.transaction(SNAPSHOTS_STORE, 'readwrite')
    const store = tx.objectStore(SNAPSHOTS_STORE)
    store.delete(id)
    tx.oncomplete = () => resolve()
    tx.onerror = () => reject(tx.error)
  })
}
