import type { ValidationFormat, ValidationRule } from '@/api/dataset'
import { t } from '@/i18n'

/**
 * 校验规则的编辑草稿：对话框里每条规则一行，所有参数都是文本框；保存时转成接口的规则（[toRule]），
 * 读回来时转成草稿（[fromRule]）
 */
export type RuleKind = ValidationRule['kind']

export interface DraftRule {
  column: string
  kind: RuleKind
  min: string
  max: string
  format: ValidationFormat
  pattern: string
  /** 允许的取值：逗号、顿号或换行分隔 */
  values: string
}

export const RULE_KINDS: { value: RuleKind, label: string }[] = [
  { value: 'required', label: t('必填') },
  { value: 'unique', label: t('不能重复') },
  { value: 'range', label: t('取值范围') },
  { value: 'format', label: t('格式') },
  { value: 'allowed', label: t('只能是') },
]

export const FORMATS: { value: ValidationFormat, label: string }[] = [
  { value: 'phone', label: t('手机号') },
  { value: 'email', label: t('邮箱') },
  { value: 'date', label: t('日期') },
  { value: 'digits', label: t('纯数字') },
  { value: 'idcard', label: t('身份证号') },
  { value: 'regex', label: t('自定义正则') },
]

export function emptyDraft(column = ''): DraftRule {
  return { column, kind: 'required', min: '', max: '', format: 'phone', pattern: '', values: '' }
}

export function parseValues(text: string): string[] {
  return [...new Set(text.split(/[,，、\n]/).map(s => s.trim()).filter(Boolean))]
}

/** 范围的一端：能读成数字就按数字（数值列），否则原样（日期时间列）；空为不限 */
function bound(s: string): number | string | null {
  const v = s.trim()
  if (!v)
    return null
  const n = Number(v.replace(/,/g, ''))
  return Number.isFinite(n) && /^-?[\d,]+(?:\.\d+)?$/.test(v) ? n : v
}

export function toRule(d: DraftRule): ValidationRule {
  const column = d.column
  switch (d.kind) {
    case 'range': return { column, kind: 'range', min: bound(d.min), max: bound(d.max) }
    case 'format': return d.format === 'regex' ? { column, kind: 'format', format: 'regex', pattern: d.pattern.trim() } : { column, kind: 'format', format: d.format }
    case 'allowed': return { column, kind: 'allowed', values: parseValues(d.values) }
    case 'unique': return { column, kind: 'unique' }
    default: return { column, kind: 'required' }
  }
}

export function fromRule(r: ValidationRule): DraftRule {
  const d = emptyDraft(r.column)
  d.kind = r.kind
  if (r.kind === 'range') {
    d.min = r.min == null ? '' : String(r.min)
    d.max = r.max == null ? '' : String(r.max)
  }
  else if (r.kind === 'format') {
    d.format = r.format
    d.pattern = r.pattern ?? ''
  }
  else if (r.kind === 'allowed') {
    d.values = r.values.join('，')
  }
  return d
}
