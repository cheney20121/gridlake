import { LEDGER_BASE } from '@/api/ledger-base'
import { t } from '@/i18n'
import request from '@/utils/request'

/** 定时推送：把仪表盘页按计划推送到企业微信 / 钉钉 / 飞书群或邮箱 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger${path}`

export type ChannelKind = 'wecom' | 'dingtalk' | 'feishu' | 'email'

export const CHANNEL_LABEL: Record<ChannelKind, string> = {
  wecom: t('企业微信'),
  dingtalk: t('钉钉'),
  feishu: t('飞书'),
  email: t('邮件'),
}

export interface ReportChannel {
  id: string
  name: string
  kind: ChannelKind
  /** 打码后的 Webhook 地址 / 收件人 */
  target: string
  hasSecret: boolean
  createdAt: string
}

export interface ChannelInput {
  name: string
  kind: ChannelKind
  /** 修改时留空沿用已保存的 */
  target?: string
  /** 加签密钥：修改时不传沿用，空串清除 */
  secret?: string
}

export type ReportSchedule
  = | { kind: 'manual' }
    | { kind: 'daily', at: string }
    | { kind: 'weekly', weekday: number, at: string }
    | { kind: 'monthly', day: number, at: string }
    | { kind: 'after_sync', connectorId: string }
    /** 某个数据集有新数据后：导入、数据连接同步、外部文件有变化、SQL 查询重算写入 */
    | { kind: 'after_data', datasetId: string }

export interface ChannelResult {
  channelId: string
  channelName: string
  ok: boolean
  message: string | null
}

export interface ReportRun {
  id: string
  reportId: string
  trigger: 'manual' | 'schedule' | 'after_sync' | 'after_data'
  status: 'running' | 'success' | 'partial' | 'failed'
  startedAt: string
  finishedAt: string | null
  message: string | null
  results: ChannelResult[]
}

export interface Report {
  id: string
  name: string
  workbookId: string
  workbookName: string | null
  sheetId: string
  sheetName: string | null
  title: string
  schedule: ReportSchedule
  channelIds: string[]
  attachExcel: boolean
  enabled: boolean
  createdAt: string
  updatedAt: string
  lastRun: ReportRun | null
  nextRun: string | null
  running: boolean
}

export interface ReportInput {
  name: string
  workbookId: string
  sheetId: string
  title: string
  schedule: ReportSchedule
  channelIds: string[]
  attachExcel: boolean
  enabled: boolean
}

export interface ReportSheet {
  id: string
  name: string
  /** 指标卡 / 明细卡 / 图表 / 切片器数量 */
  parts: number
}

export interface ReportPreview {
  content: {
    workbookName: string
    sheetName: string
    filters: string[]
    generatedAt: string
  }
  /** 邮件正文 */
  html: string
  /** 机器人消息（Markdown） */
  markdown: string
}

export function listReportsApi() {
  return request<{ items: Report[], channels: ReportChannel[], smtpConfigured: boolean, publicUrl: string | null }>({ url: url('/reports'), method: 'get' })
}

export function createReportApi(data: ReportInput) {
  return request<string>({ url: url('/reports'), method: 'post', data })
}

export function updateReportApi(id: string, data: ReportInput) {
  return request<boolean>({ url: url(`/reports/${id}`), method: 'put', data })
}

export function deleteReportApi(id: string) {
  return request<boolean>({ url: url(`/reports/${id}`), method: 'delete' })
}

/** 立即发送（等待完成，返回本次记录） */
export function sendReportApi(id: string) {
  return request<ReportRun>({ url: url(`/reports/${id}/send`), method: 'post', timeout: 4 * 60 * 1000 })
}

export function reportRunsApi(id: string, limit = 30) {
  return request<ReportRun[]>({ url: url(`/reports/${id}/runs`), method: 'get', params: { limit } })
}

export function previewReportApi(data: { workbookId: string, sheetId: string, title?: string }) {
  return request<ReportPreview>({ url: url('/reports/preview'), method: 'post', data, timeout: 2 * 60 * 1000 })
}

export function reportSheetsApi(workbookId: string) {
  return request<ReportSheet[]>({ url: url('/reports/sheets'), method: 'get', params: { workbookId } })
}

export function createChannelApi(data: ChannelInput) {
  return request<string>({ url: url('/report-channels'), method: 'post', data })
}

export function updateChannelApi(id: string, data: ChannelInput) {
  return request<boolean>({ url: url(`/report-channels/${id}`), method: 'put', data })
}

export function deleteChannelApi(id: string) {
  return request<boolean>({ url: url(`/report-channels/${id}`), method: 'delete' })
}

export function testChannelApi(id: string) {
  return request<boolean>({ url: url(`/report-channels/${id}/test`), method: 'post', timeout: 90 * 1000 })
}
