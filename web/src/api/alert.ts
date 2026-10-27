import type { DatasetFilter } from '@/api/dataset'
import type { ChannelResult, ReportChannel, ReportSchedule } from '@/api/report'
import { LEDGER_BASE } from '@/api/ledger-base'
import { t } from '@/i18n'
import request from '@/utils/request'

/** 阈值告警：指标（可按某一列分组）超过阈值时，推送到群或邮箱 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger${path}`

export type AlertAgg = 'sum' | 'count' | 'avg' | 'min' | 'max' | 'count_distinct'

export const AGG_LABEL: Record<AlertAgg, string> = {
  sum: t('求和'),
  count: t('计数'),
  avg: t('平均'),
  min: t('最小'),
  max: t('最大'),
  count_distinct: t('去重计数'),
}

export interface AlertPart {
  column?: string | null
  agg: AlertAgg
}

export type AlertMetric
  = | { kind: 'agg', column?: string | null, agg: AlertAgg }
    | { kind: 'ratio', numerator: AlertPart, denominator: AlertPart }
    /** 数据停更：某日期列的最新一条距今几天 */
    | { kind: 'freshness', dateColumn: string }

export type AlertWindowPreset = 'none' | 'yesterday' | 'last_7' | 'last_30' | 'this_month' | 'last_month'

export const WINDOW_LABEL: Record<AlertWindowPreset, string> = {
  none: t('全部数据'),
  yesterday: t('昨天'),
  last_7: t('近 7 天（不含今天）'),
  last_30: t('近 30 天（不含今天）'),
  this_month: t('本月（到今天）'),
  last_month: t('上月'),
}

export type AlertOp = 'gt' | 'gte' | 'lt' | 'lte'

export const OP_LABEL: Record<AlertOp, string> = {
  gt: t('大于'),
  gte: t('大于等于'),
  lt: t('小于'),
  lte: t('小于等于'),
}

/** 条件判断的对象：当前值 / 较上期的相对变化 / 较上期的绝对变化 */
export type AlertBasis = 'value' | 'change_pct' | 'change_abs'

export const BASIS_LABEL: Record<AlertBasis, string> = {
  value: t('当前值'),
  change_pct: t('较上期变化（百分比）'),
  change_abs: t('较上期变化（绝对值）'),
}

/** 能做「较上期」比较的时间窗口 */
export const COMPARABLE_PRESETS: AlertWindowPreset[] = ['yesterday', 'last_7', 'last_30', 'last_month']

export type AlertRepeat
  = | { kind: 'until_recovered' }
    | { kind: 'every_run' }
    | { kind: 'every_hours', hours: number }

export interface AlertGuard {
  column?: string | null
  agg: AlertAgg
  min: number
}

export interface AlertInput {
  name: string
  datasetId: string
  metric: AlertMetric
  metricLabel?: string | null
  groupBy?: string | null
  filters: DatasetFilter[]
  window: { dateColumn?: string | null, preset: AlertWindowPreset }
  /** 比率的阈值用小数（5% 写 0.05）；变化率同理（下降 20% 写 -0.2） */
  condition: { op: AlertOp, threshold: number, basis?: AlertBasis }
  guard?: AlertGuard | null
  schedule: ReportSchedule
  channelIds: string[]
  repeat: AlertRepeat
  /** 恢复正常时也发一条通知 */
  notifyRecovery?: boolean
  enabled: boolean
}

export interface AlertRun {
  id: string
  alertId: string
  trigger: 'manual' | 'schedule' | 'after_sync' | 'after_data'
  status: 'running' | 'success' | 'partial' | 'failed'
  startedAt: string
  finishedAt: string | null
  message: string | null
  /** 满足条件的分组数 */
  hits: number
  /** 本次通知了几个分组 */
  notified: number
  results: ChannelResult[]
}

export interface Alert extends AlertInput {
  id: string
  datasetName: string | null
  createdBy: string
  createdByName: string
  createdAt: string
  updatedAt: string
  lastRun: AlertRun | null
  nextRun: string | null
  running: boolean
  /** 当前处于触发中的分组数 */
  firing: number
}

export interface AlertPreviewRow {
  key: string
  value: number | null
  valueText: string
  /** 比较上期时：上期的值、变化量 */
  prevText?: string | null
  changeText?: string | null
  guard: number | null
  verdict: 'hit' | 'clear' | 'unknown'
}

export interface AlertPreview {
  rows: AlertPreviewRow[]
  totalGroups: number
  hits: number
  clear: number
  /** 没有值或样本量不足的分组数 */
  unknown: number
  window: string | null
  metricLabel: string
  condition: string
  guard: string | null
  elapsedMs: number
}

export function listAlertsApi() {
  return request<{ items: Alert[], channels: ReportChannel[], smtpConfigured: boolean }>({ url: url('/alerts'), method: 'get' })
}

export function createAlertApi(data: AlertInput) {
  return request<string>({ url: url('/alerts'), method: 'post', data })
}

export function updateAlertApi(id: string, data: AlertInput) {
  return request<boolean>({ url: url(`/alerts/${id}`), method: 'put', data })
}

export function deleteAlertApi(id: string) {
  return request<boolean>({ url: url(`/alerts/${id}`), method: 'delete' })
}

/** 试算：按当前数据算一遍会命中哪些分组，不保存、不发送 */
export function previewAlertApi(data: AlertInput) {
  return request<AlertPreview>({ url: url('/alerts/preview'), method: 'post', data, timeout: 5 * 60 * 1000 })
}

/** 立即评估并按规则通知（等待完成，返回本次记录） */
export function evaluateAlertApi(id: string) {
  return request<AlertRun>({ url: url(`/alerts/${id}/evaluate`), method: 'post', timeout: 6 * 60 * 1000 })
}

export function alertRunsApi(id: string, limit = 30) {
  return request<AlertRun[]>({ url: url(`/alerts/${id}/runs`), method: 'get', params: { limit } })
}
