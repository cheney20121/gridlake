import type { CommentThread, Inbox } from '@/api/comment'
import { ref, shallowRef } from 'vue'
import { inboxApi, listCommentsApi } from '@/api/comment'

/** 当前工作簿的评论线程（面板、格子旁的提示共用）与我的通知 */
export const commentThreads = shallowRef<CommentThread[]>([])
export const inbox = ref<Inbox>({ items: [], unread: 0 })

let loadedFor: string | null = null

export async function reloadComments(workbookId: string | null | undefined) {
  if (!workbookId) {
    commentThreads.value = []
    loadedFor = null
    return
  }
  try {
    commentThreads.value = await listCommentsApi(workbookId)
    loadedFor = workbookId
  }
  catch {
    // 没有权限或网络问题：当作没有评论，请求自己会提示
    if (loadedFor !== workbookId)
      commentThreads.value = []
  }
}

/** 某个格子上的线程（未解决的在前） */
export function threadsAt(sheetId: string, row: number, col: number) {
  return commentThreads.value
    .filter(t => t.sheetId === sheetId && t.row === row && t.col === col)
    .sort((a, b) => Number(a.resolved) - Number(b.resolved))
}

export async function refreshInbox() {
  try {
    inbox.value = await inboxApi()
  }
  catch {}
}

/** 要在评论面板里定位到的格子（点铃铛里的通知、格子旁的评论按钮）；nonce 保证同一个格子重复点击也会触发 */
export const commentFocus = ref<{ sheetId: string, row: number, col: number, nonce: number } | null>(null)

export function focusComment(sheetId: string, row: number, col: number) {
  commentFocus.value = { sheetId, row, col, nonce: Date.now() }
}
