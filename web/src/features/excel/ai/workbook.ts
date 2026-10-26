import type { FUniver } from '@univerjs/core/facade'
import type { DatasetSavePayload, DatasetSaveUndo } from './workbook-dataset'
import type { ColumnFormatPayload, ColumnFormatUndo, TableCreatePayload, TableCreateUndo, TableViewPayload, TableViewUndo } from './workbook-table'
import type { AiAction, AiContext } from '@/api/ai'
import type { CleanOp, DatasetColumnKind, ValidationRule, WindowSpec } from '@/api/dataset'
import type { ChartSpec } from '@/features/excel/chart/types'
import type { PivotSpec } from '@/features/excel/pivot/pivot-sheet'
import { addCalcApi, applyCleanApi, createWindowApi, getValidationApi, lastCleanApi, removeCalcApi, saveValidationApi, undoCleanApi } from '@/api/dataset'
import { colName } from '@/features/excel/chart/chart-defaults'
import { insertChart, newChartId, removeChart } from '@/features/excel/chart/chart-manager'
import { currentDatasetView } from '@/features/excel/dataset/active-view'
import { markDataChanged } from '@/features/excel/dataset/data-changes'
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
  // 数据集页面在前台：带上正在看的数据集与筛选；工作表、选区被盖在后面，不带（工作簿里的数据表仍可调整）
  const view = currentDatasetView()
  if (view) {
    const { id: _, ...dataset } = view
    const tables = collectTableContext(api)
    return tables.length ? { dataset, tables } : { dataset }
  }
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
    | { kind: 'clean', datasetId: string, datasetName: string, summary: string, at: string | null }
    | { kind: 'validation', datasetId: string, datasetName: string, previous: ValidationRule[], applied: ValidationRule[] }
    | { kind: 'plan', steps: UndoStep[] }

export interface Applied {
  /** 结果说明（记入审计） */
  message: string
  undo: UndoStep
}

