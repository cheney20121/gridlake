import type { AxiosProgressEvent, AxiosResponse } from 'axios'
import type { RuleTerm } from '@/features/excel/shop-ledger/summary-template'
import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/**
 * 店铺资金流水服务（ledger-server，Rust + DuckDB）
 *
 * request.ts 的拦截器会把 baseURL 固定改写为业务网关地址，这里使用绝对地址绕开，
 * 仍然复用其响应信封解析与错误提示。开发环境经 Vite 代理 /ledger-api 转发到 127.0.0.1:8787
 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger${path}`

export interface EntityCount {
  name: string
  rowCount: number
  /** 已保存的主体简称（仅聚合接口返回） */
  alias?: string | null
}

export interface InvalidNumber {
  column: string
  count: number
  sample: string
}

export interface ShopCandidate {
  shop: string
  /** 文件中有多少个商品 ID 在该店铺出现过 */
  matchedProducts: number
  /** 商品重合度 0~1 */
  productRatio: number
  /** 文件中已存在于该店铺的流水条数（合并导入时会被跳过） */
  duplicateRows: number
}

export interface LedgerImportPreview {
  /** 提交时回传，对应服务器暂存的文件 */
  token: string
  fileName: string
  fileSize: number
  fileHash: string
  rowCount: number
  /** 文件内重复的流水（同一流水号多次出现），只保留一条 */
  duplicateInFile: number
  periods: string[]
  timeFrom: string
  timeTo: string
  entities: EntityCount[]
  productCount: number
  /** 按匹配程度降序 */
  candidates: ShopCandidate[]
  suggestedShop: string | null
  /** 同一文件（内容相同）以前导入过 */
  sameFile: { shop: string, importedAt: string } | null
  missingColumns: string[]
  extraColumns: string[]
  invalidNumbers: InvalidNumber[]
  /** 文件涉及月份中已冻结（审批中 / 已锁账）的店铺月份，导入到这些店铺会被拒绝 */
  locks: LedgerPeriodLock[]
}

export type LedgerImportMode = 'merge' | 'replace'

export interface LedgerImportResult {
  importId: string
  shop: string
  fileName: string
  mode: LedgerImportMode
  rowCount: number
  insertedRows: number
  /** 合并模式下因已存在而跳过（含变更行） */
  skippedRows: number
  /** 流水号已存在但金额/方向/场景不同（未覆盖） */
  changedRows: number
  changedSamples: { id: string, time: string, before: string, after: string }[]
  /** 替换模式下删除的旧数据行数 */
  replacedRows: number
  duplicateInFile: number
  periods: string[]
  timeFrom: string
  timeTo: string
  entities: EntityCount[]
  missingColumns: string[]
  extraColumns: string[]
  invalidNumbers: InvalidNumber[]
  elapsedMs: number
}

/** 预览：上传文件，识别店铺、查重（不入库） */
export function previewLedgerImportApi(file: File, onUploadProgress?: (e: AxiosProgressEvent) => void) {
  const data = new FormData()
  data.append('file', file)
  return request<LedgerImportPreview>({
    url: url('/imports/preview'),
    method: 'post',
    data,
    timeout: 10 * 60 * 1000,
    onUploadProgress,
  })
}

/** 提交：把预览过的文件导入指定店铺 */
export function commitLedgerImportApi(token: string, shop: string, mode: LedgerImportMode, fileName: string) {
  return request<LedgerImportResult>({
    url: url('/imports/commit'),
    method: 'post',
    data: { token, shop, mode, fileName },
    timeout: 10 * 60 * 1000,
  })
}

export interface LedgerShop {
  name: string
  douyinShopId: string | null
  /** active 运营中 / closed 已关店 */
  status: 'active' | 'closed'
  remark: string | null
  createdAt: string
  rowCount: number
  periods: string[]
  entities: string[]
  productCount: number
  firstTime: string | null
  lastTime: string | null
}

export function getLedgerShopsApi() {
  return request<LedgerShop[]>({ url: url('/shops'), method: 'get' })
}

export function createLedgerShopApi(data: { name: string, douyinShopId?: string, remark?: string }) {
  return request<LedgerShop[]>({ url: url('/shops'), method: 'post', data })
}

