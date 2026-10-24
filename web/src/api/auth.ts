import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/**
 * 表格服务的登录身份、内置账号与操作审计。
 * 登录方式由服务端的 LEDGER_AUTH_MODE 决定：off 不登录 / local 内置账号 / gateway 网关令牌 / proxy 反向代理传入身份
 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger${path}`

export interface LedgerUser {
  id: string
  name: string
  /** 管理员：可管理数据连接、推送渠道、外部 AI 令牌，查看操作审计，管理所有人的工作簿与数据集 */
  admin: boolean
}

/** 数据范围（行级权限）：受限时只能看到这些店铺的数据 */
export interface LedgerScope {
  restricted: boolean
  /** 「店铺」维度的可见取值（老字段，等于 dimensions['店铺']） */
  shops: string[]
  /** 受限的维度 → 可见的取值 */
  dimensions?: Record<string, string[]>
}

export interface LedgerMe {
  authRequired: boolean
  mode: 'off' | 'local' | 'gateway' | 'proxy' | 'share'
  user: LedgerUser | null
  scope: LedgerScope
  /** 启用了哪些场景包（服务端 LEDGER_PACKS）；老版本服务端没有这个字段 */
  packs?: { ecommerce: boolean }
  /** 通过分享链接访问时：被分享的工作簿（此时 mode 为 share，只能只读查看它）；passwordRequired 为真表示还要输入访问密码，此时没有工作簿信息 */
  share?: { workbookId: string, workbookName: string, passwordRequired?: boolean }
  /** 管理员要求两步验证而当前账号还没开启：必须先设置，其他功能暂时不可用 */
  twoFactorRequired?: boolean
  /** 服务端配置了单点登录（OIDC）：登录页显示「用 XX 登录」；loginPath 是整页跳转的入口，不是 fetch */
  sso?: { name: string, loginPath: string }
}

/** 开始单点登录的地址（整页跳转）；登录完成后服务端把浏览器送回 returnTo（站内路径） */
export function ssoLoginUrl(loginPath: string, returnTo: string) {
  return `${LEDGER_BASE}/api${loginPath}?return=${encodeURIComponent(returnTo)}`
}

export function getLedgerMeApi() {
  return request<LedgerMe>({ url: url('/me'), method: 'get' })
}

// ── 内置账号（LEDGER_AUTH_MODE=local）──

export interface LoginResult {
  token: string
  /** 令牌有效天数 */
  days: number
  user: LedgerUser
  /** 密码对了，但这个账号启用了两步验证、还没给验证码：没有令牌，带上 code 再提交一次 */
  needCode?: boolean
}

export function loginApi(username: string, password: string, code?: string) {
  return request<LoginResult>({ url: url('/auth/login'), method: 'post', data: { username, password, ...(code ? { code } : {}) }, silent: true })
}

// ── 两步验证（TOTP）──

export interface TwoFactorStatus {
  enabled: boolean
  /** 还剩几个没用过的恢复码 */
  recoveryLeft: number
}

export interface TwoFactorSetup {
  /** Base32 密钥（手工输入用） */
  secret: string
  /** otpauth:// 地址 */
  uri: string
}

export function getTwoFactorApi() {
  return request<TwoFactorStatus>({ url: url('/auth/2fa'), method: 'get' })
}

export function setupTwoFactorApi() {
  return request<TwoFactorSetup>({ url: url('/auth/2fa/setup'), method: 'post', silent: true })
}

/** 用验证器里的 6 位数字确认并启用；返回一次性恢复码（只此一次） */
export function enableTwoFactorApi(code: string) {
  return request<{ recoveryCodes: string[] }>({ url: url('/auth/2fa/enable'), method: 'post', data: { code }, silent: true })
}

export function disableTwoFactorApi(password: string, code: string) {
  return request<boolean>({ url: url('/auth/2fa/disable'), method: 'post', data: { password, code }, silent: true })
}

export function logoutApi() {
  return request<boolean>({ url: url('/auth/logout'), method: 'post' })
}

export function changePasswordApi(oldPassword: string, newPassword: string) {
  return request<boolean>({ url: url('/auth/password'), method: 'post', data: { oldPassword, newPassword } })
}

export interface Account {
  id: string
  username: string
  name: string
  admin: boolean
  disabled: boolean
  /** 是否启用了两步验证 */
  twoFactor: boolean
  createdAt: string
  lastLoginAt: string | null
}

export function listAccountsApi() {
  return request<Account[]>({ url: url('/accounts'), method: 'get' })
}

export function createAccountApi(data: { username: string, name: string, password: string, admin: boolean }) {
  return request<Account>({ url: url('/accounts'), method: 'post', data })
}

