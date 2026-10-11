/**
 * 行列操作管理 — 插入/删除行、插入/删除列
 */

import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'

export class RowColManager {
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

  // ─── 行操作 ───────────────────────────────────────────

  /** 在指定行前插入一行 */
  insertRow(rowIndex: number): void {
    const sheet = this.getActiveSheet()
    sheet?.insertRows(rowIndex, 1)
  }

  /** 在指定行前插入多行 */
  insertRows(rowIndex: number, count: number): void {
    const sheet = this.getActiveSheet()
    sheet?.insertRows(rowIndex, count)
  }

  /** 在指定行后插入行 */
  insertRowAfter(rowIndex: number, count = 1): void {
    const sheet = this.getActiveSheet()
    sheet?.insertRowsAfter(rowIndex, count)
  }

  /** 删除指定行 */
  deleteRow(rowIndex: number, count = 1): void {
    const sheet = this.getActiveSheet()
    sheet?.deleteRows(rowIndex, count)
  }

  /** 设置行高 */
  setRowHeight(rowIndex: number, height: number): void {
    const sheet = this.getActiveSheet()
    sheet?.setRowHeight(rowIndex, height)
  }

  /** 批量设置行高 */
  setRowHeights(startRow: number, numRows: number, height: number): void {
    const sheet = this.getActiveSheet()
    sheet?.setRowHeights(startRow, numRows, height)
  }

  // ─── 列操作 ───────────────────────────────────────────

  /** 在指定列前插入一列 */
  insertColumn(colIndex: number): void {
    const sheet = this.getActiveSheet()
    sheet?.insertColumns(colIndex, 1)
  }

  /** 在指定列前插入多列 */
  insertColumns(colIndex: number, count: number): void {
    const sheet = this.getActiveSheet()
    sheet?.insertColumns(colIndex, count)
  }

  /** 在指定列后插入列 */
  insertColumnAfter(colIndex: number, count = 1): void {
    const sheet = this.getActiveSheet()
    sheet?.insertColumnsAfter(colIndex, count)
  }

  /** 删除指定列 */
  deleteColumn(colIndex: number, count = 1): void {
    const sheet = this.getActiveSheet()
    sheet?.deleteColumns(colIndex, count)
  }

  /** 设置列宽 */
  setColumnWidth(colIndex: number, width: number): void {
    const sheet = this.getActiveSheet()
    sheet?.setColumnWidth(colIndex, width)
  }

  /** 批量设置列宽 */
  setColumnWidths(startCol: number, numCols: number, width: number): void {
    const sheet = this.getActiveSheet()
    for (let c = startCol; c < startCol + numCols; c++) {
      sheet?.setColumnWidth(c, width)
    }
  }
}

/** 单例 */
export const rowColManager = new RowColManager()
