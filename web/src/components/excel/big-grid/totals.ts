import type { GridColumn } from './types'
import type { CellValue, TotalFunc } from '@/api/dataset'
import { isNumericKind } from '@/features/excel/dataset/column-kinds'
import { t } from '@/i18n'
import { formatCell, integer } from './grid-format'

/**
 * 数据表底部的汇总行：每列选一种汇总方式，服务端按当前筛选对整个结果集计算（不是只算屏幕上的行）。
 * 可选的方式与服务端 dataset::total_funcs 一致
 */
export function totalFuncs(kind: string): TotalFunc[] {
  if (isNumericKind(kind))
    return ['sum', 'average', 'median', 'min', 'max', 'count', 'empty', 'distinct']
  if (kind === 'datetime')
    return ['min', 'max', 'count', 'empty', 'distinct']
  if (kind === 'boolean')
    return ['trues', 'count', 'empty']
  return ['count', 'empty', 'distinct']
}

export function totalLabel(func: TotalFunc, kind: string): string {
  switch (func) {
    case 'sum': return t('合计')
    case 'average': return t('平均')
    case 'median': return t('中位数')
    case 'min': return kind === 'datetime' ? t('最早') : t('最小')
    case 'max': return kind === 'datetime' ? t('最晚') : t('最大')
    case 'count': return t('非空')
    case 'empty': return t('空值')
    case 'distinct': return t('不同值')
    case 'trues': return t('「是」')
  }
}

/** 看起来是编号的列（单号、商品编号、用户 ID）：存成整数也不该求和 */
const ID_TITLE = /(?:号|编码|(?:^|[\s_-])id)$/i

/** 没选过时的默认：数值列合计（编号列除外），其他列不汇总 */
export function defaultTotal(kind: string, title = ''): TotalFunc | '' {
  return isNumericKind(kind) && !ID_TITLE.test(title.trim()) ? 'sum' : ''
}

/** 某列实际用的汇总方式：选过且仍适用（列类型可能改过）就用选的，否则用默认；'' 为不汇总 */
export function pickTotal(col: GridColumn, saved: Record<string, TotalFunc | ''>): TotalFunc | '' {
  const s = saved[col.key]
  if (s === '')
    return ''
  return s && totalFuncs(col.kind).includes(s) ? s : defaultTotal(col.kind, col.title)
}

const COUNTS: TotalFunc[] = ['count', 'empty', 'distinct', 'trues']

/** 汇总值的显示：计数类按整数；合计等沿用列的格式（金额两位、数字格式）；整数列的平均 / 中位数保留小数 */
export function formatTotal(v: CellValue, func: TotalFunc, col: GridColumn): string {
  if (v == null || v === '')
    return '—'
  if (COUNTS.includes(func))
    return typeof v === 'number' ? integer.format(v) : String(v)
  // 超出 JS 安全整数的整数合计按文本返回，原样加千分位
  if (typeof v === 'string' && /^-?\d+$/.test(v))
    return BigInt(v).toLocaleString('zh-CN')
  if (col.kind === 'integer' && (func === 'average' || func === 'median') && !col.numFmt)
    return formatCell(v, { ...col, kind: 'number' })
  return formatCell(v, col)
}
