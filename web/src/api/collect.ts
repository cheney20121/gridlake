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
  /** 最近一期及它的提交进度 */
  latestPeriod: string | null
  latestSubmitted: number
  createdByName: string
  createdAt: string
  updatedAt: string
}

export interface CollectUnitStatus {
  unit: string
  userId: string | null
  userName: string | null
  status: 'submitted' | 'pending'
  submittedAt: string | null
  submittedByName: string | null
  /** 数据集里这个单位这一期现在有几行 */
  rows: number
}

export interface CollectPeriod {
  period: string
  openedAt: string
  total: number
  submitted: number
}

export interface CollectDetail {
  collect: Collect
  /** 已开启的期次，新的在前 */
  periods: CollectPeriod[]
  /** units 是哪一期的状态；还没有任何期次时为空 */
  period: string | null
  units: CollectUnitStatus[]
}

export interface MyPeriod {
  period: string
  status: 'submitted' | 'pending'
  submittedAt: string | null
  rows: number
}

export interface MyTask {
  id: string
  name: string
  datasetId: string
  datasetName: string | null
  unitColumn: string
  periodColumn: string
  units: { unit: string, periods: MyPeriod[] }[]
  /** 还没填的（单位 × 已开启的期次）个数 */
  pending: number
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

export function updateCollectApi(id: string, data: { name: string, units: CollectUnitInput[] }) {
  return request<boolean>({ url: url(`/collects/${id}`), method: 'put', data })
}

export function deleteCollectApi(id: string) {
  return request<boolean>({ url: url(`/collects/${id}`), method: 'delete' })
}

export function openCollectPeriodApi(id: string, period: string) {
  return request<boolean>({ url: url(`/collects/${id}/periods`), method: 'post', data: { period } })
}

export function deleteCollectPeriodApi(id: string, period: string) {
  return request<boolean>({ url: url(`/collects/${id}/periods/${encodeURIComponent(period)}`), method: 'delete' })
}

/** 我负责的单位在最近几期里填了没有（受数据范围限制的用户也能调用） */
export function myCollectsApi() {
  return request<MyTask[]>({ url: url('/collects/mine'), method: 'get' })
}
