import type { DrillTarget } from './ledger-workbook'
import type { AnalysisDimension, AnalysisMetricDef, AnalysisMetricKey, AnalysisRow } from '@/packs/ecommerce/api'
import { t } from '@/i18n'

/** 资金瀑布：结算实付起，逐项加减，落到资金净额 */
export const WATERFALL_KEYS: AnalysisMetricKey[] = ['gmv', 'freight', 'subsidy', 'refund', 'platformFee', 'commission', 'otherFee', 'other']

/** 占结算实付的比例（扣费类取正数，便于阅读「费率」） */
export function rateOf(row: AnalysisRow, key: AnalysisMetricKey): number | null {
  const gmv = row.values.gmv
  if (!gmv)
    return null
  const v = row.values[key]
  return ['refund', 'platformFee', 'commission', 'otherFee'].includes(key) ? -v / gmv : v / gmv
}

export function formatPct(v: number | null | undefined, digits = 1): string {
  if (v == null || !Number.isFinite(v))
    return '-'
  return `${(v * 100).toFixed(digits)}%`
}

export interface DrillContext {
  /** 下钻按钮 / 明细标题的范围描述，如「精选联盟」「达人 某某达人」 */
  scope: string
  shop?: string
  by?: AnalysisDimension
  /** 维度键（"" 表示该字段为空） */
  key?: string
}

/** 维度 → 明细筛选的固定上下文 */
function dimensionFilter(by: AnalysisDimension | undefined, key: string | undefined): DrillTarget['filter'] {
  if (by == null || key == null)
    return {}
  switch (by) {
    case 'channel': return { billingTypes: [key] }
    case 'talent': return { talentId: key }
    case 'product': return { productId: key }
    case 'order_month': return { orderMonth: key }
    case 'shop': return key ? { shop: key } : {}
    case 'entity': return key ? { entity: key } : {}
  }
}

/** 点开分析表中的数字：带同源规则下钻到明细，明细贡献合计应等于该数字 */
export function buildDrillTarget(metric: AnalysisMetricDef, value: number, ctx: DrillContext): DrillTarget {
  const filter = { ...dimensionFilter(ctx.by, ctx.key) }
  if (ctx.shop && !filter.shop)
    filter.shop = ctx.shop
  return {
    title: `${metric.label} · ${ctx.scope}${ctx.shop ? ` / ${ctx.shop}` : ''}`,
    filter,
    rule: metric.terms,
    expected: { label: t('分析表 {0}', [metric.label]), value, kind: 'contribution' },
  }
}

/** 名称后带上 ID 尾号，方便区分同名达人 / 商品 */
export function dimensionLabel(by: AnalysisDimension, row: AnalysisRow): string {
  if ((by === 'talent' || by === 'product') && row.key)
    return `${row.name}（…${row.key.slice(-4)}）`
  return row.name
}
