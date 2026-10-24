import { LEDGER_BASE } from '@/api/ledger-base'
import { t } from '@/i18n'
import request from '@/utils/request'

/** 事件 Webhook：数据集导入、同步、填报、审批、告警等事件发生时 POST 到外部地址（只有管理员能管理） */
const url = (path: string) => `${LEDGER_BASE}/api/ledger${path}`

export interface WebhookEvent {
  name: string
  /** 服务端的中文说明（界面优先用下面的 EVENT_LABEL） */
  description: string
}

export interface Webhook {
  id: string
  name: string
  /** 查询参数部分打码 */
  url: string
  /** 订阅的事件；['*'] 表示全部 */
  events: string[]
  enabled: boolean
  createdBy: string
  createdAt: string
  lastStatus: 'success' | 'failed' | null
  lastAt: string | null
  lastError: string | null
  /** 排队等待发送 / 重试的个数 */
  pending: number
}

export interface WebhookInput {
  name: string
  /** 修改时不填表示不变 */
  url?: string
  events: string[]
  enabled: boolean
}

export interface WebhookDelivery {
  id: string
  event: string
  status: 'pending' | 'success' | 'failed' | 'cancelled'
  attempts: number
  httpStatus: number | null
  error: string | null
  response: string | null
  createdAt: string
  lastAttemptAt: string | null
  nextAt: string | null
  payload: unknown
}

export const EVENT_LABEL: Record<string, string> = {
  'dataset.created': t('新建数据集'),
  'dataset.imported': t('向已有数据集导入'),
  'dataset.deleted': t('删除数据集'),
  'dataset.refreshed': t('外部数据集文件有变化'),
  'sql.refreshed': t('SQL 查询重算'),
  'connector.synced': t('数据连接同步结束'),
  'writeback.submitted': t('填报提交'),
  'collect.reviewed': t('收集任务审核'),
  'approval.created': t('发起文件审批'),
  'approval.decided': t('审批通过 / 驳回 / 撤回'),
  'alert.triggered': t('告警触发'),
  'ping': t('测试'),
  '*': t('全部事件'),
}

export function listWebhooksApi() {
  return request<{ hooks: Webhook[], events: WebhookEvent[] }>({ url: url('/webhooks'), method: 'get' })
}

/** 返回 id 与签名密钥（只显示这一次） */
export function createWebhookApi(data: WebhookInput) {
  return request<{ id: string, secret: string }>({ url: url('/webhooks'), method: 'post', data })
}

export function updateWebhookApi(id: string, data: WebhookInput) {
  return request<boolean>({ url: url(`/webhooks/${id}`), method: 'put', data })
}

export function deleteWebhookApi(id: string) {
  return request<boolean>({ url: url(`/webhooks/${id}`), method: 'delete' })
}

export function rotateWebhookSecretApi(id: string) {
  return request<string>({ url: url(`/webhooks/${id}/secret`), method: 'post' })
}

/** 立即发一个 ping（等待结果，最多约 10 秒） */
export function testWebhookApi(id: string) {
  return request<WebhookDelivery>({ url: url(`/webhooks/${id}/test`), method: 'post', timeout: 30 * 1000 })
}

export function webhookDeliveriesApi(id: string, limit = 50) {
  return request<WebhookDelivery[]>({ url: url(`/webhooks/${id}/deliveries`), method: 'get', params: { limit } })
}

export function redeliverWebhookApi(deliveryId: string) {
  return request<boolean>({ url: url(`/webhooks/deliveries/${deliveryId}/redeliver`), method: 'post' })
}