/** 修改店铺；改名会同步更新历史明细与导入记录 */
export function updateLedgerShopApi(data: { name: string, newName?: string, douyinShopId?: string, status?: 'active' | 'closed', remark?: string }) {
  return request<LedgerShop[]>({ url: url('/shops'), method: 'put', data })
}

export interface LedgerCoverageCell {
  shop: string
  period: string
  rowCount: number
  /** complete 完整 / partial 有缺口或疑似截断 / missing 未导入 / not_expected 开店前或关店后 */
  status: 'complete' | 'partial' | 'missing' | 'not_expected'
  /** 每日笔数，下标 0 = 1 日 */
  days: number[]
  missingDays: number[]
  lowDays: number[]
  expectedFrom: number | null
  expectedTo: number | null
}

export interface LedgerCoverage {
  today: string
  periods: string[]
  shops: { name: string, status: 'active' | 'closed', medianDaily: number }[]
  cells: LedgerCoverageCell[]
}

/** 月结完整性：店铺 × 月份 */
export function getLedgerCoverageApi() {
  return request<LedgerCoverage>({ url: url('/coverage'), method: 'get' })
}

export interface LedgerPeriod {
  period: string
  shops: string[]
  rowCount: number
}

/** 已导入的月份（倒序）及各月店铺 */
export function getLedgerPeriodsApi() {
  return request<LedgerPeriod[]>({ url: url('/periods'), method: 'get' })
}

export interface LedgerPivot {
  period: string
  /** 维度列（商户主体名称/店铺/动账场景/动账方向/笔数）+ 各金额列合计 + 佣金(负)/佣金(正) */
  columns: string[]
  rows: (string | number)[][]
  shops: string[]
  /** 按流水笔数降序 */
  entities: EntityCount[]
  totalRows: number
}

/** 按 主体 × 店铺 × 动账场景 × 动账方向 聚合 */
export function getLedgerPivotApi(period: string) {
  return request<LedgerPivot>({ url: url('/pivot'), method: 'get', params: { period } })
}

/** 明细筛选条件；多选字段中的 "" 表示该字段为空 */
export interface LedgerDetailFilter {
  period: string
  /** 下钻上下文：固定的店铺 / 主体（来自汇总表所选列） */
  shop?: string
  entity?: string
  scenes?: string[]
  directions?: string[]
  billingTypes?: string[]
  entities?: string[]
  shops?: string[]
  /** 模糊匹配订单号 / 子订单号 / 流水号 / 售后编号 / 商品名称 / 达人名称 / 备注 */
  keyword?: string
  timeFrom?: string
  timeTo?: string
  amountMin?: number
  amountMax?: number
  /** 汇总项规则：只返回对该汇总项有贡献的流水，并附加「贡献金额」列 */
  rule?: RuleTerm[]
  /** 经营分析下钻的固定上下文；"" 表示该字段为空 */
  talentId?: string
  productId?: string
  /** 下单月份 YYYY-MM */
  orderMonth?: string
}

export interface LedgerDetailSort {
  column: string
  desc: boolean
}

export interface LedgerDetailRows {
  columns: string[]
  rows: (string | number | null)[][]
  offset: number
}

/** 按偏移量取一段明细（虚拟滚动按可视区域分段加载） */
export function queryLedgerRowsApi(filter: LedgerDetailFilter, offset: number, limit: number, sort?: LedgerDetailSort, signal?: AbortSignal) {
  return request<LedgerDetailRows>({
    url: url('/details/rows'),
    method: 'post',
    data: { filter, sort, offset, limit },
    signal,
  })
}

export interface LedgerFacetItem {
  value: string
  count: number
  /** 资金净额：入账为正、出账为负 */
  net: number
}

export interface LedgerDetailSummary {
  totalCount: number
  inAmount: number
  outAmount: number
  netAmount: number
  /** 规则下钻时各行贡献金额之和，应等于汇总表对应单元格 */
  contribution: number | null
  /** 各金额列合计 */
  sums: Record<string, number>
  /** 分面统计：动账场景 / 动账方向 / 计费类型 / 商户主体名称 / 店铺 */
  facets: Record<string, LedgerFacetItem[]>
}

