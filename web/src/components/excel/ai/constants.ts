import { t } from '@/i18n'
/** AI 助手面板用的文案常量 */
export const TOOL_LABEL: Record<string, string> = {
  describe_dataset: t('查看数据集'),
  column_values: t('查看取值'),
  query_data: t('汇总查询'),
  compare_periods: t('两期对比'),
  find_anomalies: t('异常发现'),
  propose_plan: t('方案建议'),
  create_pivot: t('透视表建议'),
  create_chart: t('图表建议'),
  write_formula: t('公式建议'),
  add_calc_column: t('计算列建议'),
  set_table_view: t('数据表视图建议'),
  set_column_format: t('列样式建议'),
  create_data_table: t('新建数据表建议'),
  save_query_as_dataset: t('另存数据集建议'),
}

export const KIND_LABEL: Record<string, string> = { pivot: t('透视表'), chart: t('图表'), formula: '公式', calc: t('计算列'), table_view: t('数据表'), column_format: t('列样式'), table_create: t('新建数据表'), dataset_save: t('另存数据集'), plan: t('方案') }

export const EXAMPLES = [
  t('本月销售额比上月变化了多少？主要是哪些店铺造成的？'),
  t('各店铺今年每月的销售额，做成透视表'),
  t('画一张各店铺月度销售趋势图'),
  t('选中区域的数字是怎么算出来的？'),
  t('订单表只看9月的退款订单，按金额降序'),
  t('订单表的金额用万元显示，超过10万的标红'),
  t('把9月的退款订单放成一张表'),
  t('把各店铺的月度销售汇总存成一个数据集'),
]

/** 方案里某一步的标题 */
export function stepTitle(st: Record<string, any>) {
  switch (st.kind) {
    case 'formula': return t('{0} 写入公式', [st.cell])
    case 'calc': return t('计算列「{0}」', [st.name])
    default: return st.title ?? ''
  }
}
