/**
 * 分享链接的令牌：打开 /share/<令牌> 页面时记下，之后的请求都带上 x-share-token 请求头（见 request.ts），
 * 服务端据此把请求当作「只读访客」处理（只能读被分享的工作簿和它用到的数据集）。不写入 cookie / localStorage。
 *
 * 链接设了访问密码时，输对密码换到的通行证随请求放在 x-share-proof 里。通行证只存在内存和本标签页的 sessionStorage
 * （刷新页面不用重输，关掉标签页就没了）
 */
let token: string | null = null
let proof: string | null = null

const storageKey = (t: string) => `gridlake_share_proof:${t.slice(0, 16)}`

export function setShareToken(value: string | null) {
  token = value
  proof = null
  if (value) {
    try {
      proof = sessionStorage.getItem(storageKey(value))
    }
    catch {}
  }
}

export function getShareToken(): string | null {
  return token
}

export function setShareProof(value: string | null) {
  proof = value
  if (!token)
    return
  try {
    if (value)
      sessionStorage.setItem(storageKey(token), value)
    else
      sessionStorage.removeItem(storageKey(token))
  }
  catch {}
}

export function getShareProof(): string | null {
  return proof
}
