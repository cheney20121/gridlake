import type { Connector, ConnectorSchedule, ConnectorSource, DbEngine, HttpAuth, HttpPagination, SyncMode } from '@/api/connector'
import type { DatasetColumnKind } from '@/api/dataset'
import { t as tr } from '@/i18n'

/** 新建 / 修改连接器对话框的表单状态与转换（表单 ↔ 接口入参） */

export type SourceKind = 'folder' | DbEngine | 'http'
export type AuthKind = 'none' | 'bearer' | 'basic' | 'header' | 'doudian'
export type PageKind = 'none' | 'page' | 'offset' | 'cursor'
export type Step = 0 | 1 | 2

export interface FolderForm { path: string, pattern: string }
export interface DbForm { host: string, port: number, database: string, user: string, password: string, ssl: 'disable' | 'prefer' | 'require', sql: string, incrementalColumn: string }
export interface HttpForm {
  url: string
  method: 'get' | 'post'
  headers: { name: string, value: string }[]
  body: string
  authKind: AuthKind
  authUser: string
  authHeader: string
  appKey: string
  shopId: string
  secret: string
  recordsPath: string
  pageKind: PageKind
  pageStart: number
  pageSize: number
  cursorPath: string
  incrementalField: string
  initialWatermark: string
}
export interface FormColumn { source: string, title: string, kind: DatasetColumnKind, include: boolean, samples: string[] }

export interface ConnectorForm {
  name: string
  sourceKind: SourceKind
  folder: FolderForm
  db: DbForm
  http: HttpForm
  targetType: 'new' | 'existing'
  datasetName: string
  datasetId: string
  mode: SyncMode
  primaryKey: string[]
  columns: FormColumn[]
  schedule: ConnectorSchedule['kind']
  minutes: number
  at: string
  enabled: boolean
  runNow: boolean
}

export const KIND_OPTIONS: { value: DatasetColumnKind, label: string }[] = [
  { value: 'text', label: tr('文本') },
  { value: 'id', label: tr('编号') },
  { value: 'money', label: tr('金额') },
  { value: 'number', label: tr('数值') },
  { value: 'datetime', label: tr('日期时间') },
]

export const MODE_OPTIONS: { value: SyncMode, label: string, hint: string }[] = [
  { value: 'replace', label: tr('全量替换'), hint: tr('每次用最新数据整体替换（商品成本、档案类）；文件夹来源只取最新的文件') },
  { value: 'append', label: tr('增量追加'), hint: tr('只加入新数据，按主键跳过已有的行（流水、日报类）') },
  { value: 'upsert', label: tr('增量更新'), hint: tr('按主键新增或覆盖已有的行（订单状态、退款等会变化的数据）') },
]

/** 占位符（模板里不能直接写双花括号） */
export const PH = { page: '{{page}}', size: '{{size}}', offset: '{{offset}}', cursor: '{{cursor}}', watermark: '{{watermark}}', now: '{{now}}' }
export const URL_EXAMPLE = 'https://erp.example.com/api/orders?page={{page}}&size={{size}}&updated_after={{watermark}}'
export const BODY_EXAMPLE = '{"page": {{page}}, "pageSize": {{size}}, "updatedAfter": "{{watermark}}"}'

/** 切换数据库类型时带出的默认端口 */
export const DEFAULT_PORT: Partial<Record<SourceKind, number>> = { mysql: 3306, postgres: 5432, clickhouse: 8123 }

export function defaultHttpForm(): HttpForm {
  return { url: '', method: 'get', headers: [], body: '', authKind: 'none', authUser: '', authHeader: 'X-API-Key', appKey: '', shopId: '', secret: '', recordsPath: '', pageKind: 'none', pageStart: 1, pageSize: 100, cursorPath: '', incrementalField: '', initialWatermark: '' }
}

export function defaultForm(): ConnectorForm {
  return {
    name: '',
    sourceKind: 'folder',
    folder: { path: '', pattern: '*.csv' },
    db: { host: '', port: 3306, database: '', user: '', password: '', ssl: 'prefer', sql: '', incrementalColumn: '' },
    http: defaultHttpForm(),
    targetType: 'new',
    datasetName: '',
    datasetId: '',
    mode: 'append',
    primaryKey: [],
    columns: [],
    schedule: 'daily',
    minutes: 30,
    at: '06:00',
    enabled: true,
    runNow: true,
  }
}

