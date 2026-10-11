/**
 * 单元格样式管理 — 字体、颜色、对齐方式的统一管理
 */

import type { FUniver } from '@univerjs/core/facade'
import type { FRange, FWorksheet } from '@univerjs/sheets/facade'
import { HorizontalAlign, VerticalAlign } from '@univerjs/presets'

/** 字体样式配置 */
export interface FontStyle {
  fontSize?: number
  fontFamily?: string
  fontColor?: string | null
  fontWeight?: 'normal' | 'bold' | null
  italic?: boolean
  underline?: boolean
  strikethrough?: boolean
}

/** 对齐方式配置 */
export interface AlignmentStyle {
  horizontal?: 'left' | 'center' | 'right'
  vertical?: 'top' | 'middle' | 'bottom'
  wrapText?: boolean
}

/** 背景色配置 */
export interface BackgroundStyle {
  color?: string | null
}

/** 完整单元格样式 */
export interface CellStyleOptions extends FontStyle, AlignmentStyle, BackgroundStyle {}

/**
 * Univer facade 的水平对齐沿用 Google Apps Script 命名：'normal' 表示右对齐，不接受 'right'
 */
type FHorizontalAlignment = Parameters<FRange['setHorizontalAlignment']>[0]

const H_ALIGN_TO_FACADE: Record<NonNullable<AlignmentStyle['horizontal']>, FHorizontalAlignment> = {
  left: 'left',
  center: 'center',
  right: 'normal',
}

const H_ALIGN_FROM_CORE: Partial<Record<HorizontalAlign, AlignmentStyle['horizontal']>> = {
  [HorizontalAlign.LEFT]: 'left',
  [HorizontalAlign.CENTER]: 'center',
  [HorizontalAlign.RIGHT]: 'right',
}

const V_ALIGN_FROM_CORE: Partial<Record<VerticalAlign, AlignmentStyle['vertical']>> = {
  [VerticalAlign.TOP]: 'top',
  [VerticalAlign.MIDDLE]: 'middle',
  [VerticalAlign.BOTTOM]: 'bottom',
}

export class StyleManager {
  private univerAPI: FUniver | null = null

  /** 绑定 Univer API */
  init(api: FUniver): void {
    this.univerAPI = api
  }

  /** 获取当前活动工作表 */
  private getActiveSheet(): FWorksheet | null {
    const workbook = this.univerAPI?.getActiveWorkbook()
    if (!workbook)
      return null
    return workbook.getActiveSheet()
  }

  /** 获取指定范围的 FRange */
  private getRange(startRow: number, startCol: number, endRow?: number, endCol?: number): FRange | null {
    const sheet = this.getActiveSheet()
    if (!sheet)
      return null

    if (endRow !== undefined && endCol !== undefined) {
      return sheet.getRange(startRow, startCol, endRow - startRow + 1, endCol - startCol + 1)
    }
    return sheet.getRange(startRow, startCol)
  }

  // ─── 字体样式 ─────────────────────────────────────────

  /** 设置字体大小 */
  setFontSize(startRow: number, startCol: number, endRow?: number, endCol?: number, size?: number): void {
    const range = this.getRange(startRow, startCol, endRow, endCol)
    range?.setFontSize(size ?? null)
  }

  /** 设置字体族 */
  setFontFamily(startRow: number, startCol: number, endRow?: number, endCol?: number, fontFamily?: string): void {
    const range = this.getRange(startRow, startCol, endRow, endCol)
    range?.setFontFamily(fontFamily ?? null)
  }

  /** 设置字体颜色 */
  setFontColor(startRow: number, startCol: number, endRow?: number, endCol?: number, color?: string): void {
    const range = this.getRange(startRow, startCol, endRow, endCol)
    range?.setFontColor(color ?? null)
  }

  /** 设置字体粗细 */
  setFontWeight(startRow: number, startCol: number, endRow?: number, endCol?: number, weight?: 'normal' | 'bold'): void {
    const range = this.getRange(startRow, startCol, endRow, endCol)
    range?.setFontWeight(weight ?? null)
  }

  /** 切换粗体 */
  toggleBold(startRow: number, startCol: number, endRow?: number, endCol?: number): void {
    const range = this.getRange(startRow, startCol, endRow, endCol)
    if (!range)
      return
    const isBold = range.getCellStyleData()?.bl === 1
    range.setFontWeight(isBold ? 'normal' : 'bold')
  }

