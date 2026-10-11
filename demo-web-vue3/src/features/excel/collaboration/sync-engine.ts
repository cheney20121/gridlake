/**
 * 本机的协作身份（没有登录校验时的显示名与颜色）。
 * 真正的多人实时协作见 session.ts（服务端中转）；这里原先用 BroadcastChannel 只在同一个浏览器的标签页之间同步光标，已不再使用
 */

/** 协作者信息（给界面显示） */
export interface Collaborator {
  id: string
  name: string
  color: string
  cursor?: { row: number, col: number, sheetId?: string }
  lastActive: number
}

const COLLAB_COLORS = ['#f97316', '#8b5cf6', '#06b6d4', '#10b981', '#f59e0b', '#ef4444']

export class CollaborationSync {
  private userId: string
  private userName: string
  private userColor: string

  constructor(userId?: string, userName?: string) {
    this.userId = userId || `user_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`
    this.userName = userName || `用户${Math.floor(Math.random() * 100)}`
    this.userColor = COLLAB_COLORS[Math.floor(Math.random() * COLLAB_COLORS.length)] ?? '#107C41'
  }

  getUserId(): string {
    return this.userId
  }

  getUserName(): string {
    return this.userName
  }

  getUserColor(): string {
    return this.userColor
  }
}

/** 单例 */
export const collaborationSync = new CollaborationSync()
