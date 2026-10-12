import type { FUniver } from '@univerjs/core/facade'
import type { DatasetSavePayload, DatasetSaveUndo } from './workbook-dataset'
import type { ColumnFormatPayload, ColumnFormatUndo, TableCreatePayload, TableCreateUndo, TableViewPayload, TableViewUndo } from './workbook-table'
import type { AiAction, AiContext } from '@/api/ai'
import type { DatasetColumnKind } from '@/api/dataset'
import type { ChartSpec } from '@/features/excel/chart/types'
import type { PivotSpec } from '@/features/excel/pivot/pivot-sheet'
import { addCalcApi, removeCalcApi } from '@/api/dataset'
import { colName } from '@/features/excel/chart/chart-defaults'
import { insertChart, newChartId, removeChart } from '@/features/excel/chart/chart-manager'
import { createPivotSheet, getPivotSpec, refreshPivot } from '@/features/excel/pivot/pivot-sheet'
import { t } from '@/i18n'
import { applyDatasetSave, undoDatasetSave } from './workbook-dataset'
import { applyColumnFormat, applyTableCreate, applyTableView, collectTableContext, undoColumnFormat, undoTableCreate, undoTableView } from './workbook-table'

/**
 * AI 助手与工作簿的连接：
 * - 提问时附带界面上下文（当前工作表、选区内容、透视表定义、工作簿里的数据表）
 * - 用户确认后执行 AI 的建议（新建透视表、插入图表、写公式、加计算列、调整数据表视图），或一张方案卡里的多个步骤（全部成功或全部撤回）
 * - 应用时记下撤销信息（新建的页签 / 图表 / 被覆盖的单元格原值 / 新建的计算列 / 数据表原来的视图），之后可以一次撤销
 */

const MAX_ROWS = 20
const MAX_COLS = 10

export function collectContext(api: FUniver | null | undefined): AiContext {
  const sheet = api?.getActiveWorkbook()?.getActiveSheet()
  if (!sheet)
    return {}
  const ctx: AiContext = { sheetName: sheet.getSheetName() }
  const tables = collectTableContext(api)
  if (tables.length)
    ctx.tables = tables
  const pivot = getPivotSpec(sheet)
  if (pivot)
    ctx.pivot = { datasetName: pivot.datasetName, request: pivot.request }
  const sel = sheet.getSelection()?.getActiveRange()
  if (sel) {
    const r0 = sel.getRow()
    const c0 = sel.getColumn()
    const r1 = sel.getLastRow()
    const c1 = sel.getLastColumn()
    ctx.selection = r0 === r1 && c0 === c1 ? `${colName(c0)}${r0 + 1}` : `${colName(c0)}${r0 + 1}:${colName(c1)}${r1 + 1}`
    const rows = Math.min(r1 - r0 + 1, MAX_ROWS)
    const cols = Math.min(c1 - c0 + 1, MAX_COLS)
    const values = sheet.getRange(r0, c0, rows, cols).getValues() as unknown[][]
    // 只有一个空白单元格时不带内容
    if (values.some(r => r.some(v => v != null && v !== '')))
      ctx.selectionValues = values
  }
  return ctx
}

function uniqueSheetName(api: FUniver, base: string) {
  const taken = new Set(api.getActiveWorkbook()?.getSheets().map(s => s.getSheetName()) ?? [])
  const clean = base.replace(/[\\/?*[\]:]/g, '').slice(0, 28) || '透视表'
  if (!taken.has(clean))
    return clean
  let n = 2
  while (taken.has(`${clean}${n}`))
    n++
  return `${clean}${n}`
}

/** 应用一条建议时记下的撤销信息（随建议存在服务端，刷新页面后仍能撤销） */
export type UndoStep
  = | { kind: 'pivot', sheetId: string, name: string }
    | { kind: 'chart', chartId: string, sheetId: string, title: string }
    | { kind: 'formula', sheetId: string, cell: string, written: string, oldFormula: string | null, oldValue: unknown }
    | { kind: 'calc', datasetId: string, name: string }
    | TableViewUndo
    | ColumnFormatUndo
    | TableCreateUndo
    | DatasetSaveUndo
    | { kind: 'plan', steps: UndoStep[] }

