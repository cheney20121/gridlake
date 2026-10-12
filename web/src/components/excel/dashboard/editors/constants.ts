import type { PivotAgg, PivotShow } from '@/api/dataset'
import { t } from '@/i18n'

/** 仪表盘部件编辑表单用的选项 */
export const AGGS: { value: PivotAgg, label: string }[] = [
  { value: 'sum', label: t('求和') },
  { value: 'count', label: t('计数') },
  { value: 'avg', label: t('平均') },
  { value: 'max', label: t('最大') },
  { value: 'min', label: t('最小') },
  { value: 'count_distinct', label: t('去重计数') },
]

export const SHOWS: { value: PivotShow, label: string }[] = [
  { value: 'raw', label: t('原值') },
  { value: 'pct_total', label: t('占合计') },
  { value: 'yoy', label: t('同比') },
  { value: 'mom', label: t('环比') },
]
