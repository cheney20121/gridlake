import Cookies from 'js-cookie'

/**
 * 页面可读的登录令牌，请求时作为 x-user-token 请求头发给表格服务。只用于两种情况：
 * - 网关模式：由统一登录入口写入同名 cookie（cookie 名可在运行时配置 tokenCookie 里改）；
 * - 内置账号 + 前后端跨域部署：由登录页写入。
 * 内置账号 + 同源部署（默认）不走这里：服务端登录时写 HttpOnly 会话 cookie，页面读不到，也不需要读
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