export function updateAccountApi(id: string, data: { name?: string, admin?: boolean, disabled?: boolean, password?: string, resetTotp?: boolean }) {
  return request<Account>({ url: url(`/accounts/${id}`), method: 'put', data })
}

export interface AuditItem {
  at: string
  userId: string
  userName: string
  ip: string
  action: string
  targetKind: string
  targetId: string
  targetName: string
  /** HTTP 状态码：>= 400 为失败或被拒绝 */
  status: number
  ms: number
}

export interface AuditPage {
  total: number
  items: AuditItem[]
  /** 保留天数 */
  days: number
}

export interface AuditQuery {
  user?: string
  action?: string
  from?: string
  to?: string
  failed?: boolean
  page?: number
  size?: number
}

export function getAuditApi(params: AuditQuery) {
  return request<AuditPage>({ url: url('/audit'), method: 'get', params })
}

// ── 数据范围设置（仅管理员）──

export interface ScopeRule {
  /** 用户 id；* 是默认规则（没有单独规则的用户） */
  subjectId: string
  subjectName: string
  /** 维度（店铺 / 地区 / 部门…） */
  dimension: string
  /** all 不限 / values 指定取值 */
  mode: 'all' | 'values'
  values: string[]
  updatedBy: string | null
  updatedAt: string | null
}

export interface ScopeDatasetBinding {
  id: string
  name: string
  kind: 'builtin' | 'table' | 'view' | 'recon'
  /** 设置的过滤列（列 key） */
  scopeColumn: string | null
  /** 设置的维度（没设置时按「店铺」） */
  scopeDimension: string
  /** 设置为所有人可见 */
  open: boolean
  /** column 按列过滤 / open 所有人可见 / inherit 沿用左表 / hidden 受限用户看不到 */
  effective: 'column' | 'open' | 'inherit' | 'hidden'
  effectiveColumn: string | null
  effectiveDimension: string | null
  columns: { key: string, title: string }[]
}

export interface ScopeOverview {
  rules: ScopeRule[]
  datasets: ScopeDatasetBinding[]
  /** 已经用到的维度 */
  dimensions: string[]
  /** 各维度的取值候选（目前只有「店铺」：来自店铺档案） */
  candidates: Record<string, string[]>
}

export function getScopeOverviewApi() {
  return request<ScopeOverview>({ url: url('/data-scope'), method: 'get' })
}

export function saveScopeRuleApi(data: { subjectId: string, subjectName?: string, dimension: string, mode: 'all' | 'values', values: string[] }) {
  return request<boolean>({ url: url('/data-scope/rules'), method: 'put', data })
}

/** 不传 dimension 删掉这个用户的全部规则 */
export function deleteScopeRuleApi(subjectId: string, dimension?: string) {
  const q = dimension ? `?dimension=${encodeURIComponent(dimension)}` : ''
  return request<boolean>({ url: url(`/data-scope/rules/${encodeURIComponent(subjectId)}${q}`), method: 'delete' })
}

export function saveScopeBindingApi(datasetId: string, data: { scopeColumn?: string | null, scopeDimension?: string, open: boolean }) {
  return request<boolean>({ url: url(`/data-scope/datasets/${datasetId}`), method: 'put', data })
}

// ── 个人访问令牌（脚本、CI 调用接口用；只在内置账号登录方式下可用）──

export interface AccessToken {
  id: string
  name: string
  /** 令牌开头的几位，用来认出是哪一个 */
  prefix: string
  createdAt: string
  /** 空表示永不过期 */
  expiresAt: string | null
  lastUsedAt: string | null
  expired: boolean
  /** 只读令牌（glr_ 开头）：只能读取数据，写操作一律被拒绝 */
  readOnly: boolean
  /** 限定能访问的数据集（空表示不限）；name 为 null 表示数据集已被删除 */
  datasets: { id: string, name: string | null }[]
}

export interface CreatedAccessToken extends AccessToken {
  /** 完整令牌：只在创建时返回这一次 */
  token: string
}

export function listAccessTokensApi() {
  return request<AccessToken[]>({ url: url('/tokens'), method: 'get' })
}

/** days 为 0 表示永不过期；readOnly 生成只读令牌；datasets 非空时令牌只能访问这些数据集（id） */
export function createAccessTokenApi(name: string, days: number, readOnly = false, datasets: string[] = []) {
  return request<CreatedAccessToken>({ url: url('/tokens'), method: 'post', data: { name, days, readOnly, datasets }, silent: true })
}

export function revokeAccessTokenApi(id: string) {
  return request<boolean>({ url: url(`/tokens/${encodeURIComponent(id)}`), method: 'delete' })
}
