import type { DatasetColumnKind, ImportPreview } from '@/api/dataset'
import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/** 数据连接器：按计划把文件夹 / 对象存储 / 数据库 / HTTP 接口的数据自动同步进数据集（设计见 docs/connectors-design.md） */
const url = (path: string) => `${LEDGER_BASE}/api/ledger/connectors${path}`

export interface FolderSource {
  kind: 'folder'
  /** 目录（绝对路径，或相对于第一个收件目录） */
  path: string
  /** 文件名通配符，多个用 ; 分隔 */
  pattern: string
}

/** ClickHouse 走它的 HTTP 接口（默认端口 8123；ssl 为 require 时用 HTTPS） */
export type DbEngine = 'mysql' | 'postgres' | 'clickhouse'

export interface DatabaseSource {
  kind: 'database'
  engine: DbEngine
  host: string
  port: number
  database: string
  user: string
  /** 只在提交时填写；留空表示沿用已保存的密码 */
  password?: string
  ssl: 'disable' | 'prefer' | 'require'
  sql: string
  incrementalColumn?: string | null
}

export type HttpAuth
  = | { kind: 'none' }
    | { kind: 'bearer' }
    | { kind: 'basic', user: string }
    | { kind: 'header', name: string }
    /** 抖店开放平台自用型应用：密钥填 App Secret，签名与令牌由服务端处理 */
    | { kind: 'doudian', appKey: string, shopId: string }

export type HttpPagination
  = | { kind: 'none' }
    | { kind: 'page', start: number, size: number }
    | { kind: 'offset', size: number }
    | { kind: 'cursor', cursorPath: string }

/** HTTP 接口：地址 / 请求体可写 {{page}} {{size}} {{offset}} {{cursor}} {{watermark}} {{now}}（当前 Unix 秒）占位符 */
export interface HttpSource {
  kind: 'http'
  url: string
  method: 'get' | 'post'
  headers: { name: string, value: string }[]
  body?: string | null
  auth: HttpAuth
  /** 只在提交时填写；留空表示沿用已保存的密钥 */
  secret?: string
  /** 记录数组在响应中的路径，如 data.list；为空时响应本身是数组 */
  recordsPath: string
  pagination: HttpPagination
  incrementalField?: string | null
  initialWatermark?: string | null
}

/** 对象存储（S3 及兼容 S3 协议的存储） */
export interface S3Source {
  kind: 's3'
  /** 自建 / 其他云的服务地址；留空是 AWS S3 */
  endpoint?: string | null
  region: string
  bucket: string
  prefix: string
  pattern: string
  recursive: boolean
  /** 路径风格；不传时：填了 endpoint 用路径风格，AWS 用虚拟主机风格 */
  pathStyle?: boolean | null
  accessKey: string
  /** 只在提交时填写；留空表示沿用已保存的密钥 */
  secretKey?: string
}

export type ConnectorSource = FolderSource | DatabaseSource | HttpSource | S3Source

export type SyncMode = 'replace' | 'append' | 'upsert'

export interface ConnectorColumn {
  source: string
  title: string
  kind: DatasetColumnKind
  include: boolean
}

export interface ConnectorTarget {
  datasetId?: string | null
  name?: string | null
  mode: SyncMode
  primaryKey: string[]
  columns: ConnectorColumn[]
}

export type ConnectorSchedule
  = | { kind: 'manual' }
    | { kind: 'interval', minutes: number }
    | { kind: 'daily', at: string }

export interface ConnectorRun {
  id: string
  connectorId: string
  trigger: 'manual' | 'schedule'
  status: 'running' | 'success' | 'failed' | 'skipped'
  startedAt: string
  finishedAt: string | null
  rowsRead: number
  inserted: number
  updated: number
  skipped: number
  replaced: number
  files: string[]
  watermarkFrom: string | null
  watermarkTo: string | null
  message: string | null
}

export interface Connector {
  id: string
  name: string
  kind: 'folder' | 'database' | 'http' | 's3'
  source: ConnectorSource
  hasPassword: boolean
  target: ConnectorTarget
  datasetName: string | null
  schedule: ConnectorSchedule
  enabled: boolean
  watermark: string | null
  createdAt: string
  updatedAt: string
  lastRun: ConnectorRun | null
  nextRun: string | null
  running: boolean
}

export interface ConnectorInput {
  name: string
  source: ConnectorSource
  target: ConnectorTarget
  schedule: ConnectorSchedule
  enabled: boolean
}

export interface ConnectorFile {
  path: string
  name: string
  size: number
  mtime: string
  imported: boolean
  settling: boolean
}

export interface ConnectorTestResult {
  message: string
  database?: { serverVersion: string, columns: string[] }
  files?: ConnectorFile[]
  /** HTTP 接口：第一页记录数、第一条记录的字段、响应开头 */
  http?: { records: number, fields: string[], sample: string }
  inboxRoots: string[]
}

export type ConnectorPreview = ImportPreview & { sampleOf: string }

export function listConnectorsApi() {
  return request<{ items: Connector[], inboxRoots: string[] }>({ url: url(''), method: 'get' })
}

export function createConnectorApi(data: ConnectorInput) {
  return request<string>({ url: url(''), method: 'post', data })
}

export function updateConnectorApi(id: string, data: ConnectorInput) {
  return request<boolean>({ url: url(`/${id}`), method: 'put', data })
}

export function deleteConnectorApi(id: string) {
  return request<boolean>({ url: url(`/${id}`), method: 'delete' })
}

/** 测试来源；编辑已有连接器时带 connectorId，未填写密码则用已保存的 */
export function testConnectorApi(source: ConnectorSource, connectorId?: string) {
  return request<ConnectorTestResult>({ url: url('/test'), method: 'post', data: { source, connectorId }, timeout: 60 * 1000 })
}

export function previewConnectorApi(source: ConnectorSource, connectorId?: string) {
  return request<ConnectorPreview>({ url: url('/preview'), method: 'post', data: { source, connectorId }, timeout: 5 * 60 * 1000 })
}

/** 立即同步（后台执行），返回同步记录 ID */
export function runConnectorApi(id: string) {
  return request<string>({ url: url(`/${id}/run`), method: 'post' })
}

export function connectorRunsApi(id: string, limit = 50) {
  return request<ConnectorRun[]>({ url: url(`/${id}/runs`), method: 'get', params: { limit } })
}
