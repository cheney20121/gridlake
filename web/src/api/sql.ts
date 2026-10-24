import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/** SQL 查询：对数据集写只读 SQL，结果可另存为数据集并按计划重算 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger/sql${path}`

export type SqlColumnKind = 'text' | 'id' | 'integer' | 'number' | 'money' | 'datetime' | 'boolean'

export interface SqlColumn {
  name: string
  kind: SqlColumnKind
  sqlType: string
}

export interface SqlResult {
  columns: SqlColumn[]
  rows: (string | number | boolean | null)[][]
  truncated: boolean
  elapsedMs: number
  datasets: string[]
}

export type SqlSchedule
  = | { kind: 'manual' }
    | { kind: 'daily', at: string }
    | { kind: 'after_sync', connectorId: string }
    /** 某个数据集有新数据后（导入、数据连接同步、外部文件有变化）；SQL 查询写入的数据集不会再触发别的查询 */
    | { kind: 'after_data', datasetId: string }

export type ParamKind = 'text' | 'number' | 'date'

/** 参数化查询：SQL 里的 {{名称}}，运行时按类型安全代入 */
export interface ParamDef {
  name: string
  kind: ParamKind
  default?: string | null
  /** 文本参数的可选值（为空不限制） */
  choices?: string[]
}

export interface SavedQuery {
  id: string
  name: string
  sql: string
  description: string
  datasetId: string | null
  datasetName: string | null
  columnKinds: Record<string, SqlColumnKind>
  schedule: SqlSchedule
  createdBy: string | null
  createdAt: string
  updatedAt: string
  lastRunAt: string | null
  lastStatus: 'success' | 'failed' | null
  lastRows: number | null
  lastMessage: string | null
  nextRun: string | null
  running: boolean
  params: ParamDef[]
  values: Record<string, string>
  /** 实例：由哪个参数化查询生成的 */
  templateId: string | null
}

export interface QueryInput {
  name: string
  sql: string
  description?: string
  /** 另存为数据集：新数据集名称（已关联数据集时忽略） */
  saveAsDataset?: string | null
  columnKinds?: Record<string, SqlColumnKind>
  schedule: SqlSchedule
  operator?: string
  params?: ParamDef[]
  values?: Record<string, string>
}

export interface Materialized {
  datasetId: string
  datasetName: string
  rows: number
  rebuilt: boolean
  elapsedMs: number
}

export function runSqlApi(sql: string, signal?: AbortSignal, params?: ParamDef[], values?: Record<string, string>) {
  return request<SqlResult>({ url: url('/run'), method: 'post', data: { sql, limit: 1000, params, values }, signal, timeout: 5 * 60 * 1000, silent: true })
}

/** 用一组参数值从参数化查询生成实例（独立的查询 + 新数据集） */
export function instantiateQueryApi(id: string, values: Record<string, string>, name?: string) {
  return request<SavedQuery>({ url: url(`/queries/${id}/instances`), method: 'post', data: { values, name: name || undefined }, timeout: 10 * 60 * 1000 })
}

export function listQueriesApi() {
  return request<SavedQuery[]>({ url: url('/queries'), method: 'get' })
}

export function createQueryApi(data: QueryInput) {
  return request<SavedQuery>({ url: url('/queries'), method: 'post', data, timeout: 10 * 60 * 1000 })
}

export function updateQueryApi(id: string, data: QueryInput) {
  return request<SavedQuery>({ url: url(`/queries/${id}`), method: 'put', data, timeout: 10 * 60 * 1000 })
}

export function deleteQueryApi(id: string) {
  return request<boolean>({ url: url(`/queries/${id}`), method: 'delete' })
}

export function refreshQueryApi(id: string, datasetName?: string) {
  return request<Materialized>({ url: url(`/queries/${id}/refresh`), method: 'post', data: datasetName ? { datasetName } : {}, timeout: 10 * 60 * 1000 })
}
