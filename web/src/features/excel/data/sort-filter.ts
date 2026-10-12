/**
 * 数据排序与筛选引擎
 * 基于 Univer API 读取/写入数据，支持多列排序与列筛选
 */

import type { ICellData, Nullable } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import { t } from '@/i18n'

/** 排序方向 */
export type SortOrder = 'asc' | 'desc'

/** 排序条件 */
export interface SortCondition {
  column: number
  order: SortOrder
}

/** 筛选条件 */
export interface FilterCondition {
  column: number
  values: unknown[]
}

/** 排序/筛选结果 */
export interface SortFilterResult {
  success: boolean
  message: string
  affectedRows: number
}

/** A1 引用（可带 $），排除函数名（如 LOG10(）和工作表名/标识符中的片段 */
const A1_REF = /(?<![\w.$])(\$?[A-Z]{1,3})(\$?)(\d+)(?![\w(])/g

/**
 * 把公式中「行号未加 $」的相对引用平移 rowDelta 行
 * 与 Excel 排序一致：整行移动后，=D2*E2 移到第 5 行会变成 =D5*E5
 */
export function shiftFormulaRows(formula: string, rowDelta: number): string {
  if (rowDelta === 0)
    return formula
  // 跳过字符串字面量，只处理引号外的部分
  return formula.split(/("(?:[^"]|"")*")/).map((part, i) => {
    if (i % 2 === 1)
      return part
    return part.replace(A1_REF, (match, col: string, absRow: string, row: string) =>
      absRow ? match : `${col}${Number(row) + rowDelta}`)
  }).join('')
}

function isBlank(v: unknown): boolean {
  return v === null || v === undefined || v === ''
}

export class SortFilterEngine {
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

  // ─── 排序 ─────────────────────────────────────────────

  /**
   * 对指定范围的数据进行排序
   * @param startRow 数据起始行（不含表头）
   * @param endRow 数据结束行
   * @param startCol 起始列
   * @param endCol 结束列
   * @param conditions 排序条件（支持多列优先级）
   */
  sort(
    startRow: number,
    endRow: number,
    startCol: number,
    endCol: number,
    conditions: SortCondition[],
  ): SortFilterResult {
    if (!this.univerAPI)
      return { success: false, message: t('未初始化'), affectedRows: 0 }

    const sheet = this.getActiveSheet()
    if (!sheet)
      return { success: false, message: t('无活动工作表'), affectedRows: 0 }

    if (conditions.length === 0)
      return { success: false, message: t('未指定排序列'), affectedRows: 0 }

    if (startRow >= endRow)
      return { success: false, message: t('无数据可排序'), affectedRows: 0 }

    const rowCount = endRow - startRow + 1
    const colCount = endCol - startCol + 1
    const range = sheet.getRange(startRow, startCol, rowCount, colCount)
    const worksheet = sheet.getSheet()

    // 排序键用原始值：getValues() 返回的是数字格式化后的显示文本（如 "1,905.71"），会按字符串排序
    const rawValues = range.getRawValues()
    const rows = rawValues.map((values, i) => ({
      values,
      srcRow: startRow + i,
      cells: Array.from({ length: colCount }, (_, c) => worksheet.getCellRaw(startRow + i, startCol + c)),
    }))

    // 稳定排序；空值无论升降序都排在最后（与 Excel 一致）
    rows.sort((a, b) => {
      for (const cond of conditions) {
        const colOffset = cond.column - startCol
        if (colOffset < 0 || colOffset >= colCount)
          continue

        const valA = a.values[colOffset]
        const valB = b.values[colOffset]
        if (isBlank(valA) || isBlank(valB)) {
          if (isBlank(valA) && isBlank(valB))
            continue
          return isBlank(valA) ? 1 : -1
        }
        const cmp = this.compareValues(valA, valB)
        if (cmp !== 0)
          return cond.order === 'asc' ? cmp : -cmp
      }
      return 0
    })

    // 整行搬移原始单元格（值 + 公式 + 样式），公式内同行相对引用随行号平移，避免公式被覆盖成静态值
    const sorted: Nullable<ICellData>[][] = rows.map((row, i) => {
      const rowDelta = startRow + i - row.srcRow
      return row.cells.map((cell) => {
        if (!cell)
          return { v: null }
        if (cell.f)
          return { ...cell, f: shiftFormulaRows(cell.f, rowDelta), v: undefined }
        return { ...cell }
      })
    })
    range.setValues(sorted as ICellData[][])

    return {
      success: true,
      message: t('已按列 {0} 排序', [conditions.map(c => String.fromCharCode(65 + c.column)).join(', ')]),
      affectedRows: endRow - startRow + 1,
    }
  }

