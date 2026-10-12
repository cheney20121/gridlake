/**
 * 店铺资金汇总工作簿装配
 *
 * 明细存放在 server（Rust + DuckDB），前端只加载某个月份的聚合结果（几十到几百行）：
 * - 「流水汇总数据」页签：按 主体 × 店铺 × 动账场景 × 动账方向 聚合的金额
 * - 「汇总表」页签：引用流水汇总数据的 SUMIFS 公式
 * 导入下个月明细或切换月份时，只替换流水汇总数据，汇总表公式自动重算
 *
 * 主体简称（汇总表第 1 行）保存在后端：用户修改后由 watchEntityAliasEdits 立即保存，
 * 加载时随聚合结果一并返回
 */

import type { IWorkbookData, IWorksheetData } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'
import type { FWorkbook } from '@univerjs/sheets/facade'
import type { RuleTerm, SummaryColumn } from './summary-template'
import type { LedgerDetailFilter, LedgerPivot } from '@/api/ledger'
import { getLedgerPivotApi, saveEntityAliasesApi } from '@/api/ledger'
import { datasetFormulasBusy, invalidateDatasetFormulas } from '@/features/excel/formula/dataset-functions'
import { t } from '@/i18n'
import { disposeWorkbook } from '../data/selection'
import { buildSummarySheet, getDrillRule, LEGACY_PIVOT_SHEET_NAME, readEntityAliases, shortEntityName, SUBTOTAL_SHOP_LABEL, SUMMARY_ROW, SUMMARY_SHEET_NAME, SUMMARY_STYLES, summaryLabelAt, TOTAL_ENTITY_LABEL, TOTAL_SHOP_LABEL } from './summary-template'

const SUMMARY_SHEET_ID = 'ledger-summary'
/** 旧版「流水汇总数据」页签（公式下推后由 DSUMIFS 直接查数据集，加载时移除） */
const LEGACY_PIVOT_SHEET_ID = 'ledger-pivot'
/** 浏览器端解析 CSV 时代遗留的明细页签（id 以 detail- 开头），切到后端方案后不再需要 */
const LEGACY_DETAIL_PREFIX = 'detail-'
/** 简称改存后端之前保存在浏览器本地，首次加载时迁移到后端后删除 */
const LEGACY_ALIAS_STORAGE_KEY = 'ledger-entity-aliases'
const SET_RANGE_VALUES_MUTATION = 'sheet.mutation.set-range-values'

/** 当前汇总表上显示的简称：主体全称 → 简称。用于识别用户修改 */
const displayedAliases = new Map<string, string>()

function readLegacyAliases(): Map<string, string> {
  try {
    return new Map(Object.entries(JSON.parse(localStorage.getItem(LEGACY_ALIAS_STORAGE_KEY) || '{}') as Record<string, string>))
  }
  catch {
    return new Map()
  }
}

/**
 * 重建前补存尚未保存的简称：本地旧记录（迁移）+ 当前汇总表上与已保存值不同的修改（如上次保存时网络失败）
 * 返回 主体全称 → 简称（已保存的值；保存失败时用本地值兜底，避免用户的修改从界面上消失）
 */
async function syncAliases(pivot: LedgerPivot, summaryCellData: unknown): Promise<Map<string, string>> {
  const saved = new Map(pivot.entities.filter(e => e.alias).map(e => [e.name, e.alias as string]))
  const pending = new Map<string, string>()

  const legacy = readLegacyAliases()
  legacy.forEach((short, full) => {
    if (!saved.has(full) && short !== shortEntityName(full))
      pending.set(full, short)
  })
  readEntityAliases(summaryCellData as never).forEach((short, full) => {
    if (displayedAliases.has(full) && displayedAliases.get(full) !== short)
      pending.set(full, short)
  })

  if (pending.size) {
    try {
      const result = await saveEntityAliasesApi([...pending].map(([entity, alias]) => ({ entity, alias })))
      result.forEach(a => saved.set(a.entity, a.alias))
      localStorage.removeItem(LEGACY_ALIAS_STORAGE_KEY)
    }
    catch (e) {
      console.warn('[ledger] 主体简称保存失败，暂用本地值显示:', e)
      pending.forEach((short, full) => saved.set(full, short))
    }
  }
  else if (legacy.size) {
    localStorage.removeItem(LEGACY_ALIAS_STORAGE_KEY)
  }
  return saved
}

/** 汇总表列：主体按流水笔数降序（主力主体在前），组内店铺按名称 */
function buildSummaryColumns(pivot: LedgerPivot, aliases: Map<string, string>): SummaryColumn[] {
  const entityIdx = pivot.columns.indexOf('商户主体名称')
  const shopIdx = pivot.columns.indexOf('店铺')
  const combos = new Set(pivot.rows.map(r => `${r[entityIdx]}\u0001${r[shopIdx]}`))
  return pivot.entities.flatMap(entity => pivot.shops
    .filter(shop => combos.has(`${entity.name}\u0001${shop}`))
    .map(shop => ({
      entityFull: entity.name,
      entityShort: aliases.get(entity.name) ?? shortEntityName(entity.name),
      shop,
    })))
}

