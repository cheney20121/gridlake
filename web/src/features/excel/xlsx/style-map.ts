import type { IStyleData } from '@univerjs/core'
import type { Alignment, Border, Borders, BorderStyle, Fill, Font, Style } from 'exceljs'

/**
 * Univer 单元格样式 ↔ Excel（exceljs）样式
 * 覆盖：字体（字体 / 字号 / 粗斜体 / 下划线 / 删除线 / 颜色）、填充、边框、对齐（水平 / 垂直 / 换行 / 旋转）、数字格式
 */

// ── 颜色 ──────────────────────────────────────────────

/** Office 默认主题色（Excel 文件中 theme 索引 0~9） */
const THEME = ['FFFFFF', '000000', 'E7E6E6', '44546A', '4472C4', 'ED7D31', 'A5A5A5', 'FFC000', '5B9BD5', '70AD47']

/** Excel 旧式索引色（indexed 8~63，0~7 同 8~15） */
const INDEXED = [
  '000000',
  'FFFFFF',
  'FF0000',
  '00FF00',
  '0000FF',
  'FFFF00',
  'FF00FF',
  '00FFFF',
  '800000',
  '008000',
  '000080',
  '808000',
  '800080',
  '008080',
  'C0C0C0',
  '808080',
  '9999FF',
  '993366',
  'FFFFCC',
  'CCFFFF',
  '660066',
  'FF8080',
  '0066CC',
  'CCCCFF',
  '000080',
  'FF00FF',
  'FFFF00',
  '00FFFF',
  '800080',
  '800000',
  '008080',
  '0000FF',
  '00CCFF',
  'CCFFFF',
  'CCFFCC',
  'FFFF99',
  '99CCFF',
  'FF99CC',
  'CC99FF',
  'FFCC99',
  '3366FF',
  '33CCCC',
  '99CC00',
  'FFCC00',
  'FF9900',
  'FF6600',
  '666699',
  '969696',
  '003366',
  '339966',
  '003300',
  '333300',
  '993300',
  '993366',
  '333399',
  '333333',
]

function clamp(n: number) {
  return Math.max(0, Math.min(255, Math.round(n)))
}

/** 主题色的明暗调整（tint > 0 变浅，< 0 变深） */
function applyTint(hex: string, tint: number): string {
  const rgb = [0, 2, 4].map(i => Number.parseInt(hex.slice(i, i + 2), 16))
  const out = rgb.map(c => tint < 0 ? clamp(c * (1 + tint)) : clamp(c + (255 - c) * tint))
  return out.map(c => c.toString(16).padStart(2, '0')).join('').toUpperCase()
}

/** exceljs 颜色 → #RRGGBB（无法识别返回 undefined） */
export function excelColorToHex(color: Partial<{ argb: string, theme: number, tint: number, indexed: number }> | undefined): string | undefined {
  if (!color)
    return undefined
  if (color.argb && /^[0-9a-f]{8}$/i.test(color.argb))
    return `#${color.argb.slice(2).toUpperCase()}`
  if (color.argb && /^[0-9a-f]{6}$/i.test(color.argb))
    return `#${color.argb.toUpperCase()}`
  if (color.theme != null && THEME[color.theme])
    return `#${color.tint ? applyTint(THEME[color.theme]!, color.tint) : THEME[color.theme]}`
  if (color.indexed != null) {
    const i = color.indexed < 8 ? color.indexed : color.indexed - 8
    if (INDEXED[i])
      return `#${INDEXED[i]}`
  }
  return undefined
}

/** #RGB / #RRGGBB / rgb(r,g,b) → exceljs ARGB */
export function hexToArgb(color: string | undefined | null | void): string | undefined {
  if (!color)
    return undefined
  const c = color.trim()
  const m = c.match(/^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/i)
  if (m)
    return `FF${[m[1], m[2], m[3]].map(x => clamp(Number(x)).toString(16).padStart(2, '0')).join('').toUpperCase()}`
  let hex = c.replace('#', '')
  if (hex.length === 3)
    hex = hex.split('').map(x => x + x).join('')
  return /^[0-9a-f]{6}$/i.test(hex) ? `FF${hex.toUpperCase()}` : undefined
}

// ── 边框 ──────────────────────────────────────────────

/** Univer BorderStyleTypes（下标即枚举值）↔ Excel 边框样式 */
const BORDER_STYLES: (BorderStyle | null)[] = [
  null,
  'thin',
  'hair',
  'dotted',
  'dashed',
  'dashDot',
  'dashDotDot',
  'double',
  'medium',
  'mediumDashed',
  'mediumDashDot',
  'mediumDashDotDot',
  'slantDashDot',
  'thick',
]

const H_ALIGN: Record<number, Alignment['horizontal']> = { 1: 'left', 2: 'center', 3: 'right', 4: 'justify', 5: 'justify', 6: 'distributed' }
const V_ALIGN: Record<number, Alignment['vertical']> = { 1: 'top', 2: 'middle', 3: 'bottom' }

