/** 工作簿管理对话框里的文件夹 / 视图模型 */

/** 根目录在树里的键 */
export const ROOT = '__root__'

export type View = { kind: 'all' } | { kind: 'starred' } | { kind: 'folder', id: string } | { kind: 'trash' }

export interface TreeNode {
  id: string
  label: string
  count: number
  children: TreeNode[]
}
