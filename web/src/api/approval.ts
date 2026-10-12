import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/**
 * 文件审批（服务端）：把工作簿的某个已保存版本交给指定审批人，通过 / 驳回 / 撤回，全程留痕。
 * 审批人从登录过的用户里选；只有审批人（或管理员代为处理）能审批，发起人不能审批自己的单据
 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger${path}`

export type ApprovalStatus = 'pending' | 'approved' | 'rejected' | 'cancelled'

export interface ApprovalEvent {
  at: string
  /** submit 发起 / approve 通过 / reject 驳回 / cancel 撤回 */
  action: 'submit' | 'approve' | 'reject' | 'cancel'
  userName: string
  comment: string
}

export interface Approval {
  id: string
  workbookId: string
  workbookName: string
  /** 审批的版本 */
  workbookVersion: number
  /** 工作簿现在的版本（大于审批版本说明之后又修改过；工作簿已删除时为空） */
  currentVersion: number | null
  initiatorId: string
  initiatorName: string
  approverId: string
  approverName: string
  comment: string
  /** 发起时由页面统计的概况，仅供展示 */
  summary: { fileName?: string, dataRows?: number, orderCount?: number, totalAmount?: number } | null
  status: ApprovalStatus
  createdAt: string
  decidedAt: string | null
  decidedBy: string | null
  decisionComment: string | null
  canDecide: boolean
  canCancel: boolean
  /** 仅详情接口返回 */
  events?: ApprovalEvent[]
}

export interface LedgerUserItem {
  id: string
  name: string
}

/** 登录过的用户（选审批人） */
export function listLedgerUsersApi(keyword = '') {
  return request<LedgerUserItem[]>({ url: url('/users'), method: 'get', params: { keyword } })
}

/** todo 待我审批 / started 我发起的 / done 已处理的 / all 全部（仅管理员） */
export type ApprovalBox = 'todo' | 'started' | 'done' | 'all'

export function listApprovalsApi(box: ApprovalBox) {
  return request<Approval[]>({ url: url('/approvals'), method: 'get', params: { box } })
}

export function approvalCountApi() {
  return request<{ todo: number }>({ url: url('/approvals/count'), method: 'get' })
}

export function getApprovalApi(id: string) {
  return request<Approval>({ url: url(`/approvals/${id}`), method: 'get' })
}

export function createApprovalApi(data: {
  workbookId: string
  approverId?: string
  approverName?: string
  comment?: string
  summary?: Approval['summary']
  operator?: string
}) {
  return request<Approval>({ url: url('/approvals'), method: 'post', data })
}

export function decideApprovalApi(id: string, approve: boolean, comment: string, operator?: string) {
  return request<Approval>({ url: url(`/approvals/${id}/decide`), method: 'post', data: { approve, comment, operator } })
}

export function cancelApprovalApi(id: string, comment: string, operator?: string) {
  return request<Approval>({ url: url(`/approvals/${id}/cancel`), method: 'post', data: { comment, operator } })
}
