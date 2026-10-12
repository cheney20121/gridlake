import type { ParamDef, ParamKind } from '@/api/sql'
import { t } from '@/i18n'

/** 参数化查询的纯函数：SQL 里的 {{名称}} 与参数定义、取值的对应（与服务端 sqlq/params.rs 同一套规则） */

export const KIND_OPTIONS: { value: ParamKind, label: string }[] = [
  { value: 'text', label: t('文本') },
  { value: 'number', label: t('数字') },
  { value: 'date', label: t('日期') },
]

/** SQL 里出现的占位符名称（去重，按出现顺序） */
export function placeholderNames(sql: string): string[] {
  const out: string[] = []
  for (const m of sql.matchAll(/\{\{([^{}]*)\}\}/g)) {
    const name = (m[1] ?? '').trim()
    if (name && !out.includes(name))
      out.push(name)
  }
  return out
}

/** 编辑 SQL 时让参数定义跟着占位符走：新出现的默认文本，没了的去掉，已有的保留类型 */
export function syncDefs(names: string[], prev: ParamDef[]): ParamDef[] {
  return names.map(name => prev.find(p => p.name === name) ?? { name, kind: 'text' })
}

/** 值的问题（空表示没问题）；空值交给「默认值 / 请填写」处理，这里只查格式 */
export function valueError(kind: ParamKind, value: string): string {
  const v = value.trim()
  if (!v)
    return ''
  if (kind === 'number') {
    const n = Number(v.replace(/,/g, ''))
    return Number.isFinite(n) ? '' : t('应为数字')
  }
  if (kind === 'date') {
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(v)
    if (!m)
      return t('日期格式如 2026-09-01')
    const d = new Date(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3])))
    return d.getUTCFullYear() === Number(m[1]) && d.getUTCMonth() === Number(m[2]) - 1 && d.getUTCDate() === Number(m[3]) ? '' : t('没有这一天')
  }
  return ''
}

/** 运行前：缺哪些参数（没填值也没默认值）与格式问题，各一句话 */
export function paramProblems(defs: ParamDef[], values: Record<string, string>): string[] {
  const out: string[] = []
  for (const d of defs) {
    const v = (values[d.name] ?? '').trim() || d.default?.trim() || ''
    if (!v) {
      out.push(t('请填写参数「{0}」', [d.name]))
      continue
    }
    const e = valueError(d.kind, v)
    if (e)
      out.push(t('参数「{0}」{1}', [d.name, e]))
  }
  return out
}

/** 只保留还在用的参数的取值，空值不保留 */
export function cleanValues(defs: ParamDef[], values: Record<string, string>): Record<string, string> {
  return Object.fromEntries(defs.flatMap(d => (values[d.name] ?? '').trim() ? [[d.name, values[d.name]!.trim()]] : []))
}
