import type { FUniver } from '@univerjs/core/facade'
import type { CrossFilter, Pick } from './cross-filter-pure'
import type { DatasetFilter } from '@/api/dataset'
import type { DatasetSource } from '@/features/excel/chart/types'
import { shallowRef } from 'vue'
import { chartBus } from '@/features/excel/chart/chart-manager'
import { crossFilterConditions, nextCrossFilter } from './cross-filter-pure'
import { datasetById, slicerFiltersFor } from './slicer'

/**
 * 图表点击联动：点数据集图表的某个分类（柱、扇区、折线上的点），同一工作表上的其他看板部件
 * （图表、指标卡、表格卡片、切片器的选项）按这个分类筛选；再点一次或点「×」取消。
 *
 * - 点柱子 / 扇区 / 折线上的点：选中这个分类；Ctrl（Mac 为 ⌘）+ 点击：在已选的基础上增减，可选多个
 * - 图表按某个字段拆分了系列时，点某个系列的柱子同时选中这个系列：其他部件按「分类 + 系列」筛选；
 *   多选时分类与系列各自取并集（选了 A·甲 和 B·乙，也会包含 A·乙、B·甲）
 * - 联动随工作簿保存（存在工作表元数据里），下次打开仍然生效；演示模式里点的只在本次演示有效，不产生修改
 * - 每个工作表同时只有一个联动筛选（点另一个图表会替换）
 * - 部件所用数据集没有这一列时不受影响（与切片器的规则一致）
 */

export type { CrossFilter, Pick } from './cross-filter-pure'

function hasColumn(datasetId: string, column: string) {
  return !!datasetById(datasetId)?.columns.some(c => c.title === column || c.key === column)
}

/** 工作表 → 联动筛选 */
export const crossFilters = shallowRef<Map<string, CrossFilter>>(new Map())

export function crossFilterOf(sheetId: string | null | undefined): CrossFilter | null {
  return (sheetId && crossFilters.value.get(sheetId)) || null
}

/** 点击了图表 chartId 的分类（及系列）：按 nextCrossFilter 更新 */
export function toggleCrossFilter(sheetId: string, chartId: string, src: DatasetSource, pick: Pick) {
  const next = new Map(crossFilters.value)
  const f = nextCrossFilter(crossFilters.value.get(sheetId) ?? null, chartId, src, pick)
  if (f)
    next.set(sheetId, f)
  else
    next.delete(sheetId)
  crossFilters.value = next
  chartBus.emit('slicer', sheetId)
}

export function clearCrossFilter(sheetId: string) {
  if (!crossFilters.value.has(sheetId))
    return
  const next = new Map(crossFilters.value)
  next.delete(sheetId)
  crossFilters.value = next
  chartBus.emit('slicer', sheetId)
}

/** 图表被删除时去掉它发起的联动 */
export function dropCrossFiltersOf(chartId: string) {
  for (const [sheetId, f] of crossFilters.value) {
    if (f.chartId === chartId)
      clearCrossFilter(sheetId)
  }
}

/**
 * 看板部件要叠加的筛选：切片器 + 图表联动。
 * exceptId：发起联动的图表自己不按联动筛选（改为高亮选中的分类）；切片器取选项时排除自己
 */
export function dashboardFiltersFor(api: FUniver, sheetId: string | null, datasetId: string, exceptId?: string): DatasetFilter[] {
  const filters = slicerFiltersFor(api, sheetId, datasetId, exceptId)
  const cross = crossFilterOf(sheetId)
  if (cross && cross.chartId !== exceptId) {
    // 联动的筛选字段在别的数据集里：用部件自己的数据集里同名的列
    filters.push(...crossFilterConditions(cross, col => hasColumn(datasetId, col)))
  }
  return filters
}

/** 部件当前受哪个联动筛选影响（显示「联动」标记）；不受影响时为 null。读取 crossFilters 以便在组件里响应变化 */
export function crossFilterAffecting(sheetId: string | null, datasetId: string, selfId: string): CrossFilter | null {
  const f = crossFilters.value && crossFilterOf(sheetId)
  if (!f || f.chartId === selfId)
    return null
  return crossFilterConditions(f, col => hasColumn(datasetId, col)).length ? f : null
}

// ── 随工作簿保存 ──

const META_KEY = 'crossfilter'
/** 现在能不能把联动写进工作簿（演示模式里不能：演示是只读的，不产生修改） */
let persistGuard: () => boolean = () => true

export function setCrossFilterPersistGuard(fn: () => boolean) {
  persistGuard = fn
}

/** 把工作表当前的联动写进它的元数据（没有联动时删除） */
export function persistCrossFilter(api: FUniver, sheetId: string) {
  if (!persistGuard())
    return
  const sheet = api.getActiveWorkbook()?.getSheetBySheetId(sheetId)
  if (!sheet)
    return
  const meta = { ...(sheet.getCustomMetadata() as Record<string, unknown> | undefined) }
  const f = crossFilters.value.get(sheetId)
  if (f)
    meta[META_KEY] = f
  else if (META_KEY in meta)
    delete meta[META_KEY]
  else
    return
  sheet.setCustomMetadata(meta)
}

/**
 * 打开工作簿后恢复各工作表保存的联动。浮动的图表要等页面渲染后才能列出来，所以先恢复，再分几次核对：
 * 发起联动的图表一直找不到（已被删除）的就丢弃，否则没有地方可以取消它。chartExists 判断图表是否还在；wrap 把清理元数据的写入包一层（如不算用户修改）
 */
export function restoreCrossFilters(api: FUniver, chartExists: (sheetId: string, chartId: string) => boolean, wrap: (fn: () => void) => void = fn => fn()) {
  const next = new Map<string, CrossFilter>()
  for (const sheet of api.getActiveWorkbook()?.getSheets() ?? []) {
    const f = (sheet.getCustomMetadata() as Record<string, unknown> | undefined)?.[META_KEY] as CrossFilter | undefined
    if (f && Array.isArray(f.values) && f.values.length && Array.isArray(f.picks))
      next.set(sheet.getSheetId(), f)
  }
  crossFilters.value = next
  for (const id of next.keys())
    chartBus.emit('slicer', id)
  if (!next.size)
    return
  const check = (attempt: number) => {
    for (const [sheetId, f] of crossFilters.value) {
      if (!next.has(sheetId) || chartExists(sheetId, f.chartId))
        continue
      if (attempt < 5) {
        setTimeout(check, 600, attempt + 1)
        return
      }
      // 图表已经不在了：丢弃这个联动（不写回工作簿，下次保存时随之消失）
      clearCrossFilter(sheetId)
      const sheet = api.getActiveWorkbook()?.getSheetBySheetId(sheetId)
      const meta = { ...(sheet?.getCustomMetadata() as Record<string, unknown> | undefined) }
      if (sheet && META_KEY in meta) {
        delete meta[META_KEY]
        wrap(() => sheet.setCustomMetadata(meta))
      }
    }
  }
  setTimeout(check, 300, 0)
}
