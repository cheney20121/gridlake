import type { DatasetColumnKind } from '@/api/dataset'
import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/**
 * 多人填报：收集任务。一个任务绑定一个填报数据集，数据集里用「填报单位」列和「期次」列区分每个人每一期填的行；
 * 单位和负责人会同步成数据权限（负责人只能看 / 填自己负责的单位）
 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger${path}`

export interface CollectUnitInput {
  unit: string
  /** 负责人（登录过系统的用户 id）；不指派就只能由管理员代填 */
  userId?: string | null
}

/** 新建填报表：单位列和期次列自动加在最前面，主键 = 单位 + 期次 + 业务主键 */
export interface CollectNewTable {
  name: string
  columns: { title: string, kind: DatasetColumnKind }[]
  /** 同一单位同一期里区分不同行的列；不写表示每个单位每期只填一行 */
  keyColumns: string[]
}

export interface CollectInput {
  name: string
  /** 用已有的数据集（主键必须包含单位列和期次列）… */
  datasetId?: string
  /** …或者新建一张 */
  newTable?: CollectNewTable
  unitColumn?: string
  periodColumn?: string
  dimension?: string
  /** 确认人（登录过系统的用户 id）；不指定就只有管理员能审核 */
  reviewerId?: string | null
  /** 催办时同时推送的渠道（「定时推送」里配置的群 / 邮箱）；不选就只发站内通知 */
  remindChannels?: string[]
  units: CollectUnitInput[]
}

export interface Collect {
  id: string
  name: string
  datasetId: string
  /** 数据集已被删除时为空 */
  datasetName: string | null
  unitColumn: string
  periodColumn: string
  dimension: string
  unitCount: number
  /** 指派了负责人的单位数 */
  assignedCount: number
  periodCount: number
  /** 最近一期及它的提交进度（已提交含已确认，不含已退回） */
  latestPeriod: string | null
  latestSubmitted: number
  latestConfirmed: number
  /** 最近一期的截止日期，以及到期没提交的单位数 */
  latestDeadline: string | null
  latestOverdue: number
  /** 催办推送的渠道 id */
  remindChannels: string[]
  /** 确认人（没指定就只有管理员审核） */
  reviewerId: string | null
  reviewerName: string | null
  createdByName: string
  createdAt: string
  updatedAt: string
}

/** pending 还没提交 / submitted 已提交（等确认）/ returned 已退回 / confirmed 已确认（锁定） */
export type CollectStatus = 'pending' | 'submitted' | 'returned' | 'confirmed'

export interface CollectUnitStatus {
  unit: string
  userId: string | null
  userName: string | null
  status: CollectStatus
  submittedAt: string | null
  submittedByName: string | null
  /** 退回原因（已退回的才有） */
  returnReason: string | null
  reviewedAt: string | null
  reviewedByName: string | null
  /** 过了截止日期还没提交（含被退回没改好的） */
  overdue: boolean
  /** 上次被催办的时间 */
  remindedAt: string | null
  /** 数据集里这个单位这一期现在有几行 */
  rows: number
}

export interface CollectPeriod {
  period: string
  openedAt: string
  total: number
  /** 已提交（含已确认，不含已退回） */
  submitted: number
  confirmed: number
  /** 截止日期（年-月-日，没设为空）和到期没提交的单位数 */
  deadline: string | null
  overdue: number
}

export interface CollectDetail {
  collect: Collect
  /** 能不能开启 / 删除期次（只有管理员；确认人只能审核） */
  canManage: boolean
  /** 已开启的期次，新的在前 */
  periods: CollectPeriod[]
  /** units 是哪一期的状态；还没有任何期次时为空 */
  period: string | null
  units: CollectUnitStatus[]
}

export interface MyPeriod {
  period: string
  status: CollectStatus
  submittedAt: string | null
  /** 退回原因（已退回的才有） */
  returnReason: string | null
  rows: number
  /** 这一期的截止日期；过了截止日期还没提交（含被退回没改好的）overdue 为 true */
  deadline: string | null
  overdue: boolean
}

