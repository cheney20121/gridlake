/**
 * 表格服务（ledger-server）的地址
 *
 * 优先级：运行时配置 config/index.js 的 ledgerApi（部署后可改，无需重新构建）→ 构建变量 VITE_LEDGER_API → 同源 /ledger-api
 */
function resolveLedgerBase() {
  const base = window.__webexcel_config?.ledgerApi || import.meta.env.VITE_LEDGER_API || '/ledger-api'
  return new URL(base, window.location.origin).href.replace(/\/$/, '')
}

export const LEDGER_BASE = resolveLedgerBase()

/** 表格服务与页面同源：登录用服务端写的 HttpOnly cookie；跨域部署时浏览器不会带 cookie，改用 x-user-token 请求头 */
export const LEDGER_SAME_ORIGIN = new URL(LEDGER_BASE).origin === window.location.origin