  // ─── 对齐方式 ─────────────────────────────────────────

  /** 设置水平对齐 */
  setHorizontalAlignment(startRow: number, startCol: number, endRow?: number, endCol?: number, alignment: NonNullable<AlignmentStyle['horizontal']> = 'left'): void {
    const range = this.getRange(startRow, startCol, endRow, endCol)
    range?.setHorizontalAlignment(H_ALIGN_TO_FACADE[alignment])
  }

  /** 设置垂直对齐 */
  setVerticalAlignment(startRow: number, startCol: number, endRow?: number, endCol?: number, alignment?: 'top' | 'middle' | 'bottom'): void {
    const range = this.getRange(startRow, startCol, endRow, endCol)
    range?.setVerticalAlignment(alignment ?? 'middle')
  }

  // ─── 背景色 ───────────────────────────────────────────

  /** 设置背景色 */
  setBackgroundColor(startRow: number, startCol: number, endRow?: number, endCol?: number, color?: string): void {
    const range = this.getRange(startRow, startCol, endRow, endCol)
    range?.setBackgroundColor(color ?? 'white')
  }

  // ─── 批量样式 ─────────────────────────────────────────

  /**
   * 一次性应用完整样式到指定范围
   */
  applyStyle(startRow: number, startCol: number, endRow: number, endCol: number, style: CellStyleOptions): void {
    const range = this.getRange(startRow, startCol, endRow, endCol)
    if (!range)
      return

    if (style.fontSize !== undefined)
      range.setFontSize(style.fontSize)
    if (style.fontFamily !== undefined)
      range.setFontFamily(style.fontFamily)
    if (style.fontColor !== undefined)
      range.setFontColor(style.fontColor)
    if (style.fontWeight !== undefined)
      range.setFontWeight(style.fontWeight)
    if (style.horizontal !== undefined)
      range.setHorizontalAlignment(H_ALIGN_TO_FACADE[style.horizontal])
    if (style.vertical !== undefined)
      range.setVerticalAlignment(style.vertical)
    if (style.color !== undefined)
      range.setBackgroundColor(style.color ?? 'white')

    // 只改样式不碰单元格值，避免把公式覆盖成静态值
    if (style.italic !== undefined)
      range.setFontStyle(style.italic ? 'italic' : 'normal')
    if (style.strikethrough)
      range.setFontLine('line-through')
    else if (style.underline)
      range.setFontLine('underline')
    else if (style.underline === false || style.strikethrough === false)
      range.setFontLine('none')
  }

  /**
   * 快速应用预设样式
   */
  applyPreset(startRow: number, startCol: number, endRow: number, endCol: number, preset: 'header' | 'highlight' | 'warning' | 'success' | 'default'): void {
    const presets: Record<string, CellStyleOptions> = {
      header: {
        fontWeight: 'bold',
        color: '#107C41',
        fontColor: '#FFFFFF',
        horizontal: 'center',
        vertical: 'middle',
      },
      highlight: {
        color: '#FEF3C7',
        fontWeight: 'bold',
      },
      warning: {
        fontColor: '#DC2626',
        color: '#FEE2E2',
      },
      success: {
        fontColor: '#16A34A',
        color: '#DCFCE7',
      },
      default: {
        color: null,
        fontColor: null,
        fontWeight: 'normal',
      },
    }

    const style = presets[preset]
    if (style)
      this.applyStyle(startRow, startCol, endRow, endCol, style)
  }

  /**
   * 获取单元格当前样式
   */
  getCellStyle(row: number, col: number): CellStyleOptions | null {
    const range = this.getRange(row, col)
    if (!range)
      return null

    // 读原始 IStyleData：颜色是 { rgb } 结构，需要取出字符串给颜色选择器
    const style = range.getCellStyleData()
    if (!style)
      return null

    return {
      fontSize: style.fs,
      fontFamily: style.ff ?? undefined,
      fontColor: style.cl?.rgb ?? null,
      color: style.bg?.rgb ?? null,
      fontWeight: style.bl === 1 ? 'bold' : 'normal',
      italic: style.it === 1,
      horizontal: style.ht != null ? H_ALIGN_FROM_CORE[style.ht] : undefined,
      vertical: style.vt != null ? V_ALIGN_FROM_CORE[style.vt] : undefined,
    }
  }
}

/** 单例 */
export const styleManager = new StyleManager()
