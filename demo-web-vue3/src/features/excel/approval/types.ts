/** 审批状态 */
export type ApprovalStatus = 'pending' | 'approved' | 'rejected'

/** 审批历史记录条目 */
export interface ApprovalHistoryEntry {
  action: string
  user: string
  comment: string
  time: Date
}

/** 文件级审批记录 */
export interface ApprovalRecord {
  id: number
  fileName: string
  dataRows: number
  totalAmount: number
  orderCount: number
  statusBreakdown: Record<string, number>
  initiator: string
  approver: string
  comment: string
  status: ApprovalStatus
  timestamp: Date
  history: ApprovalHistoryEntry[]
  /** Univer 工作簿快照 (序列化 JSON) */
  snapshot?: unknown
}

/** 审批人选项 */
export interface Approver {
  name: string
  title: string
}
