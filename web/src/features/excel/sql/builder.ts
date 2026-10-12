import type { DatasetInfo } from '@/api/dataset'

/**
 * 查询构建器：选数据集、分组、汇总值、筛选、排序 → 生成可读的 SQL（之后可在编辑器里继续改）
 * 标识符一律加双引号（中文列名、含空格的名称都安全），文本值用单引号并转义
 */

export type Granularity = 'year' | 'month' | 'day' | null
export type Agg = 'sum' | 'count' | 'avg' | 'min' | 'max' | 'count_distinct'
export type FilterOp = '=' | '<>' | '>' | '>=' | '<' | '<=' | 'in' | 'contains' | 'prefix' | 'between' | 'empty' | 'not_empty'

export interface BuilderModel {
  dataset: string
  /** 分组字段（有汇总值时）或要列出的字段（没有汇总值时） */
  fields: { column: string, granularity: Granularity }[]
  measures: { agg: Agg, column: string | null, alias: string }[]
  filters: { column: string, op: FilterOp, value: string, value2: string }[]
  sort: { by: string, desc: boolean } | null
  limit: number | null
}

export const AGG_LABEL: Record<Agg, string> = {
  sum: '求和',
  count: '计数',
  avg: '平均',
  min: '最小',
  max: '最大',
  count_distinct: '去重计数',
}

export const OP_LABEL: Record<FilterOp, string> = {
  '=': '等于',
  '<>': '不等于',
  '>': '大于',
  '>=': '大于等于',
  '<': '小于',
  '<=': '小于等于',
  'in': '是其中之一（逗号分隔）',
  'contains': '包含',
  'prefix': '开头是',
  'between': '介于',
  'empty': '为空',
  'not_empty': '不为空',
}

const GRAN_LEN = { year: 4, month: 7, day: 10 } as const
const GRAN_LABEL = { year: '年', month: '月', day: '日' } as const

export function q(id: string) {
  return `"${id.replace(/"/g, '""')}"`
}

function lit(v: string) {
  return `'${v.replace(/'/g, '\'\'')}'`
}

export function emptyModel(dataset = ''): BuilderModel {
  return { dataset, fields: [], measures: [], filters: [], sort: null, limit: 1000 }
}

/** 字段在结果中的名称 */
export function fieldAlias(f: { column: string, granularity: Granularity }) {
  return f.granularity ? `${f.column}（${GRAN_LABEL[f.granularity]}）` : f.column
}

export function measureAlias(m: BuilderModel['measures'][number]) {
  if (m.alias.trim())
    return m.alias.trim()
  return m.agg === 'count' && !m.column ? '行数' : `${m.column ?? ''}${AGG_LABEL[m.agg]}`
}

function fieldExpr(f: { column: string, granularity: Granularity }) {
  return f.granularity ? `left(${q(f.column)}, ${GRAN_LEN[f.granularity]})` : q(f.column)
}

function measureExpr(m: BuilderModel['measures'][number]) {
  const c = m.column ? q(m.column) : '*'
  switch (m.agg) {
    case 'count': return m.column ? `count(${c})` : 'count(*)'
    case 'count_distinct': return `count(DISTINCT ${c})`
    case 'sum': return `round(sum(${c}), 2)`
    case 'avg': return `round(avg(${c}), 2)`
    default: return `${m.agg}(${c})`
  }
}

function value(v: string, numeric: boolean) {
  const t = v.trim()
  return numeric && t !== '' && !Number.isNaN(Number(t)) ? t : lit(t)
}

function filterExpr(f: BuilderModel['filters'][number], numeric: boolean) {
  const c = q(f.column)
  switch (f.op) {
    case 'in': {
      const vals = f.value.split(/[,，\n]/).map(s => s.trim()).filter(Boolean)
      return vals.length ? `${c} IN (${vals.map(v => value(v, numeric)).join(', ')})` : ''
    }
    case 'contains': return `${c} ILIKE ${lit(`%${f.value.trim()}%`)}`
    case 'prefix': return `${c} LIKE ${lit(`${f.value.trim()}%`)}`
    case 'between': {
      const hi = f.value2.trim()
      // 日期写到日时把结束日补到当天结束（文本比较）
      const max = !numeric && /^\d{4}-\d{2}-\d{2}$/.test(hi) ? `${hi} 23:59:59` : hi
      return `${c} BETWEEN ${value(f.value, numeric)} AND ${value(max, numeric)}`
    }
    case 'empty': return `(${c} IS NULL OR CAST(${c} AS VARCHAR) = '')`
    case 'not_empty': return `(${c} IS NOT NULL AND CAST(${c} AS VARCHAR) <> '')`
    default: return `${c} ${f.op} ${value(f.value, numeric)}`
  }
}

/** 生成 SQL；数据集或内容不完整时返回空串 */
export function buildSql(m: BuilderModel, ds: DatasetInfo | undefined): string {
  if (!m.dataset || !ds)
    return ''
  const numeric = (col: string) => ['money', 'number'].includes(ds.columns.find(c => c.title === col)?.kind ?? '')
  const aggregate = m.measures.length > 0
  const select: string[] = m.fields.map(f => `${fieldExpr(f)}${f.granularity || aggregate ? ` AS ${q(fieldAlias(f))}` : ''}`)
  select.push(...m.measures.map(x => `${measureExpr(x)} AS ${q(measureAlias(x))}`))
  const lines = [`SELECT ${select.length ? select.join(',\n       ') : '*'}`, `FROM ${q(m.dataset)}`]
  const where = m.filters.filter(f => f.column).map(f => filterExpr(f, numeric(f.column))).filter(Boolean)
  if (where.length)
    lines.push(`WHERE ${where.join('\n  AND ')}`)
  if (aggregate && m.fields.length)
    lines.push(`GROUP BY ${m.fields.map((_, i) => i + 1).join(', ')}`)
  if (m.sort?.by)
    lines.push(`ORDER BY ${q(m.sort.by)}${m.sort.desc ? ' DESC' : ''}`)
  else if (aggregate && m.fields.length)
    lines.push(`ORDER BY ${m.fields.map((_, i) => i + 1).join(', ')}`)
  if (m.limit && m.limit > 0)
    lines.push(`LIMIT ${Math.floor(m.limit)}`)
  return lines.join('\n')
}

/** 排序可选的列（结果中的列名） */
export function sortOptions(m: BuilderModel, ds: DatasetInfo | undefined): string[] {
  if (m.measures.length || m.fields.length)
    return [...m.fields.map(f => (m.measures.length || f.granularity ? fieldAlias(f) : f.column)), ...m.measures.map(measureAlias)]
  return ds?.columns.map(c => c.title) ?? []
}
