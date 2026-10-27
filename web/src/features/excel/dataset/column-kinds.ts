import type { DatasetColumnKind } from '@/api/dataset'
import { t } from '@/i18n'

/**
 * 数据集列类型（与服务端 server/src/kinds.rs 一一对应）
 *
 * 判断「是不是数值列」一律用 isNumericKind，不要再写 kind === 'money' || kind === 'number'：新增类型时只改这里
 */
export const COLUMN_KINDS: DatasetColumnKind[] = ['text', 'id', 'integer', 'number', 'money', 'datetime', 'boolean']

/** 数值列：可以求和 / 平均，筛选按数值区间 */
export function isNumericKind(kind: string | null | undefined): boolean {
  return kind === 'integer' || kind === 'number' || kind === 'money'
}

/** 列类型的名称 */
export function kindLabel(kind: string): string {
  switch (kind) {
    case 'id': return t('编号')
    case 'integer': return t('整数')
    case 'number': return t('数值')
    case 'money': return t('金额')
    case 'datetime': return t('日期时间')
    case 'boolean': return t('是/否')
    default: return t('文本')
  }
}

/** 下拉框选项：全部列类型 */
export function kindOptions(): { value: DatasetColumnKind, label: string }[] {
  return COLUMN_KINDS.map(value => ({ value, label: kindLabel(value) }))
}

/** 是否列的显示：是 / 否（服务端下发 true / false，筛选候选项是文本 'true' / 'false'） */
export function formatBool(v: boolean | string): string {
  return v === true || v === 'true' ? t('是') : v === false || v === 'false' ? t('否') : String(v)
}