export interface LoadResult {
  workbook: FWorkbook
  pivot: LedgerPivot
  columns: SummaryColumn[]
}

/**
 * 加载某个月份：按该月出现的 主体 × 店铺 重建「汇总表」页签（其他页签保持不变）。
 * 汇总表公式（DSUMIFS）直接查服务端数据集，聚合接口只用来确定有哪些列
 */
export async function loadLedgerPeriod(api: FUniver, period: string): Promise<LoadResult> {
  const pivot = await getLedgerPivotApi(period)

  const current = api.getActiveWorkbook()
  const snapshot = (current?.save() ?? { id: 'shop-ledger', name: '店铺资金汇总', sheetOrder: [], sheets: {}, styles: {} }) as IWorkbookData
  const sheets = Object.fromEntries(
    Object.entries(snapshot.sheets).filter(([id, sheet]) =>
      !id.startsWith(LEGACY_DETAIL_PREFIX) && !(id === LEGACY_PIVOT_SHEET_ID && sheet.name === LEGACY_PIVOT_SHEET_NAME)),
  ) as Record<string, Partial<IWorksheetData>>

  const conflict = Object.values(sheets).find(s => s.name === SUMMARY_SHEET_NAME && s.id !== SUMMARY_SHEET_ID)
  if (conflict)
    throw new Error(t('已存在名为「{0}」的其他工作表，请先重命名', [conflict.name]))

  const aliases = await syncAliases(pivot, sheets[SUMMARY_SHEET_ID]?.cellData)
  const columns = buildSummaryColumns(pivot, aliases)
  displayedAliases.clear()
  columns.forEach(c => displayedAliases.set(c.entityFull, c.entityShort))
  sheets[SUMMARY_SHEET_ID] = {
    id: SUMMARY_SHEET_ID,
    name: SUMMARY_SHEET_NAME,
    ...buildSummarySheet(columns, pivot.period),
    defaultRowHeight: 28,
    defaultColumnWidth: 120,
    freeze: { xSplit: 1, ySplit: 2, startRow: 2, startColumn: 1 },
    showGridlines: 1,
    rightToLeft: 0,
  } as Partial<IWorksheetData>

  const others = (snapshot.sheetOrder ?? []).filter(id => id !== SUMMARY_SHEET_ID && sheets[id])
  const workbookData: Partial<IWorkbookData> = {
    ...snapshot,
    sheetOrder: [SUMMARY_SHEET_ID, ...others],
    sheets: sheets as IWorkbookData['sheets'],
    styles: { ...snapshot.styles, ...SUMMARY_STYLES } as IWorkbookData['styles'],
  }

  // 数据可能已变化（导入、改名）：清空下推结果缓存，重建后的工作簿会重新批量取数
  invalidateDatasetFormulas(false)

  if (current)
    disposeWorkbook(api, current.getId())
  const workbook = api.createWorkbook(workbookData)
  workbook.setActiveSheet(workbook.getSheetBySheetId(SUMMARY_SHEET_ID)!)
  await api.getFormula().onCalculationResultApplied(30_000).catch(() => {
    // 超时不阻断：公式会在后台继续计算
  })
  // 第一遍计算只收集了未命中的下推请求，等批量结果回来、第二遍重算完成
  await waitDatasetFormulas(api)
  return { workbook, pivot, columns }
}

/** 等待下推计算（批量请求 + 第二遍重算）完成，最多 60 秒 */
async function waitDatasetFormulas(api: FUniver) {
  const deadline = Date.now() + 60_000
  while (datasetFormulasBusy() && Date.now() < deadline)
    await new Promise(r => setTimeout(r, 50))
  await api.getFormula().onCalculationResultApplied(30_000).catch(() => {})
}

/** 店铺名用作汇总表列标题及公式条件，限制与后端一致 */
export function validateShopName(name: string): string | null {
  const shop = name.trim()
  if (!shop)
    return '请输入店铺名称'
  if (/[[\]:*?/\\]/.test(shop))
    return '店铺名称不能包含 [ ] : * ? / \\'
  if (shop.length > 31)
    return '店铺名称不能超过 31 个字符'
  if (shop === SUBTOTAL_SHOP_LABEL || shop === TOTAL_SHOP_LABEL)
    return `「${shop}」是汇总表保留名称，请换一个店铺名`
  return null
}

export interface AliasWatcher {
  dispose: () => void
}

/**
 * 监听汇总表第 1 行（主体简称）的修改并保存到后端
 * 300ms 内的多次修改合并为一次请求；撤销/重做同样会触发保存。
 * 只改样式（如加粗）的变更不含 v 字段，不视为修改简称；清空内容视为删除简称、恢复默认
 */
