import type { DatasetFilter } from '@/api/dataset'
import type { DatasetSource } from '@/features/excel/chart/types'

/**
 * 图表点击联动的纯逻辑（点击后的选中状态、显示文字、筛选条件），不依赖界面与 Univer，便于单元测试。
 * 行为说明见 cross-filter.ts
 */

export interface CrossFilter {
  /** 发起联动的图表 */
  chartId: string
  datasetId: string
  column: string
  granularity?: 'year' | 'month' | 'day' | null
  /** 点中的柱 / 扇区：[分类, 系列]，没有按系列时系列为 null（空白分类为 ""） */
  picks: [string, string | null][]
  /** 选中的分类值（picks 里的分类去重，筛选用） */
  values: string[]
  /** 按系列联动：拆分字段及选中的系列值（picks 里每一个都带系列时才有，否则不按系列筛选） */
  series?: { column: string, values: string[] }
  /** 显示用（如「店铺：旗舰店」「下单时间（月）：2026-03、2026-05 · 渠道：线上」） */
  label: string
}

const BLANK = '（空）'
const GRANULARITY_TEXT: Record<string, string> = { year: '（年）', month: '（月）', day: '（日）' }

function listText(values: string[]) {
  const shown = values.slice(0, 3).map(v => v || '（空白）').join('、')
  return values.length > 3 ? `${shown} 等 ${values.length} 项` : shown
}

const unique = (list: string[]) => [...new Set(list)]

/** 由点中的柱子得到完整的联动筛选（分类、系列、显示文字） */
function build(chartId: string, src: DatasetSource, picks: [string, string | null][]): CrossFilter {
  const splitBy = src.splitBy ?? null
  const values = unique(picks.map(p => p[0]))
  // 只要有一次点的是分类（没带系列），这个分类下的所有系列都要，就不能再按系列筛
  const seriesValues = splitBy && picks.every(p => p[1] != null) ? unique(picks.map(p => p[1]!)) : []
  const base = {
    chartId,
    datasetId: src.datasetId,
    column: src.x.column,
    granularity: src.x.granularity ?? null,
    picks,
    values,
    series: seriesValues.length && splitBy ? { column: splitBy, values: seriesValues } : undefined,
  }
  const text = `${base.column}${GRANULARITY_TEXT[base.granularity ?? ''] ?? ''}：${listText(values)}`
  return { ...base, label: base.series ? `${text} · ${base.series.column}：${listText(base.series.values)}` : text }
}

export interface Pick {
  /** 点中的分类 */
  category: string
  /** 点中的系列（图表按某个字段拆分了系列、点在某个系列的柱子上时才有） */
  series?: string
  /** Ctrl / ⌘ 点击：在已选的基础上增减 */
  additive?: boolean
}

const samePick = (a: [string, string | null], b: [string, string | null]) => a[0] === b[0] && a[1] === b[1]

/** 点击后的联动筛选（纯函数）：返回 null 表示取消 */
export function nextCrossFilter(cur: CrossFilter | null, chartId: string, src: DatasetSource, pick: Pick): CrossFilter | null {
  const pk: [string, string | null] = [pick.category === BLANK ? '' : pick.category, src.splitBy && pick.series != null ? pick.series : null]
  const sameChart = cur && cur.chartId === chartId && cur.column === src.x.column
  if (!sameChart)
    return build(chartId, src, [pk])
  if (pick.additive) {
    // 在已选的基础上增减：已经选了就去掉，否则加上；去光了就取消
    const picks = cur.picks.some(p => samePick(p, pk)) ? cur.picks.filter(p => !samePick(p, pk)) : [...cur.picks, pk]
    return picks.length ? build(chartId, src, picks) : null
  }
  // 单击：与当前选中完全相同（再点一次）则取消，否则替换
  return cur.picks.length === 1 && samePick(cur.picks[0]!, pk) ? null : build(chartId, src, [pk])
}

/** 联动筛选条件：日期按年 / 月 / 日分组的取值用前缀匹配；hasColumn 判断部件的数据集有没有这一列（没有的不筛选，与切片器的规则一致） */
export function crossFilterConditions(f: CrossFilter, hasColumn: (column: string) => boolean): DatasetFilter[] {
  const out: DatasetFilter[] = []
  if (hasColumn(f.column)) {
    if (f.granularity)
      out.push(f.values.length === 1 ? { op: 'prefix', column: f.column, value: f.values[0]! } : { op: 'prefixes', column: f.column, values: f.values })
    else
      out.push({ op: 'in', column: f.column, values: f.values })
  }
  if (f.series?.values.length && hasColumn(f.series.column))
    out.push({ op: 'in', column: f.series.column, values: f.series.values })
  return out
}
