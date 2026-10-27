import type { DatasetInfo } from '@/api/dataset'
import type { ColumnKind, TemplateColumn, TemplateDataset } from '@/api/template'
import { isNumericKind } from '@/features/excel/dataset/column-kinds'

/**
 * 使用模板时自动对应：为模板需要的数据集挑选最像的数据集，为每一列挑选最像的列
 * 优先级：同名 → 常见同义词 → 名称互相包含 → 同类型且唯一（日期 / 金额）
 */

/** 电商财务常见列名的同义词（双向使用） */
const SYNONYMS: string[][] = [
  ['店铺', '门店', '店铺名称', '店名', '店铺名', '渠道', '商家', '店铺 / 渠道'],
  ['下单时间', '支付时间', '订单时间', '创建时间', '成交时间', '下单日期', '付款时间', '订单创建时间'],
  ['订单金额', '实付金额', '支付金额', '成交金额', '订单实付', '实付', '付款金额', '金额', '订单实付应结', 'GMV'],
  ['日期', '投放日期', '消耗日期', '统计日期', '时间', '数据日期'],
  ['消耗', '花费', '消费', '投放金额', '投放消耗', '费用', '整体消耗', '消耗金额'],
  ['商品名称', '商品', '商品标题', '宝贝名称'],
  ['动账时间', '入账时间', '结算时间'],
  ['动账金额', '金额', '入账金额'],
]

export function compatible(need: ColumnKind, have: string) {
  if (need === 'datetime')
    return have === 'datetime'
  if (need === 'boolean')
    return have === 'boolean'
  if (isNumericKind(need))
    return isNumericKind(have)
  return true
}

function synonyms(title: string) {
  return SYNONYMS.filter(g => g.includes(title)).flat()
}

/** 为模板列在数据集中找对应列；used 为已被其他模板列占用的列 */
export function suggestColumn(col: TemplateColumn, ds: DatasetInfo, used: Set<string> = new Set()): string | null {
  const cands = ds.columns.filter(c => compatible(col.kind, c.kind) && !used.has(c.title))
  const exact = cands.find(c => c.title === col.title)
  if (exact)
    return exact.title
  const syn = synonyms(col.title)
  // 按同义词表中的顺序优先（更常见的说法在前）
  for (const s of syn) {
    const hit = cands.find(c => c.title === s)
    if (hit)
      return hit.title
  }
  const contains = cands.find(c => c.title.includes(col.title) || col.title.includes(c.title))
  if (contains)
    return contains.title
  if (col.kind === 'datetime' || col.kind === 'money') {
    const strict = cands.filter(c => c.kind === col.kind)
    if (strict.length === 1)
      return strict[0]!.title
  }
  return null
}

/** 整个数据集的列对应（避免两列对到同一列） */
export function suggestColumns(need: TemplateDataset, ds: DatasetInfo): Record<string, string | null> {
  const used = new Set<string>()
  const out: Record<string, string | null> = {}
  // 先处理有同名列的，再处理需要推测的
  const order = [...need.columns].sort((a, b) => Number(!ds.columns.some(c => c.title === a.title)) - Number(!ds.columns.some(c => c.title === b.title)))
  for (const col of order) {
    const hit = suggestColumn(col, ds, used)
    out[col.title] = hit
    if (hit)
      used.add(hit)
  }
  return out
}

/** 为模板数据集挑选最像的数据集（对上的列越多越好，名称相近加分）；都对不上时返回 null */
export function suggestDataset(need: TemplateDataset, datasets: DatasetInfo[], taken: Set<string> = new Set()): string | null {
  if (need.builtinId && datasets.some(d => d.id === need.builtinId))
    return need.builtinId
  let best: { id: string, score: number } | null = null
  for (const ds of datasets) {
    if (taken.has(ds.id))
      continue
    const cols = suggestColumns(need, ds)
    const matched = Object.values(cols).filter(Boolean).length
    if (!matched)
      continue
    let score = matched * 10 + (matched === need.columns.length ? 20 : 0)
    if (ds.name === need.name)
      score += 15
    else if (ds.name.includes(need.name) || need.name.includes(ds.name))
      score += 8
    else if (synonyms(need.name).some(s => ds.name.includes(s)))
      score += 4
    if (ds.rowCount > 0)
      score += 1
    if (!best || score > best.score)
      best = { id: ds.id, score }
  }
  return best?.id ?? null
}
