import { numberLocale } from '@/i18n'

/** 金额：两位小数、千分位（按界面语言） */
export function formatMoney(v: unknown): string {
  if (typeof v !== 'number')
    return ''
  return v.toLocaleString(numberLocale, { minimumFractionDigits: 2, maximumFractionDigits: 2 })
}
