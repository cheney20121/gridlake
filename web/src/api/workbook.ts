import type { AxiosResponse } from 'axios'
import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/**
 * 服务端工作簿（gridlake）
 * 内容上传前在浏览器端 gzip 压缩（约 10 倍）；读取时服务端原样返回 gzip，由浏览器按 Content-Encoding 解压。
 * 与 ledger.ts 相同，使用绝对地址绕开 request.ts 改写 baseURL，复用其信封解析与错误提示
 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger/workbooks${path}`

export interface WorkbookMeta {
  id: string
  name: string
  version: number
  /** 解压后的 JSON 字节数 */
  size: number
  /** 压缩后存储字节数 */
  storedSize: number
  createdBy: string | null
  createdAt: string
  updatedBy: string | null
  updatedAt: string
  /** 所在文件夹（空为根目录） */
  folderId: string | null
  starred: boolean
  /** 所有者（登录校验启用前创建的没有所有者，只有管理员能管理） */
  ownerId: string | null
  ownerName: string | null
  /** 共享范围：private 仅自己 / view 所有人可查看 / edit 所有人可编辑 */
  visibility: 'private' | 'view' | 'edit'
}

export interface WorkbookFolder {
  id: string
  name: string
  parentId: string | null
  createdBy: string | null
  createdAt: string
}

/** 回收站里的工作簿 */
export interface TrashedWorkbook {
  id: string
  name: string
  size: number
  version: number
  /** 删除前所在文件夹（文件夹已删除时为空） */
  folderName: string | null
  deletedAt: string
  deletedBy: string | null
  /** 到这个时间自动彻底删除（不自动清理时为空） */
  purgeAt: string | null
}

export interface SaveWorkbookResult {
  saved: boolean
  version: number
  /** 冲突：基于的版本已被别人更新（未覆盖） */
  conflict: WorkbookMeta | null
  meta: WorkbookMeta | null
}

export interface WorkbookVersion {
  version: number
  size: number
  savedBy: string | null
  savedAt: string
  description: string | null
  isAuto: boolean
}

/** JSON 文本 → gzip 字节 */
export async function gzipJson(json: string): Promise<Blob> {
  const stream = new Blob([json]).stream().pipeThrough(new CompressionStream('gzip'))
  return new Response(stream).blob()
}

export function listWorkbooksApi() {
  return request<WorkbookMeta[]>({ url: url(''), method: 'get' })
}

/** folderId：新建到这个文件夹（空为根目录） */
export async function createWorkbookApi(name: string, json: string, operator: string, folderId?: string | null) {
  return request<WorkbookMeta>({
    url: url(''),
    method: 'post',
    params: { name, operator, folder: folderId || undefined },
    data: await gzipJson(json),
    headers: { 'Content-Type': 'application/octet-stream' },
    timeout: 10 * 60 * 1000,
  })
}

/** 读取工作簿内容：返回 JSON 文本与版本号 */
async function readContent(path: string): Promise<{ json: string, version: number }> {
  const res = await request<AxiosResponse<Blob>>({
    url: url(path),
    method: 'get',
    responseType: 'blob',
    type: 'download',
    timeout: 10 * 60 * 1000,
  })
  return { json: await res.data.text(), version: Number(res.headers['x-workbook-version'] ?? 0) }
}

export function getWorkbookContentApi(id: string) {
  return readContent(`/${id}/content`)
}

export function getWorkbookVersionContentApi(id: string, version: number) {
  return readContent(`/${id}/versions/${version}/content`)
}

export function getWorkbookMetaApi(id: string) {
  return request<WorkbookMeta>({ url: url(`/${id}`), method: 'get' })
}

export async function saveWorkbookApi(id: string, json: string, params: { baseVersion: number, force?: boolean, auto?: boolean, description?: string, operator: string }, collabSeq?: number) {
  return request<SaveWorkbookResult>({
    url: url(`/${id}/content`),
    method: 'put',
    params,
    data: await gzipJson(json),
    // 协作中保存：快照已包含到哪个序号，服务端据此截断变更日志（见 server/src/collab.rs）
    headers: { 'Content-Type': 'application/octet-stream', ...(collabSeq != null ? { 'x-collab-seq': String(collabSeq) } : {}) },
    timeout: 10 * 60 * 1000,
  })
}

export function renameWorkbookApi(id: string, name: string) {
  return request<WorkbookMeta>({ url: url(`/${id}`), method: 'put', data: { name } })
}

/** 移到回收站（可恢复） */
export function deleteWorkbookApi(id: string, operator: string) {
  return request<boolean>({ url: url(`/${id}`), method: 'delete', params: { operator } })
}

/** 回收站：days 为保留天数（0 表示不自动清理） */
export function listTrashApi() {
  return request<{ days: number, items: TrashedWorkbook[] }>({ url: url('/trash'), method: 'get' })
}

export function restoreFromTrashApi(id: string) {
  return request<WorkbookMeta>({ url: url(`/${id}/untrash`), method: 'post' })
}

/** 彻底删除（只能删回收站里的） */
export function purgeWorkbookApi(id: string) {
  return request<boolean>({ url: url(`/${id}/purge`), method: 'delete' })
}

export function emptyTrashApi() {
  return request<number>({ url: url('/trash'), method: 'delete' })
}

/** 修改共享范围（所有者与管理员） */
export function shareWorkbookApi(id: string, visibility: WorkbookMeta['visibility']) {
  return request<WorkbookMeta>({ url: url(`/${id}/share`), method: 'put', data: { visibility } })
}

export function starWorkbookApi(id: string, starred: boolean) {
  return request<boolean>({ url: url(`/${id}/star`), method: 'put', data: { starred } })
}

/** 移到文件夹（folderId 为空表示根目录） */
export function moveWorkbooksApi(ids: string[], folderId: string | null) {
  return request<number>({ url: url('/move'), method: 'post', data: { ids, folderId } })
}

const folderUrl = (path: string) => `${LEDGER_BASE}/api/ledger/folders${path}`

export function listFoldersApi() {
  return request<WorkbookFolder[]>({ url: folderUrl(''), method: 'get' })
}

export function createFolderApi(name: string, parentId: string | null, operator: string) {
  return request<WorkbookFolder>({ url: folderUrl(''), method: 'post', data: { name, parentId, operator } })
}

/** 改名 / 移动 */
export function updateFolderApi(id: string, name: string, parentId: string | null) {
  return request<boolean>({ url: folderUrl(`/${id}`), method: 'put', data: { name, parentId } })
}

/** 删除文件夹（含子文件夹），其中的工作簿移到回收站；返回移入回收站的个数 */
export function deleteFolderApi(id: string, operator: string) {
  return request<number>({ url: folderUrl(`/${id}`), method: 'delete', params: { operator } })
}

export function duplicateWorkbookApi(id: string, name: string, operator: string) {
  return request<WorkbookMeta>({ url: url(`/${id}/duplicate`), method: 'post', data: { name, operator } })
}

export function listWorkbookVersionsApi(id: string) {
  return request<WorkbookVersion[]>({ url: url(`/${id}/versions`), method: 'get' })
}

export function restoreWorkbookVersionApi(id: string, version: number, operator: string) {
  return request<WorkbookMeta>({ url: url(`/${id}/restore`), method: 'post', data: { version, operator } })
}