  /**
   * 快捷单列排序
   */
  sortByColumn(column: number, order: SortOrder, dataStartRow = 1, dataEndRow?: number, startCol = 0, endCol?: number): SortFilterResult {
    const sheet = this.getActiveSheet()
    if (!sheet)
      return { success: false, message: t('无活动工作表'), affectedRows: 0 }

    const endRow = dataEndRow ?? sheet.getLastRow()
    const endC = endCol ?? sheet.getLastColumn()

    return this.sort(dataStartRow, endRow, startCol, endC, [{ column, order }])
  }

  // ─── 筛选 ─────────────────────────────────────────────

  /**
   * 对指定列进行筛选
   * 将不匹配的行隐藏（与 Excel 自动筛选一样隐藏整行，不修改行高）
   * @param column 要筛选的列
   * @param values 保留的值列表
   * @param startRow 数据起始行
   * @param endRow 数据结束行
   */
  filter(
    column: number,
    values: unknown[],
    startRow: number = 1,
    endRow?: number,
  ): SortFilterResult {
    if (!this.univerAPI)
      return { success: false, message: t('未初始化'), affectedRows: 0 }

    const sheet = this.getActiveSheet()
    if (!sheet)
      return { success: false, message: t('无活动工作表'), affectedRows: 0 }

    const lastRow = endRow ?? sheet.getLastRow()
    if (values.length === 0) {
      // 清除筛选：恢复所有行
      return this.clearFilter(startRow, lastRow)
    }

    const valueSet = new Set(values.map(v => String(v)))
    let hiddenCount = 0

    // 重新筛选前先恢复全部行，避免叠加上一次的筛选结果
    sheet.showRows(startRow, lastRow - startRow + 1)
    for (let r = startRow; r <= lastRow; r++) {
      const cellValue = sheet.getRange(r, column).getValue()
      if (!valueSet.has(String(cellValue ?? ''))) {
        sheet.hideRows(r)
        hiddenCount++
      }
    }

    return {
      success: true,
      message: t('已筛选列 {0}，隐藏 {1} 行', [String.fromCharCode(65 + column), hiddenCount]),
      affectedRows: lastRow - startRow + 1 - hiddenCount,
    }
  }

  /** 清除筛选，恢复所有隐藏行 */
  clearFilter(startRow: number = 1, endRow?: number): SortFilterResult {
    const sheet = this.getActiveSheet()
    if (!sheet)
      return { success: false, message: t('无活动工作表'), affectedRows: 0 }

    const lastRow = endRow ?? sheet.getLastRow()

    if (lastRow >= startRow)
      sheet.showRows(startRow, lastRow - startRow + 1)

    return {
      success: true,
      message: t('已清除筛选'),
      affectedRows: lastRow - startRow + 1,
    }
  }

  /**
   * 获取指定列的不重复值（用于筛选面板）
   */
  getUniqueValues(column: number, startRow: number = 1, endRow?: number): { value: unknown, count: number }[] {
    const sheet = this.getActiveSheet()
    if (!sheet)
      return []

    const lastRow = endRow ?? sheet.getLastRow()
    const counts = new Map<string, { value: unknown, count: number }>()

    for (let r = startRow; r <= lastRow; r++) {
      const cell = sheet.getRange(r, column)
      const val = cell.getValue()
      const key = String(val ?? '')

      if (val === null || val === undefined || key === '') {
        // 跳过空值
      }
      else if (counts.has(key)) {
        counts.get(key)!.count++
      }
      else {
        counts.set(key, { value: val, count: 1 })
      }
    }

    return Array.from(counts.values()).sort((a, b) => b.count - a.count)
  }

  // ─── 工具方法 ───────────────────────────────────────────

  /** 比较两个单元格值 */
  private compareValues(a: unknown, b: unknown): number {
    // 空值排到最后
    if (a === null || a === undefined || a === '') {
      if (b === null || b === undefined || b === '')
        return 0
      return 1
    }
    if (b === null || b === undefined || b === '')
      return -1

    // 数字比较
    if (typeof a === 'number' && typeof b === 'number')
      return a - b

    // 字符串数字比较
    const numA = Number(a)
    const numB = Number(b)
    if (!Number.isNaN(numA) && !Number.isNaN(numB))
      return numA - numB

    // 日期比较 (ISO 格式)
    const strA = String(a)
    const strB = String(b)
    if (strA.match(/^\d{4}-\d{2}-\d{2}/) && strB.match(/^\d{4}-\d{2}-\d{2}/))
      return strA.localeCompare(strB)

    // 字符串比较
    return strA.localeCompare(strB, 'zh-CN')
  }
}

/** 单例 */
export const sortFilterEngine = new SortFilterEngine()