/** 已有连接器 → 表单（密码 / 密钥不回填，留空表示沿用已保存的） */
export function formFromConnector(c: Connector): ConnectorForm {
  const form = defaultForm()
  form.name = c.name
  if (c.source.kind === 'folder') {
    form.sourceKind = 'folder'
    form.folder = { path: c.source.path, pattern: c.source.pattern }
  }
  else if (c.source.kind === 'database') {
    const s = c.source
    form.sourceKind = s.engine
    form.db = { host: s.host, port: s.port, database: s.database, user: s.user, password: '', ssl: s.ssl, sql: s.sql, incrementalColumn: s.incrementalColumn ?? '' }
  }
  else {
    const s = c.source
    form.sourceKind = 'http'
    form.http = {
      url: s.url,
      method: s.method,
      headers: s.headers.map(h => ({ ...h })),
      body: s.body ?? '',
      authKind: s.auth.kind,
      authUser: s.auth.kind === 'basic' ? s.auth.user : '',
      authHeader: s.auth.kind === 'header' ? s.auth.name : 'X-API-Key',
      appKey: s.auth.kind === 'doudian' ? s.auth.appKey : '',
      shopId: s.auth.kind === 'doudian' ? s.auth.shopId : '',
      secret: '',
      recordsPath: s.recordsPath,
      pageKind: s.pagination.kind,
      pageStart: s.pagination.kind === 'page' ? s.pagination.start : 1,
      pageSize: s.pagination.kind === 'page' || s.pagination.kind === 'offset' ? s.pagination.size : 100,
      cursorPath: s.pagination.kind === 'cursor' ? s.pagination.cursorPath : '',
      incrementalField: s.incrementalField ?? '',
      initialWatermark: s.initialWatermark ?? '',
    }
  }
  form.targetType = c.target.datasetId ? 'existing' : 'new'
  form.datasetId = c.target.datasetId ?? ''
  form.datasetName = c.target.name ?? ''
  form.mode = c.target.mode
  form.primaryKey = [...c.target.primaryKey]
  form.columns = c.target.columns.map(x => ({ ...x, samples: [] }))
  form.schedule = c.schedule.kind
  if (c.schedule.kind === 'interval')
    form.minutes = c.schedule.minutes
  if (c.schedule.kind === 'daily')
    form.at = c.schedule.at
  form.enabled = c.enabled
  form.runNow = false
  return form
}

function httpSource(h: HttpForm): ConnectorSource {
  const auth: HttpAuth = h.authKind === 'basic'
    ? { kind: 'basic', user: h.authUser.trim() }
    : h.authKind === 'header'
      ? { kind: 'header', name: h.authHeader.trim() }
      : h.authKind === 'doudian' ? { kind: 'doudian', appKey: h.appKey.trim(), shopId: h.shopId.trim() } : { kind: h.authKind }
  const pagination: HttpPagination = h.pageKind === 'page'
    ? { kind: 'page', start: Number(h.pageStart) || 0, size: Number(h.pageSize) || 100 }
    : h.pageKind === 'offset'
      ? { kind: 'offset', size: Number(h.pageSize) || 100 }
      : h.pageKind === 'cursor' ? { kind: 'cursor', cursorPath: h.cursorPath.trim() } : { kind: 'none' }
  return {
    kind: 'http',
    url: h.url.trim(),
    method: h.method,
    headers: h.headers.filter(x => x.name.trim()).map(x => ({ name: x.name.trim(), value: x.value })),
    body: h.method === 'post' && h.body.trim() ? h.body : null,
    auth,
    secret: h.authKind !== 'none' && h.secret ? h.secret : undefined,
    recordsPath: h.recordsPath.trim(),
    pagination,
    incrementalField: h.incrementalField || null,
    initialWatermark: h.initialWatermark.trim() || null,
  }
}

/** 表单 → 来源（接口入参） */
export function buildSource(form: ConnectorForm): ConnectorSource {
  if (form.sourceKind === 'folder')
    return { kind: 'folder', path: form.folder.path.trim(), pattern: form.folder.pattern.trim() }
  if (form.sourceKind === 'http')
    return httpSource(form.http)
  return {
    kind: 'database',
    engine: form.sourceKind,
    host: form.db.host.trim(),
    port: Number(form.db.port) || 0,
    database: form.db.database.trim(),
    user: form.db.user.trim(),
    password: form.db.password || undefined,
    ssl: form.db.ssl,
    sql: form.db.sql.trim(),
    incrementalColumn: form.db.incrementalColumn || null,
  }
}

/** 抖店「订单列表」示例：按更新时间增量，页码从 0 开始 */
export function applyDoudianOrders(h: HttpForm) {
  Object.assign(h, {
    method: 'post',
    url: 'https://openapi-fxg.jinritemai.com/order/searchList',
    body: `{"page": ${PH.page}, "size": ${PH.size}, "update_time_start": ${PH.watermark}, "update_time_end": ${PH.now}}`,
    recordsPath: 'data.shop_order_list',
    pageKind: 'page',
    pageStart: 0,
    pageSize: 100,
    incrementalField: 'update_time',
    initialWatermark: String(Math.floor(Date.now() / 1000) - 7 * 86400),
  } satisfies Partial<HttpForm>)
}

export function hostOf(u: string) {
  try {
    return new URL(u.trim()).hostname
  }
  catch {
    return 'HTTP'
  }
}

/** 地址 / 请求体里用到的占位符（提示分页、增量设置是否对得上） */
export function usedPlaceholders(h: HttpForm) {
  const t = `${h.url}${h.method === 'post' ? h.body : ''}`
  return ['{{page}}', '{{size}}', '{{offset}}', '{{cursor}}', '{{watermark}}'].filter(p => t.includes(p))
}
