import Cookies from 'js-cookie'

/**
 * 登录令牌：放在 cookie 里（同源的 WebSocket / 下载链接也能带上），请求时作为 x-user-token 请求头发给表格服务。
 * 内置账号模式由登录页写入；网关模式由统一登录入口写入同名 cookie（cookie 名可在运行时配置 tokenCookie 里改）
 */
const TOKEN_KEY = (window.__webexcel_config?.tokenCookie as string | undefined) || 'webexcel-token'

export function getToken(): string | undefined {
  return Cookies.get(TOKEN_KEY) || undefined
}

export function setToken(token: string, days = 7) {
  Cookies.set(TOKEN_KEY, token, { expires: days, sameSite: 'Lax', secure: window.location.protocol === 'https:' })
}

export function removeToken() {
  Cookies.remove(TOKEN_KEY)
}
