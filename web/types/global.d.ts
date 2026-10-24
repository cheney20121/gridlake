/** 运行时配置：部署后可改的 web/config/index.js（`window.__gridlake_config = { ... }`），不需要重新构建 */
interface GridlakeConfig {
  /** 表格服务（gridlake）地址；页面由表格服务提供时留空（同源访问 /ledger-api 或根路径） */
  ledgerApi?: string
  /** 网关登录模式：未登录时跳转的统一登录地址（会带上 redirect 参数） */
  loginUrl?: string
  /** 登录令牌所在的 cookie 名（默认 gridlake-token；网关模式改成网关写入的 cookie 名） */
  tokenCookie?: string
}

interface Window {
  __gridlake_config?: GridlakeConfig
  /** 旧名字（项目原名 WebExcel），只为兼容升级前写好的 config/index.js */
  __webexcel_config?: GridlakeConfig
}
