/** 明细表列定义 */

export type ColumnKind = 'time' | 'direction' | 'money' | 'id' | 'text'

export interface DetailColumnDef {
  key: string
  title: string
  width: number
  kind: ColumnKind
  /** 「常用列」模式下显示 */
  common?: boolean
  /** 固定在左侧 */
  fixed?: boolean
}

const money = (key: string, common = false, title = key): DetailColumnDef => ({ key, title, width: 108, kind: 'money', common })

export const CONTRIBUTION_KEY = '贡献金额'

/** 下钻时附加在最前的「贡献金额」列：每笔流水计入所选汇总项的金额 */
export const CONTRIBUTION_COLUMN: DetailColumnDef = { key: CONTRIBUTION_KEY, title: '贡献金额', width: 116, kind: 'money', common: true, fixed: true }

export const DETAIL_COLUMNS: DetailColumnDef[] = [
  { key: '动账时间', title: '动账时间', width: 154, kind: 'time', common: true, fixed: true },
  { key: '动账方向', title: '方向', width: 58, kind: 'direction', common: true, fixed: true },
  { key: '动账金额', title: '动账金额', width: 112, kind: 'money', common: true, fixed: true },
  { key: '动账场景', title: '动账场景', width: 180, kind: 'text', common: true },
  { key: '计费类型', title: '计费类型', width: 84, kind: 'text', common: true },
  { key: '订单号', title: '订单号', width: 170, kind: 'id', common: true },
  { key: '售后编号', title: '售后编号', width: 180, kind: 'id', common: true },
  { key: '商品名称', title: '商品名称', width: 240, kind: 'text', common: true },
  { key: '达人名称', title: '达人名称', width: 120, kind: 'text', common: true },
  money('订单实付应结', true),
  money('订单退款', true),
  money('实际平台补贴', true, '平台补贴'),
  money('平台服务费', true),
  money('佣金', true),
  money('招商服务费', true),
  money('站外推广费'),
  money('运费实付'),
  money('实际平台补贴_运费', false, '平台补贴_运费'),
  money('其他平台补贴'),
  money('以旧换新抵扣'),
  money('政府补贴平台垫资'),
  money('实际达人补贴', false, '达人补贴'),
  money('实际抖音支付补贴', false, '抖音支付补贴'),
  money('实际抖音月付营销补贴', false, '月付营销补贴'),
  money('银行补贴'),
  money('服务商佣金'),
  money('渠道分成'),
  money('其他分成'),
  money('免佣金额'),
  { key: '商户主体名称', title: '商户主体', width: 200, kind: 'text', common: true },
  { key: '店铺', title: '店铺', width: 130, kind: 'text', common: true },
  { key: '备注', title: '备注', width: 260, kind: 'text', common: true },
  { key: '动帐流水号', title: '流水号', width: 220, kind: 'id' },
  { key: '子订单号', title: '子订单号', width: 170, kind: 'id' },
  { key: '商品ID', title: '商品ID', width: 160, kind: 'id' },
  { key: '达人ID', title: '达人ID', width: 150, kind: 'id' },
  { key: '下单时间', title: '下单时间', width: 154, kind: 'time' },
  { key: '订单类型', title: '订单类型', width: 84, kind: 'text' },
  { key: '动账账户', title: '动账账户', width: 90, kind: 'text' },
  { key: '是否免佣', title: '是否免佣', width: 200, kind: 'text' },
]

/** 关键字搜索覆盖的字段（与后端 search_text 一致），命中处高亮 */
export const SEARCH_KEYS = new Set(['订单号', '子订单号', '动帐流水号', '售后编号', '商品名称', '达人名称', '备注'])

/** 分面维度（后端返回的 facets 键）→ 筛选字段 */
export const FACETS = [
  { dim: '动账场景', key: 'scenes' },
  { dim: '动账方向', key: 'directions' },
  { dim: '计费类型', key: 'billingTypes' },
  { dim: '商户主体名称', key: 'entities' },
  { dim: '店铺', key: 'shops' },
] as const

export type FacetKey = typeof FACETS[number]['key']

export function formatMoney(v: unknown): string {
  if (typeof v !== 'number')
    return ''
  return v.toLocaleString('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
}
