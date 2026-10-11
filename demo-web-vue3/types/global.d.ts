/** 运行时配置：部署后可改的 web/config/index.js（`window.__webexcel_config = { ... }`），不需要重新构建 */
interface WebExcelConfig {
  /** 表格服务（ledger-server）地址；页面由表格服务提供时留空（同源访问 /ledger-api 或根路径） */
  ledgerApi?: string
  /** 网关登录模式：未登录时跳转的统一登录地址（会带上 redirect 参数） */
  loginUrl?: string
  /** 登录令牌所在的 cookie 名（默认 webexcel-token；网关模式改成网关写入的 cookie 名） */
  tokenCookie?: string
}

interface Window {
  __webexcel_config?: WebExcelConfig
}