export interface MyTask {
  id: string
  name: string
  datasetId: string
  datasetName: string | null
  unitColumn: string
  periodColumn: string
  units: { unit: string, periods: MyPeriod[] }[]
  /** 还要我动手的（单位 × 已开启的期次）个数：还没填的，加上被退回的 */
  pending: number
  /** 其中已经逾期的 */
  overdue: number
  /** 我是不是这个任务的确认人 */
  reviewer: boolean
  /** 确认人姓名（没指定就是管理员） */
  reviewerName: string | null
  /** 我是确认人时：最近几期里已提交、等我确认的个数 */
  toReview: number
}

export function listCollectsApi() {
  return request<Collect[]>({ url: url('/collects'), method: 'get' })
}

export function createCollectApi(data: CollectInput) {
  return request<string>({ url: url('/collects'), method: 'post', data })
}

export function collectDetailApi(id: string, period?: string) {
  return request<CollectDetail>({ url: url(`/collects/${id}`), method: 'get', params: period ? { period } : {} })
}

export function updateCollectApi(id: string, data: { name: string, reviewerId?: string | null, remindChannels?: string[], units: CollectUnitInput[] }) {
  return request<boolean>({ url: url(`/collects/${id}`), method: 'put', data })
}

export function deleteCollectApi(id: string) {
  return request<boolean>({ url: url(`/collects/${id}`), method: 'delete' })
}

export function openCollectPeriodApi(id: string, period: string, deadline?: string | null) {
  return request<boolean>({ url: url(`/collects/${id}/periods`), method: 'post', data: { period, deadline: deadline || undefined } })
}

/** 设置 / 修改 / 取消（传空）某一期的截止日期 */
export function setCollectDeadlineApi(id: string, period: string, deadline: string | null) {
  return request<boolean>({ url: url(`/collects/${id}/periods/${encodeURIComponent(period)}`), method: 'put', data: { deadline } })
}

export interface RemindOutcome {
  /** 发了站内通知的单位数 */
  notified: number
  /** 刚催过、这次跳过的 */
  skipped: number
  /** 没有负责人、无处可催的 */
  unassigned: number
  /** 推送到群 / 邮箱的结果 */
  channels: { channelId: string, channelName: string, ok: boolean, message: string | null }[]
}

/** 催办：给没提交的单位的负责人发站内通知，任务配了渠道的还把名单推到群 / 邮箱；units 不传就催这一期所有没提交的 */
export function remindCollectApi(id: string, period: string, units?: string[]) {
  return request<RemindOutcome>({ url: url(`/collects/${id}/remind`), method: 'post', data: { period, units }, timeout: 90 * 1000 })
}

export function deleteCollectPeriodApi(id: string, period: string) {
  return request<boolean>({ url: url(`/collects/${id}/periods/${encodeURIComponent(period)}`), method: 'delete' })
}

export interface OpenedWorkbook {
  workbookId: string
  /** 这次新生成的（false 表示打开的是之前生成的） */
  created: boolean
}

/** 为（任务、单位、期次）生成填报工作簿，里面放好了填报区，单位和期次自动带上；同一个人再点返回已有的 */
export function openCollectWorkbookApi(id: string, unit: string, period: string) {
  return request<OpenedWorkbook>({ url: url(`/collects/${id}/workbook`), method: 'post', data: { unit, period }, timeout: 60 * 1000 })
}

export type ReviewAction = 'confirm' | 'return' | 'reopen'

/** 确认人（或管理员）处理一期里的若干单位：确认并锁定 / 退回重填（要写原因）/ 撤销确认；一次要么全成功要么都不变 */
export function reviewCollectApi(id: string, data: { period: string, units: string[], action: ReviewAction, reason?: string }) {
  return request<{ changed: number }>({ url: url(`/collects/${id}/review`), method: 'post', data })
}

/** 我负责的单位在最近几期里填了没有（受数据范围限制的用户也能调用） */
export function myCollectsApi() {
  return request<MyTask[]>({ url: url('/collects/mine'), method: 'get' })
}
