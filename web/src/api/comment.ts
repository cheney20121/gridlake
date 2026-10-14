import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/** 单元格评论与 @提醒：评论线程挂在某个工作表的格子上，@ 到的人在顶栏铃铛里收到通知 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger${path}`

export interface CommentMention {
  id: string
  name: string
}

export interface CommentItem {
  id: string
  workbookId: string
  sheetId: string
  row: number
  col: number
  parentId: string | null
  body: string
  mentions: CommentMention[]
  authorId: string | null
  authorName: string
  createdAt: string
  editedAt: string | null
  resolved: boolean
  resolvedBy: string | null
}

/** 一条线程：根评论 + 回复 */
export interface CommentThread extends CommentItem {
  replies: CommentItem[]
}

export interface CommentAdded {
  comment: CommentItem
  /** 没有通知到的人（没有这个工作簿的查看权限） */
  notNotified: string[]
}

export interface NotificationItem {
  id: string
  /** mention：被 @；reply：你的评论有了回复；approval_*：文件审批；close_*：月结；collect_*：多人填报被退回 / 被确认 */
  kind: 'mention' | 'reply' | 'approval_request' | 'approval_approved' | 'approval_rejected' | 'approval_cancelled'
    | 'close_request' | 'close_locked' | 'close_rejected' | 'close_cancelled' | 'close_reopened'
    | 'collect_returned' | 'collect_confirmed' | 'collect_remind'
  workbookId: string
  workbookName: string
  sheetId: string
  row: number
  col: number
  commentId: string
  fromName: string
  text: string
  createdAt: string
  read: boolean
  /** 审批单 id，或月结的 `close:店铺|月份`（评论通知为空） */
  refId: string
}

export interface Inbox {
  items: NotificationItem[]
  unread: number
}

export function listCommentsApi(workbookId: string) {
  return request<CommentThread[]>({ url: url(`/workbooks/${workbookId}/comments`), method: 'get' })
}

export function addCommentApi(workbookId: string, data: { sheetId?: string, row?: number, col?: number, parentId?: string, body: string, mentions: string[], operator?: string }) {
  return request<CommentAdded>({ url: url(`/workbooks/${workbookId}/comments`), method: 'post', data })
}

export function editCommentApi(id: string, body: string, mentions: string[]) {
  return request<CommentAdded>({ url: url(`/comments/${id}`), method: 'put', data: { body, mentions } })
}

export function deleteCommentApi(id: string) {
  return request<number>({ url: url(`/comments/${id}`), method: 'delete' })
}

export function resolveCommentApi(id: string, resolved: boolean) {
  return request<CommentItem>({ url: url(`/comments/${id}/resolve`), method: 'post', data: { resolved } })
}

/** 我的通知（轮询用，静默：失败不弹提示） */
export function inboxApi() {
  return request<Inbox>({ url: url('/notifications'), method: 'get', silent: true })
}

/** 标记已读；不传 ids 表示全部 */
export function readNotificationsApi(ids?: string[]) {
  return request<boolean>({ url: url('/notifications/read'), method: 'post', data: { ids: ids ?? [] } })
}
