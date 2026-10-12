/** SQL 工作台的显示格式 */
export const KIND_LABEL: Record<string, string> = { text: '文本', id: '编号', number: '数值', money: '金额', datetime: '日期时间' }

/** 结果单元格的显示：金额两位小数、整数加千分位、空值为空 */
export function fmt(v: unknown, kind: string) {
  if (v == null)
    return ''
  if (typeof v === 'number') {
    if (kind === 'money')
      return v.toLocaleString('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
    return Number.isInteger(v) ? v.toLocaleString('zh-CN') : v.toLocaleString('zh-CN', { maximumFractionDigits: 6 })
  }
  return String(v)
}
