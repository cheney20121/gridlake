import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/**
 * 逐笔对账：两个数据集（如银行流水与企业账）逐行勾对。对账方案本身是一种数据集（kind = recon），
 * 带「状态 / 勾对组 / 勾对完成日」等列，可直接用于公式、透视与图表；与 dataset.ts 相同，使用绝对地址复用 request.ts 的信封解析
 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger/recons${path}`

export interface ReconSide {
  /** 数据集（id 或名称） */
  dataset: string
  /** 结果里显示的「来源」，如「银行」「账面」 */
  label: string
  amount: string
  /** 1 原样；-1 取反（让两侧同一笔业务的金额符号一致） */
  sign: 1 | -1
  date?: string | null
  /** 对账键：两侧按顺序一一对应 */
  keys: string[]
  memo?: string | null
}

export interface ReconDef {
  left: ReconSide
  right: ReconSide
  /** 金额容差（元） */
  tolerance: number
  /** 日期窗口（天）；为空不看日期 */
  days?: number | null
  /** 第三轮：同一对账键下两侧合计相等的多笔也自动勾对 */
  groupByKey: boolean
}

export interface ReconSideStats {
  label: string
  dataset: string
  rows: number
  total: number
  undated: number
  minDate: string | null
  maxDate: string | null
  blankKeys: number
}

export interface ReconPreview {
  def: ReconDef
  left: ReconSideStats
  right: ReconSideStats
  warnings: string[]
}

export interface ReconInfo {
  id: string
  name: string
  def: ReconDef
  groups: number
}

export interface Bucket {
  rows: number
  total: number
}

export interface ReconSideSummary {
  label: string
  all: Bucket
  matched: Bucket
  /** 截至日的未达账项：增加（金额为正）/ 减少（为负） */
  openIn: Bucket
  openOut: Bucket
  openZero: number
  undated: number
  minDate: string | null
  maxDate: string | null
}

export interface ReconSummary {
  asOf: string | null
  left: ReconSideSummary
  right: ReconSideSummary
  groups: {
    total: number
    exact: number
    tolerance: number
    sum: number
    manual: number
    withDiff: number
    diffTotal: number
    broken: number
  }
}

export interface ReconItem {
  /** 行标识：勾对时提交 */
  key: string
  date: string | null
  amount: number
  corr: string | null
  memo: string | null
  gno: number | null
  method: string | null
  /** 原勾对的行已变动 */
  stale: boolean
}

export interface ReconItemQuery {
  side: 'left' | 'right'
  status?: 'open' | 'matched' | 'all'
  q?: string
  /** 只看和它相差不超过容差的金额 */
  amount?: number
  from?: string
  to?: string
  offset?: number
  limit?: number
}

export interface ReconGroupItem {
  side: 0 | 1
  date: string | null
  amount: number
  corr: string | null
  memo: string | null
  /** 这一行现在已经找不到了（被改过或删了） */
  missing: boolean
}

export type ReconMethod = 'exact' | 'tolerance' | 'sum' | 'manual'

export interface ReconGroup {
  gno: number
  method: ReconMethod
  note: string | null
  diff: number
  by: string | null
  at: string | null
  broken: boolean
  items: ReconGroupItem[]
}

export interface ReconGroupQuery {
  method?: ReconMethod
  withDiff?: boolean
  broken?: boolean
  q?: string
  offset?: number
  limit?: number
}

export interface ReconAutoResult {
  dryRun: boolean
  exact: number
  tolerance: number
  sum: number
  groups: number
  matchedRows: number
  clearedBroken: number
  leftOpen: number
  rightOpen: number
  warnings: string[]
}

export interface ReconManualResult {
  gno: number
  leftTotal: number
  rightTotal: number
  diff: number
}

const LONG = 5 * 60 * 1000

export function previewReconApi(def: ReconDef) {
  return request<ReconPreview>({ url: url('/preview'), method: 'post', data: { def }, timeout: LONG })
}

export function createReconApi(name: string, def: ReconDef) {
  return request<string>({ url: url(''), method: 'post', data: { name, def } })
}

export function getReconApi(id: string) {
  return request<ReconInfo>({ url: url(`/${id}`), method: 'get' })
}

/** 换了数据集 / 金额列 / 金额方向且已有勾对时，需要 resetMatches 确认清空 */
export function updateReconApi(id: string, name: string, def: ReconDef, resetMatches = false) {
  return request<boolean>({ url: url(`/${id}`), method: 'put', data: { name, def, resetMatches } })
}

export function getReconSummaryApi(id: string, asOf?: string, signal?: AbortSignal) {
  return request<ReconSummary>({ url: url(`/${id}/summary`), method: 'get', params: { asOf: asOf || undefined }, signal, timeout: LONG })
}

export function getReconItemsApi(id: string, q: ReconItemQuery, signal?: AbortSignal) {
  return request<{ total: number, sum: number, rows: ReconItem[] }>({ url: url(`/${id}/items`), method: 'get', params: q, signal, timeout: LONG })
}

export function getReconGroupsApi(id: string, q: ReconGroupQuery, signal?: AbortSignal) {
  return request<{ total: number, groups: ReconGroup[] }>({ url: url(`/${id}/groups`), method: 'get', params: q, signal, timeout: LONG })
}

/** dryRun：只试算，不保存 */
export function autoMatchReconApi(id: string, dryRun: boolean) {
  return request<ReconAutoResult>({ url: url(`/${id}/auto`), method: 'post', data: { dryRun }, timeout: LONG })
}

export function matchReconApi(id: string, data: { left: string[], right: string[], note?: string, force?: boolean }) {
  return request<ReconManualResult>({ url: url(`/${id}/match`), method: 'post', data })
}

/** groups：指定勾对组；mode：auto 全部自动勾对的 / all 全部 */
export function unmatchReconApi(id: string, data: { groups?: number[], mode?: 'auto' | 'all' }) {
  return request<{ groups: number, rows: number }>({ url: url(`/${id}/unmatch`), method: 'post', data })
}