/** 当前筛选的笔数、金额合计、规则贡献合计与分面统计 */
export function getLedgerDetailSummaryApi(filter: LedgerDetailFilter, signal?: AbortSignal) {
  return request<LedgerDetailSummary>({ url: url('/details/summary'), method: 'post', data: filter, signal })
}

/** 当前筛选结果导出为 CSV（返回 axios 完整响应，便于读取文件名） */
export function exportLedgerDetailsApi(filter: LedgerDetailFilter, sort?: LedgerDetailSort) {
  return request<AxiosResponse<Blob>>({
    url: url('/details/export'),
    method: 'post',
    data: { filter, sort },
    responseType: 'blob',
    type: 'download',
    timeout: 5 * 60 * 1000,
  })
}

export interface LedgerImportRecord {
  importId: string
  shop: string
  fileName: string | null
  periods: string[]
  entities: string[]
  rowCount: number
  replacedRows: number
  missingColumns: string[]
  extraColumns: string[]
  elapsedMs: number
  importedAt: string
}

/** 导入记录（最近 200 条） */
export function getLedgerImportsApi() {
  return request<LedgerImportRecord[]>({ url: url('/imports'), method: 'get' })
}

/** 删除某店铺某月的明细 */
export function deleteLedgerDataApi(shop: string, period: string) {
  return request<number>({ url: url('/data'), method: 'delete', params: { shop, period } })
}

export interface LedgerEntityAlias {
  entity: string
  alias: string
  updatedAt: string
}

/** 主体简称列表 */
export function getEntityAliasesApi() {
  return request<LedgerEntityAlias[]>({ url: url('/entity-aliases'), method: 'get' })
}

/** 批量保存主体简称；alias 为空表示删除（恢复为默认推导） */
export function saveEntityAliasesApi(items: { entity: string, alias: string | null }[]) {
  return request<LedgerEntityAlias[]>({ url: url('/entity-aliases'), method: 'put', data: items })
}

// ── 第二阶段：月结闭环 ─────────────────────────────────

/** pending 审批中 / locked 已锁账 / rejected 被驳回 / open 未结账 */
export type LedgerCloseStatus = 'open' | 'pending' | 'locked' | 'rejected'

export interface LedgerPeriodLock {
  shop: string
  period: string
  status: 'pending' | 'locked'
}

export interface LedgerBalanceRow {
  shop: string
  entity: string
  account: string
  period: string
  rowCount: number
  inflow: number
  outflow: number
  /** 入账 - 出账 */
  net: number
  opening: number | null
  closing: number | null
  /** 期初 + 净额 */
  expectedClosing: number | null
  /** 期末 - 应有期末 */
  diff: number | null
  prevClosing: number | null
  /** 本月期初 - 上月期末 */
  continuityDiff: number | null
  status: 'missing' | 'balanced' | 'unbalanced'
  remark: string | null
  updatedBy: string | null
  updatedAt: string | null
}

export interface LedgerBalanceInput {
  shop: string
  entity: string
  account: string
  period: string
  opening: number | null
  closing: number | null
  remark?: string | null
}

/** 余额勾稽：期初 + 入账 - 出账 = 期末 */
export function getLedgerBalancesApi(period: string, shop?: string) {
  return request<LedgerBalanceRow[]>({ url: url('/balances'), method: 'get', params: { period, shop } })
}

export function saveLedgerBalancesApi(operator: string, items: LedgerBalanceInput[]) {
  return request<boolean>({ url: url('/balances'), method: 'put', data: { operator, items } })
}

export interface LedgerBankMapping {
  date: string | null
  time: string | null
  inflow: string | null
  outflow: string | null
  amount: string | null
  direction: string | null
  counterparty: string | null
  summary: string | null
  serialNo: string | null
  accountNo: string | null
}

export interface LedgerBankImportResult {
  batchId: string
  fileName: string
  entity: string
  /** 表头所在行（1 起） */
  headerRow: number
  mapping: LedgerBankMapping
  parsedRows: number
  insertedRows: number
  skippedRows: number
  /** 无日期或无金额（合计行等）被忽略 */
  ignoredRows: number
  inflowCount: number
  inflowAmount: number
  timeFrom: string | null
  timeTo: string | null
}

