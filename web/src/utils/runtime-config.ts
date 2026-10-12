/**
 * 运行时配置（web/config/index.js 写入的全局变量）
 *
 * 旧版本（项目原名 WebExcel）的配置文件写的是 window.__webexcel_config；升级时安装脚本会保留现场改过的
 * config/index.js，所以这里两个名字都认，新名字优先
 */
export function runtimeConfig(): GridlakeConfig {
  return window.__gridlake_config ?? window.__webexcel_config ?? {}
}