export interface Applied {
  /** 结果说明（记入审计） */
  message: string
  undo: UndoStep
}

const KIND_NAME: Record<string, string> = { pivot: '透视表', chart: '图表', formula: '公式', calc: '计算列', table_view: '数据表视图', column_format: '列样式', table_create: '新建数据表', dataset_save: '另存数据集' }

async function applyOne(api: FUniver, kind: string, p: Record<string, any>, onDatasetsChanged: () => void, operator?: string): Promise<Applied> {
  switch (kind) {
    case 'pivot': {
      const spec: PivotSpec = { version: 1, datasetId: p.datasetId, datasetName: p.datasetName, request: p.request }
      const sheet = await createPivotSheet(api, spec)
      const name = uniqueSheetName(api, p.title)
      sheet.setName(name)
      await refreshPivot(sheet, spec)
      return { message: t('已新建透视表页签「{0}」', [name]), undo: { kind: 'pivot', sheetId: sheet.getSheetId(), name } }
    }
    case 'chart': {
      const sheet = api.getActiveWorkbook()?.getActiveSheet()
      if (!sheet)
        throw new Error(t('没有打开的工作表'))
      const spec = { ...p.spec, id: newChartId(), v: 1 } as ChartSpec
      insertChart(api, spec, sheet)
      return { message: t('已在「{0}」插入图表「{1}」', [sheet.getSheetName(), spec.title]), undo: { kind: 'chart', chartId: spec.id, sheetId: sheet.getSheetId(), title: spec.title } }
    }
    case 'formula': {
      const sheet = api.getActiveWorkbook()?.getActiveSheet()
      if (!sheet)
        throw new Error(t('没有打开的工作表'))
      const range = sheet.getRange(p.cell as string)
      const oldFormula = range.getFormula() || null
      const oldValue = oldFormula ? null : range.getValue()
      if (oldFormula || (oldValue != null && oldValue !== '')) {
        const ok = await ElMessageBox.confirm(t('单元格 {0} 已有内容「{1}」，覆盖为 AI 建议的公式？', [p.cell, String(oldFormula ?? oldValue).slice(0, 40)]), t('覆盖单元格'), { type: 'warning', confirmButtonText: t('覆盖') }).then(() => true, () => false)
        if (!ok)
          throw new Error(t('已取消'))
      }
      range.setValue(p.formula)
      sheet.getRange(p.cell as string).activate()
      return { message: t('已在「{0}」{1} 写入 {2}', [sheet.getSheetName(), p.cell, p.formula]), undo: { kind: 'formula', sheetId: sheet.getSheetId(), cell: p.cell, written: p.formula, oldFormula, oldValue } }
    }
    case 'calc': {
      await addCalcApi(p.datasetId, p.name, p.formula, (p.columnKind || null) as DatasetColumnKind | null)
      onDatasetsChanged()
      return { message: t('已给「{0}」添加计算列「{1}」', [p.datasetName, p.name]), undo: { kind: 'calc', datasetId: p.datasetId, name: p.name } }
    }
    case 'table_view':
      return applyTableView(api, p as TableViewPayload)
    case 'column_format':
      return applyColumnFormat(api, p as ColumnFormatPayload)
    case 'table_create':
      return applyTableCreate(api, p as TableCreatePayload)
    case 'dataset_save':
      return applyDatasetSave(p as DatasetSavePayload, onDatasetsChanged, operator)
  }
  throw new Error(t('不支持的建议类型：{0}', [kind]))
}

