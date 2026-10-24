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
  /** 是否设了访问密码 */
  protected: boolean
}

export interface CreatedShareLink extends ShareLink {
  /** 完整令牌：只在创建时返回这一次 */
  token: string
}

export function listShareLinksApi(workbookId: string) {
  return request<ShareLink[]>({ url: url(`/workbooks/${workbookId}/shares`), method: 'get' })
}

/** days 为 null 表示永不过期；password 为空表示不设访问密码 */
export function createShareLinkApi(workbookId: string, note: string, days: number | null, password = '') {
  return request<CreatedShareLink>({ url: url(`/workbooks/${workbookId}/shares`), method: 'post', data: { note, days, ...(password ? { password } : {}) } })
}

/** 访客输入访问密码：对了返回通行证（之后的请求带在 x-share-proof 里，见 share-token.ts） */
export function unlockShareApi(token: string, password: string) {
  return request<{ proof: string }>({ url: url('/share/unlock'), method: 'post', data: { token, password }, anonymous: true, silent: true })
}

export function revokeShareLinkApi(id: string) {
  return request<boolean>({ url: url(`/shares/${id}`), method: 'delete' })
}

/** 嵌入到别的网页里的 iframe 代码（?embed=1 隐藏分享页顶栏）；别的网站要显示它，服务端须设置 LEDGER_EMBED_ORIGINS */
export function embedCode(token: string) {
  return `<iframe src="${shareUrl(token)}?embed=1" style="width:100%;height:600px;border:0" referrerpolicy="no-referrer" loading="lazy" allowfullscreen></iframe>`
}

/** 分享页面的完整地址 */
export function shareUrl(token: string) {
  return `${window.location.origin}${import.meta.env.BASE_URL}share/${token}`.replace(/([^:])\/{2,}/g, '$1/')
}
