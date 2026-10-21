import type { ParamDef } from '@/api/sql'
import type { WorkbookMeta } from '@/api/workbook'
import { LEDGER_BASE } from '@/api/ledger-base'
import { t } from '@/i18n'
import request from '@/utils/request'

/** 模板：工作簿发布为模板；使用时把模板引用的数据集换成自己的数据集，生成新工作簿 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger/templates${path}`

export type ColumnKind = 'text' | 'id' | 'money' | 'number' | 'datetime'

export interface TemplateColumn {
  title: string
  kind: ColumnKind
  description?: string
}

export interface TemplateDataset {
  /** 模板内的引用键（使用时按它提交绑定） */
  key: string
  /** 发布时的数据集名称 */
  name: string
  description?: string
  /** 引用内置数据集（如资金流水）：自动对应 */
  builtinId?: string
  columns: TemplateColumn[]
  /** 部件类型 → 个数 */
  parts: Record<string, number>
  /** 来自参数化 SQL 查询：使用时可以只填参数值（按这组值生成或复用实例数据集） */
  paramSource?: TemplateParamSource
}

export interface TemplateParamSource {
  queryId: string
  queryName: string
  params: ParamDef[]
  /** 发布时用的参数值（作为默认值） */
  values: Record<string, string>
}

export interface TemplateInfo {
  id: string
  name: string
  description: string
  category: string
  builtin: boolean
  datasets: TemplateDataset[]
  /** sheets / pivot / webexcel-chart / webexcel-kpi / webexcel-table / webexcel-slicer / formula → 个数 */
  summary: Record<string, number>
  warnings: string[]
  sheetNames: string[]
  useCount: number
  lastUsedAt: string | null
  sourceWorkbookId: string | null
  createdBy: string | null
  createdAt: string | null
  updatedAt: string | null
}

export interface TemplateBinding {
  datasetId: string
  /** 模板列名 → 数据集列名 */
  columns: Record<string, string>
}

export interface UseTemplateInput {
  name: string
  bindings: Record<string, TemplateBinding>
  /** 模板数据集 key → 参数值（代替 bindings） */
  params?: Record<string, Record<string, string>>
  clearSlicers: boolean
  operator?: string
}

export interface PublishTemplateInput {
  workbookId: string
  name: string
  description: string
  category: string
  /** 覆盖已有的自建模板 */
  replaceId?: string | null
  operator?: string
}

export interface TemplateUse {
  workbookId: string
  workbookName: string | null
  usedBy: string | null
  usedAt: string
}

export const PART_LABEL: Record<string, string> = {
  'pivot': t('透视表'),
  'webexcel-chart': t('图表'),
  'webexcel-kpi': t('指标卡'),
  'webexcel-table': t('明细卡'),
  'webexcel-slicer': t('切片器'),
  'webexcel-datatable': t('数据表'),
  'formula': t('公式'),
}

export const KIND_LABEL: Record<ColumnKind, string> = {
  text: t('文本'),
  id: t('编号'),
  money: t('金额'),
  number: t('数值'),
  datetime: t('日期时间'),
}

export function listTemplatesApi() {
  return request<TemplateInfo[]>({ url: url(''), method: 'get' })
}

export function publishTemplateApi(data: PublishTemplateInput) {
  return request<TemplateInfo>({ url: url(''), method: 'post', data })
}

export function updateTemplateApi(id: string, data: { name: string, description: string, category: string }) {
  return request<boolean>({ url: url(`/${id}`), method: 'put', data })
}

export function deleteTemplateApi(id: string) {
  return request<boolean>({ url: url(`/${id}`), method: 'delete' })
}

export function useTemplateApi(id: string, data: UseTemplateInput) {
  return request<{ workbook: WorkbookMeta, warnings: string[], notes?: string[] }>({ url: url(`/${id}/use`), method: 'post', data })
}

export function templateUsesApi(id: string) {
  return request<TemplateUse[]>({ url: url(`/${id}/uses`), method: 'get' })
}