/** 导入银行流水（CSV / XLS / XLSX，自动识别表头） */
export function importLedgerBankApi(file: File, entity: string, accountNo?: string) {
  const data = new FormData()
  data.append('file', file)
  data.append('entity', entity)
  if (accountNo)
    data.append('accountNo', accountNo)
  return request<LedgerBankImportResult>({ url: url('/bank/imports'), method: 'post', data, timeout: 5 * 60 * 1000 })
}

export interface LedgerBankBatch {
  batchId: string
  entity: string
  fileName: string | null
  rowCount: number
  timeFrom: string
  timeTo: string
  matched: number
  importedAt: string
}

export function getLedgerBankBatchesApi() {
  return request<LedgerBankBatch[]>({ url: url('/bank/imports'), method: 'get' })
}

export function deleteLedgerBankBatchApi(batchId: string) {
  return request<number>({ url: url('/bank/imports'), method: 'delete', params: { batchId } })
}

export interface LedgerBankTxn {
  txnId: string
  entity: string
  accountNo: string | null
  time: string
  amount: number
  counterparty: string | null
  summary: string | null
  serialNo: string | null
  platformLike: boolean
}

/** matched 已到账 / in_transit 在途 / no_bank 银行流水未覆盖 / overdue 逾期未到账 */
export type LedgerWithdrawStatus = 'matched' | 'in_transit' | 'no_bank' | 'overdue'

export interface LedgerWithdrawRow {
  shop: string
  period: string
  serialNo: string
  time: string
  amount: number
  entity: string
  remark: string | null
  status: LedgerWithdrawStatus
  method: 'auto' | 'manual' | null
  matchedBy: string | null
  txn: LedgerBankTxn | null
  lagDays: number | null
  diff: number | null
  dueDate: string
}

export interface LedgerWithdrawStats {
  count: number
  amount: number
  matched: number
  matchedAmount: number
  inTransit: number
  inTransitAmount: number
  noBank: number
  noBankAmount: number
  overdue: number
  overdueAmount: number
}

export interface LedgerWithdrawView {
  period: string
  today: string
  windowDays: number
  rows: LedgerWithdrawRow[]
  stats: LedgerWithdrawStats
  banks: { entity: string, rowCount: number, timeFrom: string | null, timeTo: string | null }[]
  /** 到账窗口内未配对、像平台打款的银行收入 */
  unmatchedBank: LedgerBankTxn[]
}

export function getLedgerWithdrawalsApi(period: string, shop?: string) {
  return request<LedgerWithdrawView>({ url: url('/withdrawals'), method: 'get', params: { period, shop } })
}

export function autoMatchWithdrawalsApi(period: string, operator: string) {
  return request<{ matched: number, unmatched: number }>({ url: url('/withdrawals/auto-match'), method: 'post', data: { period, operator } })
}

export function getWithdrawCandidatesApi(shop: string, period: string, serialNo: string) {
  return request<LedgerBankTxn[]>({ url: url('/withdrawals/candidates'), method: 'get', params: { shop, period, serialNo } })
}

/** 手工配对；txnId 为 null 表示取消配对 */
export function matchWithdrawalApi(data: { shop: string, period: string, serialNo: string, txnId: string | null, operator: string }) {
  return request<boolean>({ url: url('/withdrawals/match'), method: 'put', data })
}

export interface LedgerCloseCheck {
  key: 'coverage' | 'balance' | 'continuity' | 'withdraw' | 'previous'
  label: string
  level: 'pass' | 'warn' | 'fail'
  message: string
}

export interface LedgerCloseReport {
  checks: LedgerCloseCheck[]
  figures: {
    rowCount: number
    inflow: number
    outflow: number
    opening: number | null
    closing: number | null
    withdrawCount: number
    withdrawAmount: number
    withdrawMatched: number
  }
}

export interface LedgerCloseRecord {
  status: LedgerCloseStatus
  submittedBy: string | null
  submittedAt: string | null
  approver: string | null
  comment: string | null
  decidedBy: string | null
  decidedAt: string | null
  decisionComment: string | null
  /** 提交时的体检快照 */
  snapshot: LedgerCloseReport | null
}

