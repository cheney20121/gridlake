import { t } from '@/i18n'
/**
 * 条件格式「图标集」与 Excel 的互转（纯函数，不依赖 Univer / exceljs）。
 *
 * Univer：规则里每个图标一条配置，**从高到低**排列——第 0 条是最高档（值 ≥ 阈值）的图标 iconId=0，依次往下，
 * 最后一条是兜底（阈值不起作用）；图标列表同样按从高到低（如 3Arrows：绿上箭头、黄右箭头、红下箭头）。
 * Excel：cfvo 阈值**从低到高**，第一个固定是最低档（通常 percent 0），第 j 个是第 j 档图标的下限；reverse 把图标顺序反过来。
 * 图标集名称两边相同（3Arrows、3TrafficLights1、5Quarters 等 20 种）。
 *
 * 做不到的返回原因，由调用方计入「未能导入 / 导出」：每个图标自定义（混用不同图标集、不按顺序）、图标个数与图标集不符。
 * exceljs 不读写 cfvo 的 gte 属性，所以「大于」与「大于等于」的区别丢失，一律按「大于等于」（只在值刚好等于阈值时有差别）
 */

export interface CfValue {
  type: string
  value?: number | string
}

export interface UniverIconConfig {
  operator: string
  value: CfValue
  iconType: string
  iconId: string
}

/** Excel 图标集名称 → 图标个数 */
export const ICON_SET_SIZE: Record<string, number> = {
  '3Arrows': 3,
  '3ArrowsGray': 3,
  '3Flags': 3,
  '3TrafficLights1': 3,
  '3TrafficLights2': 3,
  '3Signs': 3,
  '3Symbols': 3,
  '3Symbols2': 3,
  '3Stars': 3,
  '3Triangles': 3,
  '4Arrows': 4,
  '4ArrowsGray': 4,
  '4RedToBlack': 4,
  '4Rating': 4,
  '4TrafficLights': 4,
  '5Arrows': 5,
  '5ArrowsGray': 5,
  '5Rating': 5,
  '5Quarters': 5,
  '5Boxes': 5,
}

/** Univer 的兜底条阈值（与它自带的快速插入一致，计算时不起作用） */
const FALLBACK = Number.MAX_SAFE_INTEGER

export interface ExcelIconSetRule {
  type: 'iconSet'
  iconSet: string
  reverse: boolean
  showValue: boolean
  cfvo: CfValue[]
}

/** Univer 的图标集配置 → Excel 规则；做不到时返回原因 */
export function iconSetToExcel(config: UniverIconConfig[], isShowValue: boolean | undefined): ExcelIconSetRule | string {
  const n = config.length
  const type = config[0]?.iconType
  const size = type ? ICON_SET_SIZE[type] : undefined
  if (!type || !size)
    return t('图标集（Excel 没有这种图标）')
  if (config.some(c => c.iconType !== type))
    return t('图标集（混用了不同的图标）')
  if (n !== size)
    return t('图标集（图标个数与图标集不符）')
  const ids = config.map(c => Number(c.iconId))
  const natural = ids.every((id, k) => id === k)
  const reversed = ids.every((id, k) => id === n - 1 - k)
  if (!natural && !reversed)
    return t('图标集（图标的顺序被自定义）')
  if (config.slice(0, -1).some(c => c.operator !== 'greaterThanOrEqual' && c.operator !== 'greaterThan'))
    return t('图标集（阈值条件不是「大于等于」）')
  // Excel 的阈值从低到高：第一个是最低档（固定为 0%），其后是 Univer 从倒数第二条往前的阈值
  const cfvo: CfValue[] = [{ type: 'percent', value: 0 }]
  for (let k = n - 2; k >= 0; k--)
    cfvo.push(normalize(config[k]!.value))
  return { type: 'iconSet', iconSet: type, reverse: reversed && n > 1, showValue: isShowValue !== false, cfvo }
}

/** Excel 的图标集规则（exceljs 读出的）→ Univer 配置；做不到时返回原因 */
export function excelToIconSet(rule: { iconSet?: string, reverse?: boolean, showValue?: boolean, cfvo?: { type?: string, value?: number | string }[] }): { isShowValue: boolean, config: UniverIconConfig[] } | string {
  // exceljs 缺省的名字是「3TrafficLights」，Excel 里指 3TrafficLights1
  const name = !rule.iconSet || rule.iconSet === '3TrafficLights' ? '3TrafficLights1' : rule.iconSet
  const size = ICON_SET_SIZE[name]
  if (!size)
    return t('图标集「{0}」', [name])
  const cfvo = rule.cfvo ?? []
  if (cfvo.length !== size)
    return t('图标集「{0}」（阈值个数不对）', [name])
  const reverse = !!rule.reverse
  const config: UniverIconConfig[] = []
  for (let k = 0; k < size; k++) {
    const iconId = String(reverse ? size - 1 - k : k)
    if (k === size - 1)
      config.push({ operator: 'lessThanOrEqual', value: { type: 'percent', value: FALLBACK }, iconType: name, iconId })
    else
      config.push({ operator: 'greaterThanOrEqual', value: fromExcelValue(cfvo[size - 1 - k]), iconType: name, iconId })
  }
  return { isShowValue: rule.showValue !== false, config }
}

/** 公式阈值在 exceljs 里不带等号；Univer 里带 */
function fromExcelValue(v: { type?: string, value?: number | string } | undefined): CfValue {
  const t = v?.type ?? 'percent'
  if (t === 'min' || t === 'max')
    return { type: t }
  if (t === 'formula')
    return { type: 'formula', value: `=${String(v?.value ?? '')}` }
  return { type: t === 'number' ? 'num' : t, value: Number(v?.value ?? 0) }
}

function normalize(v: CfValue): CfValue {
  if (v.type === 'formula')
    return { type: 'formula', value: String(v.value ?? '').replace(/^=/, '') }
  if (v.type === 'min' || v.type === 'max' || v.value == null)
    return { type: v.type }
  return { type: v.type, value: Number(v.value) }
}
