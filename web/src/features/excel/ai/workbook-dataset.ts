import type { Applied } from './workbook'
import type { DatasetUsage } from '@/api/dataset'
import { deleteDatasetApi, getDatasetUsageApi } from '@/api/dataset'
import { createQueryApi, deleteQueryApi } from '@/api/sql'
import { t } from '@/i18n'

/**
 * AI 把一条 SQL 的结果另存为数据集：走 SQL 面板同一个接口（保存查询 + 立即物化为数据集），权限、行数上限、列名规则都与手动保存一致。
 * 撤销：先删掉另存时生成的查询（数据集保留），再把数据集移入回收站（数据保留，可恢复）；
 * 数据集已被别处引用（工作簿、模板、视图、推送、别的查询…）或有人填报过时不动它，免得弄坏别人的东西
 */

/** save_query_as_dataset 建议的内容（见服务端 tools.rs） */
export interface DatasetSavePayload {
  name: string
  sql: string
  purpose?: string
  rows?: number
}

export interface DatasetSaveUndo {
  kind: 'dataset_save'
  queryId: string
  datasetId: string
  name: string
}

export async function applyDatasetSave(p: DatasetSavePayload, onDatasetsChanged: () => void, operator?: string): Promise<Applied> {
  const q = await createQueryApi({
    name: p.name,
    sql: p.sql,
    description: p.purpose || t('由 AI 助手生成'),
    saveAsDataset: p.name,
    schedule: { kind: 'manual' },
    operator,
  })
  if (!q.datasetId)
    throw new Error(t('数据集没有创建成功'))
  onDatasetsChanged()
  return {
    message: t('已另存为数据集「{0}」{1}', [p.name, q.lastRows != null ? t('（{0} 行）', [q.lastRows.toLocaleString()]) : '']),
    undo: { kind: 'dataset_save', queryId: q.id, datasetId: q.datasetId, name: p.name },
  }
}

/** 数据集被别处用到的情况（不算我们另存时生成的那条查询）；没有则返回空 */
export function otherUses(u: DatasetUsage, queryId: string): string[] {
  const parts: [number, string][] = [
    [u.workbooks.length, t('个工作簿')],
    [u.templates.length, t('个模板')],
    [u.views.length, t('个关联视图')],
    [u.sqlQueries.filter(x => x.id !== queryId).length, t('条 SQL 查询')],
    [u.connectors.length, t('个数据连接')],
    [u.reports.length, t('个定时推送')],
    [u.writebacks, t('次填报')],
  ]
  return parts.filter(([n]) => n > 0).map(([n, unit]) => `${n} ${unit}`)
}

export async function undoDatasetSave(undo: DatasetSaveUndo, onDatasetsChanged: () => void, operator?: string): Promise<string> {
  let usage: DatasetUsage
  try {
    usage = await getDatasetUsageApi(undo.datasetId)
  }
  catch {
    return t('数据集「{0}」已不存在（或已在回收站）', [undo.name])
  }
  const uses = otherUses(usage, undo.queryId)
  if (uses.length)
    return t('数据集「{0}」已被 {1}用到，保留不动；确实不要了请在数据集管理里删除', [undo.name, uses.join('、')])
  await deleteQueryApi(undo.queryId)
  await deleteDatasetApi(undo.datasetId, operator ?? '')
  onDatasetsChanged()
  return t('已删除数据集「{0}」（移入回收站，可恢复）', [undo.name])
}