/** 当前用户能做的操作（服务端按审批人 / 提交人 / 管理员判断） */
export interface LedgerCloseCan {
  submit: boolean
  decide: boolean
  cancel: boolean
  reopen: boolean
  /** 不能通过 / 驳回的原因 */
  decideBlocked?: string
}

export interface LedgerCloseShop {
  shop: string
  shopStatus: 'active' | 'closed'
  status: LedgerCloseStatus
  record: LedgerCloseRecord | null
  can: LedgerCloseCan
  report: LedgerCloseReport
}

export function getLedgerCloseApi(period: string) {
  return request<{ period: string, shops: LedgerCloseShop[] }>({ url: url('/close'), method: 'get', params: { period } })
}

export type LedgerCloseAction = 'submit' | 'approve' | 'reject' | 'cancel' | 'reopen'

export function ledgerCloseActionApi(data: {
  shop: string
  period: string
  action: LedgerCloseAction
  operator: string
  /** 审批人的用户 id（开启登录校验后必填）；未开启时填名字用 approver */
  approverId?: string
  approver?: string
  comment?: string
  acceptRisk?: boolean
}) {
  return request<boolean>({ url: url('/close'), method: 'post', data })
}

export interface LedgerCloseLog {
  shop: string
  period: string
  action: LedgerCloseAction
  operator: string | null
  comment: string | null
  at: string
}

export function getLedgerCloseLogsApi(period?: string) {
  return request<LedgerCloseLog[]>({ url: url('/close/logs'), method: 'get', params: { period } })
}

export function getLedgerLocksApi() {
  return request<LedgerPeriodLock[]>({ url: url('/close/locks'), method: 'get' })
}

// ── 第三阶段：经营分析 ─────────────────────────────────

/** 指标 key；所有指标之和 = 资金净额（net） */
export type AnalysisMetricKey = 'gmv' | 'freight' | 'subsidy' | 'refund' | 'platformFee' | 'commission' | 'otherFee' | 'other' | 'net'

export interface AnalysisMetricDef {
  key: AnalysisMetricKey
  label: string
  hint: string
  /** 与明细下钻同源的规则：明细贡献合计 = 该指标值 */
  terms: RuleTerm[]
}

export type AnalysisDimension = 'channel' | 'talent' | 'product' | 'order_month' | 'shop' | 'entity'

export interface AnalysisRow {
  /** 下钻键；"" 表示该字段为空（非订单流水、无达人订单） */
  key: string
  name: string
  rowCount: number
  /** 结算订单数 */
  orders: number
  /** 退款售后单数 */
  refundCount: number
  values: Record<AnalysisMetricKey, number>
}

export interface AnalysisBreakdown {
  period: string
  metrics: AnalysisMetricDef[]
  rows: AnalysisRow[]
  total: AnalysisRow
}

export function getAnalysisBreakdownApi(period: string, by: AnalysisDimension, shop?: string) {
  return request<AnalysisBreakdown>({ url: url('/analysis/breakdown'), method: 'get', params: { period, by, shop: shop || undefined } })
}

export interface AnalysisMatrixCell {
  orderMonth: string
  period: string
  gmv: number
  net: number
  orders: number
}

export interface AnalysisCrossPeriod {
  periods: string[]
  orderMonths: string[]
  cells: AnalysisMatrixCell[]
}

/** 下单月份 × 结算月份 */
export function getAnalysisCrossPeriodApi(shop?: string) {
  return request<AnalysisCrossPeriod>({ url: url('/analysis/cross-period'), method: 'get', params: { shop: shop || undefined } })
}

export interface AnalysisQianchuanRow {
  shop: string
  /** 千川充值 */
  recharge: number
  /** 千川实际消耗（手工录入） */
  spend: number | null
  spendRemark: string | null
  spendUpdatedBy: string | null
  /** 巨量千川渠道结算实付 */
  gmv: number
  refund: number
  net: number
  orders: number
  shopGmv: number
}

export function getAnalysisQianchuanApi(period: string) {
  return request<AnalysisQianchuanRow[]>({ url: url('/analysis/qianchuan'), method: 'get', params: { period } })
}

export function saveAdSpendApi(data: { shop: string, period: string, spend: number | null, remark?: string, operator: string }) {
  return request<boolean>({ url: url('/analysis/qianchuan'), method: 'put', data })
}
