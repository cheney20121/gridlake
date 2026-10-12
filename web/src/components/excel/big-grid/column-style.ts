import type { CellValue } from '@/api/dataset'
import { t } from '@/i18n'

/**
 * 数据表列的显示样式：数字格式 + 条件格式规则。
 * 只改"怎么显示"，不改数据；规则只看单元格自身的值，渲染时只对屏幕上的格子求值，与数据行数无关
 */

/** 数字格式：default 为按列类型的默认显示；wan 为除以 1 万并加「万」 */
export type NumFmt = 'default' | 'int' | 'dec2' | 'percent' | 'wan' | 'plain'

export const NUM_FMTS: NumFmt[] = ['default', 'int', 'dec2', 'percent', 'wan', 'plain']

export const NUM_FMT_LABEL: Record<NumFmt, string> = {
  default: t('默认'),
  int: t('整数'),
  dec2: t('两位小数'),
  percent: t('百分比'),
  wan: t('万元'),
  plain: t('不分组'),
}

export type RuleOp = '>' | '>=' | '<' | '<=' | '=' | '!=' | 'between' | 'contains' | 'equals' | 'empty' | 'negative'

export const RULE_OPS: RuleOp[] = ['>', '>=', '<', '<=', '=', '!=', 'between', 'contains', 'equals', 'empty', 'negative']

export interface RuleStyle {
  /** 文字颜色，#rgb / #rrggbb */
  color?: string
  /** 底色 */
  bg?: string
  bold?: boolean
}

export interface CellRule {
  op: RuleOp
  /** 比较值（数字或文字）；between 为下限 */
  value?: number | string
  /** between 的上限 */
  value2?: number
  style: RuleStyle
}

export interface ColumnStyle {
  numFmt?: NumFmt
  rules?: CellRule[]
}

/** 单元格求值结果：命中规则时的样式，没命中为 null */
export type RuleMatcher = (v: CellValue | undefined) => RuleStyle | null

const HEX = /^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i

/** 只接受十六进制颜色（颜色最终交给画布，不放任意字符串） */
export function safeColor(c: unknown): string | undefined {
  return typeof c === 'string' && HEX.test(c) ? c : undefined
}

const grouped0 = new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 0 })
const grouped2 = new Intl.NumberFormat('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
const percent2 = new Intl.NumberFormat('zh-CN', { style: 'percent', minimumFractionDigits: 2, maximumFractionDigits: 2 })
const plain = new Intl.NumberFormat('zh-CN', { useGrouping: false, maximumFractionDigits: 6 })

/** 按数字格式显示一个数；default 返回 null，由调用方按列类型处理 */
export function formatNumber(v: number, fmt: NumFmt | undefined): string | null {
  if (!Number.isFinite(v))
    return String(v)
  switch (fmt) {
    case 'int': return grouped0.format(v)
    case 'dec2': return grouped2.format(v)
    case 'percent': return percent2.format(v)
    case 'wan': return `${grouped2.format(v / 10_000)} 万`
    case 'plain': return plain.format(v)
    default: return null
  }
}

function asNumber(v: CellValue | undefined): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

function isEmpty(v: CellValue | undefined) {
  return v == null || v === ''
}

/** 一条规则是否命中某个值 */
export function ruleMatches(rule: CellRule, v: CellValue | undefined): boolean {
  switch (rule.op) {
    case 'empty':
      return isEmpty(v)
    case 'negative': {
      const n = asNumber(v)
      return n !== null && n < 0
    }
    case 'contains': {
      if (isEmpty(v) || rule.value == null || rule.value === '')
        return false
      return String(v).toLowerCase().includes(String(rule.value).toLowerCase())
    }
    case 'equals':
      return !isEmpty(v) && String(v) === String(rule.value ?? '')
    case 'between': {
      const n = asNumber(v)
      return n !== null && typeof rule.value === 'number' && typeof rule.value2 === 'number' && n >= rule.value && n <= rule.value2
    }
    case '=':
    case '!=': {
      const same = typeof rule.value === 'number'
        ? asNumber(v) === rule.value
        : !isEmpty(v) && String(v) === String(rule.value ?? '')
      return rule.op === '=' ? same : !isEmpty(v) && !same
    }
    default: {
      // > >= < <=：只比较数字
      const n = asNumber(v)
      if (n === null || typeof rule.value !== 'number')
        return false
      if (rule.op === '>')
        return n > rule.value
      if (rule.op === '>=')
        return n >= rule.value
      if (rule.op === '<')
        return n < rule.value
      return n <= rule.value
    }
  }
}

/**
 * 把一列的规则编译成求值函数：先清洗颜色、去掉没有任何样式的规则；按顺序，第一条命中的生效。
 * 没有有效规则时返回 undefined，绘制时整列跳过求值
 */
export function compileRules(rules: CellRule[] | undefined): RuleMatcher | undefined {
  const clean = (rules ?? [])
    .map(r => ({ ...r, style: { color: safeColor(r.style?.color), bg: safeColor(r.style?.bg), bold: r.style?.bold === true || undefined } }))
    .filter(r => r.style.color || r.style.bg || r.style.bold)
  if (!clean.length)
    return undefined
  return (v) => {
    for (const r of clean) {
      if (ruleMatches(r, v))
        return r.style
    }
    return null
  }
}

/** 单元格模式写入 Excel 时的数字格式串；wan 在 Excel 格式里无法除以 1 万，返回 null（不支持） */
export function excelPattern(fmt: NumFmt): string | null {
  switch (fmt) {
    case 'int': return '#,##0'
    case 'dec2': return '#,##0.00'
    case 'percent': return '0.00%'
    case 'plain': return '0'
    default: return null
  }
}
