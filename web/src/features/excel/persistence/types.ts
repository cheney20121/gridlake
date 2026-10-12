/**
 * 数据持久化 — 类型定义
 */

/** 工作簿快照 */
export interface WorkbookSnapshot {
  id: string
  workbookId: string
  version: number
  timestamp: number
  author: string
  description: string
  data: string
  isAuto: boolean
}

/** 持久化的工作簿元数据 */
export interface PersistedWorkbook {
  id: string
  name: string
  data: string
  lastModified: number
  createdAt: number
  author: string
}
