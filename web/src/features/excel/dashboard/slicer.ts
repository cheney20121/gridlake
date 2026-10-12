import type { FUniver } from '@univerjs/core/facade'
import type { DatePreset, SlicerSpec } from './types'
import type { DatasetFilter, DatasetInfo } from '@/api/dataset'
import { shallowRef } from 'vue'
import { listFloats } from '@/features/excel/chart/chart-manager'
import { t } from '@/i18n'
import { SLICER_KEY } from './types'

/**
 * 切片器筛选
 *
 * 唯一的状态来源是工作表上各切片器的定义（随工作簿保存）：部件加载时从同一工作表的切片器定义算出筛选条件，
 * 切片器变化时写回定义并广播 slicer 事件，同一工作表的部件重新取数。
 */

// ── 数据集目录（页面刷新数据集列表时更新，部件据此判断数据集有没有切片器的列） ──

export const datasetCatalog = shallowRef<DatasetInfo[]>([])

export function setDatasetCatalog(list: DatasetInfo[]) {
  datasetCatalog.value = list
}

export function datasetById(id: string) {
  return datasetCatalog.value.find(d => d.id === id || d.name === id)
}

function hasColumn(ds: DatasetInfo | undefined, column: string) {
  return !!ds?.columns.some(c => c.title === column || c.key === column)
}

// ── 日期范围 ──

const pad = (n: number) => String(n).padStart(2, '0')
const ymd = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`

/** 相对时间 → 起止日期（本地时间，含两端） */
export function presetRange(preset: DatePreset, today = new Date()): { min: string, max: string } {
  const y = today.getFullYear()
  const m = today.getMonth()
  const day = (yy: number, mm: number, dd: number) => ymd(new Date(yy, mm, dd))
  const qStart = Math.floor(m / 3) * 3
  switch (preset) {
    case 'this_month':
      return { min: day(y, m, 1), max: day(y, m + 1, 0) }
    case 'last_month':
      return { min: day(y, m - 1, 1), max: day(y, m, 0) }
    case 'this_quarter':
      return { min: day(y, qStart, 1), max: day(y, qStart + 3, 0) }
    case 'last_quarter':
      return { min: day(y, qStart - 3, 1), max: day(y, qStart, 0) }
    case 'this_year':
      return { min: day(y, 0, 1), max: day(y, 11, 31) }
    case 'last_year':
      return { min: day(y - 1, 0, 1), max: day(y - 1, 11, 31) }
    case 'last_7':
      return { min: day(y, m, today.getDate() - 6), max: ymd(today) }
    case 'last_30':
      return { min: day(y, m, today.getDate() - 29), max: ymd(today) }
    case 'last_90':
      return { min: day(y, m, today.getDate() - 89), max: ymd(today) }
  }
}

/** 切片器当前的日期范围（没有选择时为 null） */
export function slicerRange(s: SlicerSpec): { min: string, max: string } | null {
  const r = s.range
  if (!r)
    return null
  if (r.preset)
    return presetRange(r.preset)
  if (r.min || r.max)
    return { min: r.min || '0000-01-01', max: r.max || '9999-12-31' }
  return null
}

/** 一个切片器的筛选条件（列用列名，后端按列名或键都能识别） */
export function slicerFilter(s: SlicerSpec): DatasetFilter | null {
  if (s.mode === 'date') {
    const r = slicerRange(s)
    // 日期时间列以文本 YYYY-MM-DD HH:MM:SS 存储：结束日补到当天最后一秒
    return r ? { op: 'between', column: s.column, min: r.min, max: `${r.max} 23:59:59` } : null
  }
  return s.values.length ? { op: 'in', column: s.column, values: s.values } : null
}

/** 工作表上的切片器 */
export function sheetSlicers(api: FUniver, sheetId: string) {
  return listFloats<SlicerSpec>(api, SLICER_KEY, sheetId).map(f => f.spec)
}

/**
 * 某个数据集部件要叠加的切片器筛选：同一工作表上、列在该数据集中存在、且有选择的切片器
 * exceptId：切片器取选项时排除自己（其他切片器的选择会联动缩小它的选项）
 */
export function slicerFiltersFor(api: FUniver, sheetId: string | null, datasetId: string, exceptId?: string): DatasetFilter[] {
  if (!sheetId)
    return []
  const ds = datasetById(datasetId)
  return sheetSlicers(api, sheetId)
    .filter(s => s.id !== exceptId && hasColumn(ds, s.column))
    .map(slicerFilter)
    .filter((f): f is DatasetFilter => !!f)
}

/** 切片器说明（如「店铺：旗舰店 等 2 项」「下单时间：本月」） */
export function slicerSummary(s: SlicerSpec, presetLabel: (p: DatePreset) => string) {
  if (s.mode === 'date') {
    if (s.range?.preset)
      return presetLabel(s.range.preset)
    const r = slicerRange(s)
    return r ? `${r.min} ~ ${r.max}` : '全部'
  }
  if (!s.values.length)
    return '全部'
  return s.values.length === 1 ? (s.values[0] || t('（空白）')) : `${s.values[0] || '（空白）'} 等 ${s.values.length} 项`
}