type BorderSide = 't' | 'b' | 'l' | 'r'
const SIDES: [BorderSide, keyof Borders][] = [['t', 'top'], ['b', 'bottom'], ['l', 'left'], ['r', 'right']]

// ── Univer → Excel ────────────────────────────────────

export function univerToExcelStyle(s: IStyleData | null | undefined): Partial<Style> | null {
  if (!s)
    return null
  const out: Partial<Style> = {}

  const font: Partial<Font> = {}
  if (s.ff)
    font.name = s.ff
  if (s.fs)
    font.size = s.fs
  if (s.bl)
    font.bold = true
  if (s.it)
    font.italic = true
  if (s.ul?.s)
    font.underline = true
  if (s.st?.s)
    font.strike = true
  const fc = hexToArgb(s.cl?.rgb)
  if (fc)
    font.color = { argb: fc }
  if (Object.keys(font).length)
    out.font = font

  const bg = hexToArgb(s.bg?.rgb)
  if (bg)
    out.fill = { type: 'pattern', pattern: 'solid', fgColor: { argb: bg } } as Fill

  if (s.bd) {
    const border: Partial<Borders> = {}
    for (const [k, e] of SIDES) {
      const side = (s.bd as Partial<Record<BorderSide, { s: number, cl?: { rgb?: string | null } } | null>>)[k]
      const style = side ? BORDER_STYLES[side.s] : null
      if (side && style)
        (border as Record<string, Partial<Border>>)[e] = { style, color: { argb: hexToArgb(side.cl?.rgb) ?? 'FF000000' } }
    }
    if (Object.keys(border).length)
      out.border = border
  }

  const align: Partial<Alignment> = {}
  if (s.ht && H_ALIGN[s.ht])
    align.horizontal = H_ALIGN[s.ht]
  if (s.vt && V_ALIGN[s.vt])
    align.vertical = V_ALIGN[s.vt]
  if (s.tb === 3)
    align.wrapText = true
  if (s.tr?.a)
    align.textRotation = Math.max(-90, Math.min(90, s.tr.a))
  if (Object.keys(align).length)
    out.alignment = align

  if (s.n?.pattern)
    out.numFmt = s.n.pattern
  return Object.keys(out).length ? out : null
}

// ── Excel → Univer ────────────────────────────────────

export function excelToUniverStyle(st: Partial<Style> | undefined): IStyleData | null {
  if (!st)
    return null
  const out: IStyleData = {}
  const f = st.font
  if (f) {
    // Excel 会把默认字体（Calibri 11）写进每个样式，导入时去掉，沿用表格自身的默认字体
    if (f.name && f.name !== 'Calibri')
      out.ff = f.name
    if (f.size && f.size !== 11)
      out.fs = f.size
    if (f.bold)
      out.bl = 1
    if (f.italic)
      out.it = 1
    if (f.underline)
      out.ul = { s: 1 }
    if (f.strike)
      out.st = { s: 1 }
    const c = excelColorToHex(f.color as never)
    if (c && c !== '#000000')
      out.cl = { rgb: c }
  }
  const fill = st.fill as (Fill & { pattern?: string, fgColor?: never }) | undefined
  if (fill?.type === 'pattern' && fill.pattern && fill.pattern !== 'none') {
    const c = excelColorToHex(fill.fgColor)
    if (c)
      out.bg = { rgb: c }
  }
  if (st.border) {
    const bd: Record<string, { s: number, cl: { rgb: string } }> = {}
    for (const [k, e] of SIDES) {
      const side = st.border[e]
      const idx = side?.style ? BORDER_STYLES.indexOf(side.style) : -1
      if (idx > 0)
        bd[k] = { s: idx, cl: { rgb: excelColorToHex(side!.color as never) ?? '#000000' } }
    }
    if (Object.keys(bd).length)
      out.bd = bd as IStyleData['bd']
  }
  const a = st.alignment
  if (a) {
    const h = Object.entries(H_ALIGN).find(([, v]) => v === a.horizontal)?.[0]
    if (h)
      out.ht = Number(h)
    const v = Object.entries(V_ALIGN).find(([, x]) => x === a.vertical)?.[0]
    if (v)
      out.vt = Number(v)
    if (a.wrapText)
      out.tb = 3
    if (typeof a.textRotation === 'number' && a.textRotation)
      out.tr = { a: a.textRotation, v: 0 }
  }
  if (st.numFmt && st.numFmt !== 'General')
    out.n = { pattern: st.numFmt }
  return Object.keys(out).length ? out : null
}

// ── 尺寸 ──────────────────────────────────────────────

/** Excel 列宽单位（默认字体下「0」字符数）↔ 像素 */
export const excelWidthToPx = (w: number) => Math.round(w * 7 + 5)
export const pxToExcelWidth = (px: number) => Math.max(1, Math.round(((px - 5) / 7) * 100) / 100)
/** Excel 行高（磅）↔ 像素 */
export const ptToPx = (pt: number) => Math.round(pt / 0.75)
export const pxToPt = (px: number) => Math.round(px * 0.75 * 100) / 100
