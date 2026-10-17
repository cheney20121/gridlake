/**
 * 分享链接的令牌：打开 /share/<令牌> 页面时记下，之后的请求都带上 x-share-token 请求头（见 request.ts），
 * 服务端据此把请求当作「只读访客」处理（只能读被分享的工作簿和它用到的数据集）。不写入 cookie / localStorage
 */
let token: string | null = null

export function setShareToken(value: string | null) {
  token = value
}

export function getShareToken(): string | null {
  return token
}