/** 执行建议或方案卡；方案里任何一步失败（或用户取消覆盖）都会撤回已完成的步骤，工作簿回到应用前的样子 */
export async function applyAction(api: FUniver, action: AiAction, onDatasetsChanged: () => void, operator?: string): Promise<Applied> {
  if (action.kind !== 'plan')
    return applyOne(api, action.kind, action.payload, onDatasetsChanged, operator)
  const steps: Record<string, any>[] = action.payload.steps ?? []
  const done: UndoStep[] = []
  const messages: string[] = []
  // 本方案新建的数据表：请求的表名（小写）→ 实际用的表名（重名时会自动加序号），后面的步骤按请求的表名引用它
  const created = new Map<string, string>()
  for (const [i, step] of steps.entries()) {
    try {
      const actual = typeof step.table === 'string' ? created.get(step.table.trim().toLowerCase()) : undefined
      const r = await applyOne(api, step.kind, actual ? { ...step, table: actual } : step, onDatasetsChanged, operator)
      if (step.kind === 'table_create' && r.undo.kind === 'table_create')
        created.set((step.name?.trim() || `${step.datasetName}表`).toLowerCase(), r.undo.name)
      done.push(r.undo)
      messages.push(r.message)
    }
    catch (e) {
      const reason = e instanceof Error ? e.message : String(e)
      await undoApplied(api, { kind: 'plan', steps: done }, onDatasetsChanged, operator)
      if (reason === '已取消')
        throw new Error(t('已取消'))
      throw new Error(`第 ${i + 1} 步（${KIND_NAME[step.kind] ?? step.kind}）失败：${reason}${done.length ? `；已撤回前面 ${done.length} 步` : ''}`)
    }
  }
  return { message: t('已应用方案「{0}」（{1} 步）：{2}', [action.payload.title, steps.length, messages.join('；')]), undo: { kind: 'plan', steps: done } }
}

function sheetById(api: FUniver, id: string) {
  return api.getActiveWorkbook()?.getSheets().find(s => s.getSheetId() === id) ?? null
}

/** 撤销应用；返回说明。对象已被用户改动 / 删除时保留或跳过，并在说明里写明 */
export async function undoApplied(api: FUniver, undo: UndoStep, onDatasetsChanged: () => void, operator?: string): Promise<string> {
  switch (undo.kind) {
    case 'plan': {
      const notes: string[] = []
      for (const step of [...undo.steps].reverse())
        notes.push(await undoApplied(api, step, onDatasetsChanged, operator))
      return notes.join('；')
    }
    case 'pivot': {
      const sheet = sheetById(api, undo.sheetId)
      if (!sheet)
        return `透视表页签「${undo.name}」已不存在`
      api.getActiveWorkbook()?.deleteSheet(sheet)
      return `已删除透视表页签「${undo.name}」`
    }
    case 'chart': {
      removeChart(api, undo.chartId)
      return `已删除图表「${undo.title}」`
    }
    case 'formula': {
      const sheet = sheetById(api, undo.sheetId)
      if (!sheet)
        return `${undo.cell} 所在的工作表已不存在`
      const range = sheet.getRange(undo.cell)
      if ((range.getFormula() || String(range.getValue() ?? '')) !== undo.written)
        return `${undo.cell} 已被修改，保留现有内容`
      if (undo.oldFormula)
        range.setValue(undo.oldFormula)
      else if (undo.oldValue != null && undo.oldValue !== '')
        range.setValue(undo.oldValue as string | number | boolean)
      else
        range.clearContent()
      return `已恢复 ${undo.cell} 原来的内容`
    }
    case 'calc': {
      await removeCalcApi(undo.datasetId, undo.name).catch((e) => {
        throw new Error(t('删除计算列「{0}」失败：{1}', [undo.name, e instanceof Error ? e.message : String(e)]))
      })
      onDatasetsChanged()
      return `已删除计算列「${undo.name}」`
    }
    case 'table_view':
      return undoTableView(api, undo)
    case 'column_format':
      return undoColumnFormat(api, undo)
    case 'table_create':
      return undoTableCreate(api, undo)
    case 'dataset_save':
      return undoDatasetSave(undo, onDatasetsChanged, operator)
  }
}