export function watchEntityAliasEdits(
  api: FUniver,
  handlers: { onSaved?: (items: { entity: string, alias: string | null }[]) => void, onError?: (e: unknown) => void } = {},
): AliasWatcher {
  const pending = new Map<string, string | null>()
  let timer: ReturnType<typeof setTimeout> | null = null

  const flush = async () => {
    timer = null
    const items = [...pending].map(([entity, alias]) => ({ entity, alias }))
    pending.clear()
    if (!items.length)
      return
    try {
      await saveEntityAliasesApi(items)
      items.forEach(i => displayedAliases.set(i.entity, i.alias ?? shortEntityName(i.entity)))
      handlers.onSaved?.(items)
    }
    catch (e) {
      handlers.onError?.(e)
    }
  }

  const listener = api.addEvent(api.Event.CommandExecuted, (event) => {
    if (event.id !== SET_RANGE_VALUES_MUTATION)
      return
    const params = event.params as { subUnitId?: string, cellValue?: Record<number, Record<number, { v?: unknown } | null>> }
    if (params.subUnitId !== SUMMARY_SHEET_ID)
      return
    const row = params.cellValue?.[SUMMARY_ROW.entity]
    const sheet = api.getActiveWorkbook()?.getSheetBySheetId(SUMMARY_SHEET_ID)
    if (!row || !sheet)
      return

    for (const [key, cell] of Object.entries(row)) {
      const col = Number(key)
      if (col < 1 || (cell !== null && !('v' in cell)))
        continue
      const full = sheet.getRange(SUMMARY_ROW.entityFull, col).getValue()
      if (typeof full !== 'string' || !full || full === TOTAL_ENTITY_LABEL)
        continue
      const alias = cell?.v == null ? '' : String(cell.v).trim()
      if (alias === (displayedAliases.get(full) ?? ''))
        continue
      pending.set(full, alias || null)
    }

    if (pending.size) {
      if (timer)
        clearTimeout(timer)
      timer = setTimeout(flush, 300)
    }
  })

  return {
    dispose() {
      if (timer) {
        clearTimeout(timer)
        void flush()
      }
      listener.dispose()
    },
  }
}

// ── 明细下钻 ──────────────────────────────────────────

/** 打开明细时的上下文：从汇总表 / 流水汇总数据中选中的单元格推导 */
export interface DrillTarget {
  /** 按钮与明细标题，如「平台补贴 · 某某主体 / 某某店」 */
  title: string
  /** 固定的筛选上下文（不在明细里显示为可改的筛选项） */
  filter: Pick<LedgerDetailFilter, 'shop' | 'entity' | 'scenes' | 'directions' | 'billingTypes' | 'talentId' | 'productId' | 'orderMonth'>
  /** 汇总项规则：只看对该汇总项有贡献的流水 */
  rule?: RuleTerm[]
  /** 核对基准：明细侧统计应等于这个值 */
  /**
   * contribution：规则贡献合计；count：笔数；columnSum：明细中某列合计（透视表的求和格子，column 为列名）
   */
  expected?: { label: string, value: number, kind: 'contribution' | 'count' | 'columnSum', column?: string }
}

function cellText(sheet: ReturnType<FWorkbook['getActiveSheet']>, row: number, col: number): string {
  const v = sheet.getRange(row, col).getRawValue()
  return v == null ? '' : String(v)
}

/** 根据当前选中的单元格解析下钻目标；不在汇总相关页签上时返回 null（打开全部明细） */
export function resolveDrillTarget(api: FUniver): DrillTarget | null {
  const workbook = api.getActiveWorkbook()
  const sheet = workbook?.getActiveSheet()
  const range = workbook?.getActiveRange()?.getRange()
  if (!sheet || !range)
    return null
  const { startRow: row, startColumn: col } = range

  if (sheet.getSheetId() === SUMMARY_SHEET_ID) {
    if (col < 1)
      return null
    const entity = cellText(sheet, SUMMARY_ROW.entityFull, col)
    const shop = cellText(sheet, SUMMARY_ROW.shop, col)
    if (!entity || !shop)
      return null
    // 合计列：主体合计只按主体筛选，全部合计不限主体与店铺
    let scope: string
    let filter: DrillTarget['filter']
    if (shop === TOTAL_SHOP_LABEL) {
      scope = '全部店铺'
      filter = {}
    }
    else if (shop === SUBTOTAL_SHOP_LABEL) {
      scope = `${displayedAliases.get(entity) ?? shortEntityName(entity)} 主体合计`
      filter = { entity }
    }
    else {
      scope = `${displayedAliases.get(entity) ?? shortEntityName(entity)} / ${shop}`
      filter = { entity, shop }
    }
    const label = summaryLabelAt(row)
    const rule = label ? getDrillRule(label) : null
    if (!label || !rule)
      return { title: scope, filter }
    const value = sheet.getRange(row, col).getRawValue()
    return {
      title: `${label} · ${scope}`,
      filter,
      rule,
      expected: typeof value === 'number' ? { label: t('汇总表「{0}」', [label]), value, kind: 'contribution' } : undefined,
    }
  }

  return null
}
