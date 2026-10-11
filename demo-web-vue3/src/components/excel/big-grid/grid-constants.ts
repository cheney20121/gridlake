/** 大数据表画布的尺寸、颜色、字体（BigGrid 的交互与 grid-draw 的绘制共用） */
export const ROW_H = 24
export const FIELD_H = 28
/** 浏览器单个元素高度有上限，滚动区高度超过它就按比例映射 */
export const MAX_SCROLL = 8_000_000
export const FILTER_BTN = 18

/** 字母行（A / B / C）高度；嵌在工作表里时不显示字母行 */
export const letterHeight = (hideLetters?: boolean) => (hideLetters ? 0 : 20)

export const COLORS = {
  grid: '#E1E1E1',
  headBg: '#F5F5F5',
  headText: '#5F5F5F',
  fieldText: '#262626',
  text: '#1F1F1F',
  negative: '#C00000',
  skeleton: '#F0F0F0',
  selFill: 'rgba(33, 115, 70, 0.10)',
  selBorder: '#217346',
  selHead: '#D3F0E0',
  selHeadText: '#0E5A31',
  accent: '#217346',
}
export const FONT = '13px -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif'
export const HEAD_FONT = '600 12px -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif'
export const SMALL_FONT = '11px -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif'
