import type { DatasetFilter, DatasetQuery } from '@/api/dataset'
import type { DrillTarget } from '@/features/excel/shop-ledger/ledger-workbook'
import { t } from '@/i18n'

/** 打开大数据表时的下钻上下文：固定筛选 + 规则 + 核对基准 */
export interface DatasetDrill {
  title: string
  /** 下钻固定的筛选（不在筛选标签里单独移除，「退出下钻」时整体清除） */
  filters: DatasetFilter[]
  rule?: DatasetQuery['rule']
  expected?: DrillTarget['expected']
}

function inFilter(column: string, values: string[] | undefined): DatasetFilter[] {
  return values?.length ? [{ op: 'in', column, values }] : []
}

/** 汇总表 / 分析表的下钻目标 → 资金流水数据集的查询条件 */
export function drillFromTarget(target: DrillTarget | null, period: string): DatasetDrill {
  const f = target?.filter ?? {}
  const filters: DatasetFilter[] = [
    ...inFilter('period', period ? [period] : undefined),
    ...inFilter('shop', f.shop ? [f.shop] : undefined),
    ...inFilter('商户主体名称', f.entity ? [f.entity] : undefined),
    ...inFilter('动账场景', f.scenes),
    ...inFilter('动账方向', f.directions),
    ...inFilter('计费类型', f.billingTypes),
    ...(f.talentId != null ? inFilter('达人ID', [f.talentId]) : []),
    ...(f.productId != null ? inFilter('商品ID', [f.productId]) : []),
    ...(f.orderMonth != null ? [{ op: 'prefix', column: '下单时间', value: f.orderMonth } as DatasetFilter] : []),
  ]
  return {
    title: target ? target.title : t('{0} 全部流水', [period]),
    filters,
    rule: target?.rule,
    expected: target?.expected,
  }
}