const KIND_NAME: Record<string, string> = { pivot: t('透视表'), chart: t('图表'), formula: t('公式'), calc: t('计算列'), table_view: t('数据表视图'), column_format: t('列样式'), table_create: t('新建数据表'), dataset_save: t('另存数据集'), clean: t('数据清洗'), window: t('窗口计算'), validation: t('校验规则') }

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
    case 'clean': {
      // 删除行再确认一次（应用按钮旁边没有预览之外的提示）
      if (p.deletes) {
        const ok = await ElMessageBox.confirm(t('将从「{0}」删除 {1} 行（{2}），之后可以撤销。继续？', [p.datasetName, Number(p.affected).toLocaleString(), p.summary]), t('删除行'), { type: 'warning', confirmButtonText: t('删除') }).then(() => true, () => false)
        if (!ok)
          throw new Error(t('已取消'))
      }
      const r = await applyCleanApi(p.datasetId, p.op as CleanOp)
      // 记下是哪一次清洗：服务端只能撤销最近一次，撤销前要确认最近一次还是它
      const last = await lastCleanApi(p.datasetId).catch(() => null)
      markDataChanged(p.datasetId)
      onDatasetsChanged()
      return {
        message: r.deletes
          ? t('已清洗「{0}」：{1}，删除 {2} 行', [p.datasetName, r.summary, r.affected.toLocaleString()])
          : t('已清洗「{0}」：{1}，修改 {2} 行', [p.datasetName, r.summary, r.affected.toLocaleString()]),
        undo: { kind: 'clean', datasetId: p.datasetId, datasetName: p.datasetName, summary: r.summary, at: last?.createdAt ?? null },
      }
    }
    case 'validation': {
      // 与「数据校验」对话框同一个接口：保存规则并立即检查；撤销时恢复原来的规则
      const v = await saveValidationApi(p.datasetId, p.rules as ValidationRule[])
      onDatasetsChanged()
      const failed = v.report?.failed ?? 0
      return {
        message: failed
          ? t('已给「{0}」保存 {1} 条校验规则，{2} 条有不合格的行', [p.datasetName, v.rules.length, failed])
          : t('已给「{0}」保存 {1} 条校验规则，全部通过', [p.datasetName, v.rules.length]),
        undo: { kind: 'validation', datasetId: p.datasetId, datasetName: p.datasetName, previous: (p.previous ?? []) as ValidationRule[], applied: v.rules },
      }
    }
    case 'window': {
      // 与「更多 → 窗口计算」同一个接口：保存为 SQL 查询并另存为数据集，源数据更新后自动重算；撤销与另存数据集相同
      const q = await createWindowApi(p.datasetId, { ...(p.spec as WindowSpec), name: p.name, autoRefresh: true })
      if (!q.datasetId)
        throw new Error(t('数据集没有创建成功'))
      onDatasetsChanged()
      return {
        message: t('已用窗口计算生成数据集「{0}」{1}', [p.name, q.lastRows != null ? t('（{0} 行）', [q.lastRows.toLocaleString()]) : '']),
        undo: { kind: 'dataset_save', queryId: q.id, datasetId: q.datasetId, name: p.name },
      }
    }
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
      throw new Error(t('第 {0} 步（{1}）失败：{2}{3}', [i + 1, KIND_NAME[step.kind] ?? step.kind, reason, done.length ? t('；已撤回前面 {0} 步', [done.length]) : '']))
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
        return t('透视表页签「{0}」已不存在', [undo.name])
      api.getActiveWorkbook()?.deleteSheet(sheet)
      return t('已删除透视表页签「{0}」', [undo.name])
    }
    case 'chart': {
      removeChart(api, undo.chartId)
      return t('已删除图表「{0}」', [undo.title])
    }
    case 'formula': {
      const sheet = sheetById(api, undo.sheetId)
      if (!sheet)
        return t('{0} 所在的工作表已不存在', [undo.cell])
      const range = sheet.getRange(undo.cell)
      if ((range.getFormula() || String(range.getValue() ?? '')) !== undo.written)
        return t('{0} 已被修改，保留现有内容', [undo.cell])
      if (undo.oldFormula)
        range.setValue(undo.oldFormula)
      else if (undo.oldValue != null && undo.oldValue !== '')
        range.setValue(undo.oldValue as string | number | boolean)
      else
        range.clearContent()
      return t('已恢复 {0} 原来的内容', [undo.cell])
    }
    case 'calc': {
      await removeCalcApi(undo.datasetId, undo.name).catch((e) => {
        throw new Error(t('删除计算列「{0}」失败：{1}', [undo.name, e instanceof Error ? e.message : String(e)]))
      })
      onDatasetsChanged()
      return t('已删除计算列「{0}」', [undo.name])
    }
    case 'table_view':
      return undoTableView(api, undo)
    case 'column_format':
      return undoColumnFormat(api, undo)
    case 'table_create':
      return undoTableCreate(api, undo)
    case 'dataset_save':
      return undoDatasetSave(undo, onDatasetsChanged, operator)
    case 'clean': {
      // 服务端只能撤销最近一次、且之后没有别的改动的清洗：最近一次已经不是这次（之后又清洗过），或之后数据又变了，
      // 都报错说明原因，建议保持「已应用」
      const last = await lastCleanApi(undo.datasetId)
      if (!last || last.summary !== undo.summary || (undo.at && last.createdAt !== undo.at))
        throw new Error(t('「{0}」之后又做过别的清洗，只能撤销最近一次', [undo.datasetName]))
      if (!last.canUndo)
        throw new Error(last.reason || t('不能撤销'))
      await undoCleanApi(undo.datasetId)
      markDataChanged(undo.datasetId)
      onDatasetsChanged()
      return t('已撤销对「{0}」的清洗（{1}）', [undo.datasetName, undo.summary])
    }
    case 'validation': {
      // 之后在对话框里又改过规则：不覆盖别人的修改
      const now = await getValidationApi(undo.datasetId)
      if (JSON.stringify(now.rules) !== JSON.stringify(undo.applied))
        throw new Error(t('「{0}」的校验规则之后又改过，没有撤销', [undo.datasetName]))
      await saveValidationApi(undo.datasetId, undo.previous)
      onDatasetsChanged()
      return t('已恢复「{0}」原来的校验规则（{1} 条）', [undo.datasetName, undo.previous.length])
    }
  }
}
