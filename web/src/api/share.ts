import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/** 工作簿的只读分享链接：把工作簿（和它用到的数据集）开放给没有账号的人查看 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger${path}`

export interface ShareLink {
  id: string
  workbookId: string
  note: string
  /** 令牌开头几位，用来在列表里认出是哪一个 */
  prefix: string
  createdByName: string
  createdAt: string
  /** 空表示永不过期 */
  expiresAt: string | null
  lastUsedAt: string | null
  viewCount: number
  expired: boolean
}

export interface CreatedShareLink extends ShareLink {
  /** 完整令牌：只在创建时返回这一次 */
  token: string
}

export function listShareLinksApi(workbookId: string) {
  return request<ShareLink[]>({ url: url(`/workbooks/${workbookId}/shares`), method: 'get' })
}

/** days 为 null 表示永不过期 */
export function createShareLinkApi(workbookId: string, note: string, days: number | null) {
  return request<CreatedShareLink>({ url: url(`/workbooks/${workbookId}/shares`), method: 'post', data: { note, days } })
}

export function revokeShareLinkApi(id: string) {
  return request<boolean>({ url: url(`/shares/${id}`), method: 'delete' })
}

/** 分享页面的完整地址 */
export function shareUrl(token: string) {
  return `${window.location.origin}${import.meta.env.BASE_URL}share/${token}`.replace(/([^:])\/{2,}/g, '$1/')
}
