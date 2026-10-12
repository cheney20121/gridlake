/**
 * 公式计算辅助 — 常用公式模板生成、公式函数文档
 * Univer 内置公式引擎，这里提供便捷的公式生成和帮助
 */

import type { FUniver } from '@univerjs/core/facade'

/** 内置公式函数列表 */
export interface FormulaDoc {
  name: string
  category: '数学' | '统计' | '文本' | '日期' | '逻辑'
  description: string
  syntax: string
  example: string
}

/** 常用公式函数文档 */
export const FORMULA_DOCS: FormulaDoc[] = [
  // 数学
  { name: 'SUM', category: '数学', description: '计算所有参数的和', syntax: 'SUM(number1, [number2], ...)', example: '=SUM(A1:A10)' },
  { name: 'PRODUCT', category: '数学', description: '计算所有参数的乘积', syntax: 'PRODUCT(number1, [number2], ...)', example: '=PRODUCT(A1:A5)' },
  { name: 'ROUND', category: '数学', description: '四舍五入到指定位数', syntax: 'ROUND(number, num_digits)', example: '=ROUND(A1, 2)' },
  { name: 'ABS', category: '数学', description: '返回数值的绝对值', syntax: 'ABS(number)', example: '=ABS(A1)' },
  { name: 'MOD', category: '数学', description: '返回两数相除的余数', syntax: 'MOD(number, divisor)', example: '=MOD(A1, 3)' },
  { name: 'POWER', category: '数学', description: '返回数的幂次方', syntax: 'POWER(number, power)', example: '=POWER(A1, 2)' },
  { name: 'SQRT', category: '数学', description: '返回数值的平方根', syntax: 'SQRT(number)', example: '=SQRT(A1)' },

  // 统计
  { name: 'AVERAGE', category: '统计', description: '计算参数的算术平均值', syntax: 'AVERAGE(number1, [number2], ...)', example: '=AVERAGE(A1:A10)' },
  { name: 'AVERAGEIF', category: '统计', description: '按条件计算平均值', syntax: 'AVERAGEIF(range, criteria, [average_range])', example: '=AVERAGEIF(A1:A10, ">100")' },
  { name: 'COUNT', category: '统计', description: '计算包含数字的单元格数量', syntax: 'COUNT(value1, [value2], ...)', example: '=COUNT(A1:A10)' },
  { name: 'COUNTA', category: '统计', description: '计算非空单元格数量', syntax: 'COUNTA(value1, [value2], ...)', example: '=COUNTA(A1:A10)' },
  { name: 'COUNTIF', category: '统计', description: '按条件计算单元格数量', syntax: 'COUNTIF(range, criteria)', example: '=COUNTIF(A1:A10, ">100")' },
  { name: 'MAX', category: '统计', description: '返回参数中的最大值', syntax: 'MAX(number1, [number2], ...)', example: '=MAX(A1:A10)' },
  { name: 'MIN', category: '统计', description: '返回参数中的最小值', syntax: 'MIN(number1, [number2], ...)', example: '=MIN(A1:A10)' },
  { name: 'MEDIAN', category: '统计', description: '返回参数的中位数', syntax: 'MEDIAN(number1, [number2], ...)', example: '=MEDIAN(A1:A10)' },
  { name: 'LARGE', category: '统计', description: '返回数据集中第k个最大值', syntax: 'LARGE(array, k)', example: '=LARGE(A1:A10, 3)' },
  { name: 'SMALL', category: '统计', description: '返回数据集中第k个最小值', syntax: 'SMALL(array, k)', example: '=SMALL(A1:A10, 3)' },
  { name: 'RANK', category: '统计', description: '返回数字在列表中的排名', syntax: 'RANK(number, ref, [order])', example: '=RANK(A1, A1:A10)' },

  // 文本
  { name: 'CONCATENATE', category: '文本', description: '连接多个文本字符串', syntax: 'CONCATENATE(text1, [text2], ...)', example: '=CONCATENATE(A1, " ", B1)' },
  { name: 'LEFT', category: '文本', description: '从左边截取指定长度文本', syntax: 'LEFT(text, [num_chars])', example: '=LEFT(A1, 3)' },
  { name: 'RIGHT', category: '文本', description: '从右边截取指定长度文本', syntax: 'RIGHT(text, [num_chars])', example: '=RIGHT(A1, 3)' },
  { name: 'MID', category: '文本', description: '从指定位置截取文本', syntax: 'MID(text, start_num, num_chars)', example: '=MID(A1, 2, 3)' },
  { name: 'LEN', category: '文本', description: '返回文本长度', syntax: 'LEN(text)', example: '=LEN(A1)' },
  { name: 'UPPER', category: '文本', description: '将文本转为大写', syntax: 'UPPER(text)', example: '=UPPER(A1)' },
  { name: 'LOWER', category: '文本', description: '将文本转为小写', syntax: 'LOWER(text)', example: '=LOWER(A1)' },
  { name: 'TRIM', category: '文本', description: '去除文本首尾空格', syntax: 'TRIM(text)', example: '=TRIM(A1)' },
  { name: 'SUBSTITUTE', category: '文本', description: '替换文本中的字符串', syntax: 'SUBSTITUTE(text, old_text, new_text, [instance_num])', example: '=SUBSTITUTE(A1, "old", "new")' },

  // 日期
  { name: 'TODAY', category: '日期', description: '返回当前日期', syntax: 'TODAY()', example: '=TODAY()' },
  { name: 'NOW', category: '日期', description: '返回当前日期时间', syntax: 'NOW()', example: '=NOW()' },
  { name: 'YEAR', category: '日期', description: '返回日期的年份', syntax: 'YEAR(serial_number)', example: '=YEAR(A1)' },
  { name: 'MONTH', category: '日期', description: '返回日期的月份', syntax: 'MONTH(serial_number)', example: '=MONTH(A1)' },
  { name: 'DAY', category: '日期', description: '返回日期的天', syntax: 'DAY(serial_number)', example: '=DAY(A1)' },
  { name: 'DATE', category: '日期', description: '返回指定日期', syntax: 'DATE(year, month, day)', example: '=DATE(2026, 1, 15)' },
  { name: 'DATEDIF', category: '日期', description: '计算两个日期差', syntax: 'DATEDIF(start_date, end_date, unit)', example: '=DATEDIF(A1, B1, "D")' },
  { name: 'WEEKDAY', category: '日期', description: '返回星期几', syntax: 'WEEKDAY(serial_number, [return_type])', example: '=WEEKDAY(A1)' },

  // 逻辑
  { name: 'IF', category: '逻辑', description: '条件判断', syntax: 'IF(logical_test, value_if_true, value_if_false)', example: '=IF(A1>100, "高", "低")' },
  { name: 'IFS', category: '逻辑', description: '多条件判断', syntax: 'IFS(test1, value1, [test2, value2], ...)', example: '=IFS(A1>100, "高", A1>50, "中", TRUE, "低")' },
  { name: 'AND', category: '逻辑', description: '所有条件为真', syntax: 'AND(logical1, [logical2], ...)', example: '=AND(A1>0, A1<100)' },
  { name: 'OR', category: '逻辑', description: '任一条件为真', syntax: 'OR(logical1, [logical2], ...)', example: '=OR(A1>100, A1<0)' },
  { name: 'NOT', category: '逻辑', description: '逻辑取反', syntax: 'NOT(logical)', example: '=NOT(A1>100)' },
  { name: 'IFERROR', category: '逻辑', description: '错误时返回指定值', syntax: 'IFERROR(value, value_if_error)', example: '=IFERROR(A1/B1, 0)' },
  { name: 'VLOOKUP', category: '逻辑', description: '垂直查找', syntax: 'VLOOKUP(lookup_value, table_array, col_index_num, [range_lookup])', example: '=VLOOKUP(A1, B1:D10, 2, FALSE)' },
]

export class FormulaHelper {
  private univerAPI: FUniver | null = null

  /** 绑定 Univer API */
  init(api: FUniver): void {
    this.univerAPI = api
  }

  /**
   * 在指定单元格写入公式
   */
  setFormula(row: number, col: number, formula: string): void {
    if (!this.univerAPI)
      return

    const workbook = this.univerAPI.getActiveWorkbook()
    if (!workbook)
      return

    const sheet = workbook.getActiveSheet()
    if (!sheet)
      return

    const range = sheet.getRange(row, col)
    // 确保 formula 以 = 开头
    const f = formula.startsWith('=') ? formula : `=${formula}`
    range.setValue(f)
  }

  /**
   * 在指定单元格写入常用聚合公式
   * @param funcName 函数名如 SUM, AVERAGE
   * @param startRow 数据起始行
   * @param endRow 数据结束行
   * @param col 目标列
   */
  setAggregateFormula(row: number, col: number, funcName: string, dataStartRow: number, dataEndRow: number, dataCol: number): void {
    const colLetter = String.fromCharCode(65 + dataCol)
    const formula = `${funcName}(${colLetter}${dataStartRow + 1}:${colLetter}${dataEndRow + 1})`
    this.setFormula(row, col, formula)
  }

  /**
   * 生成列范围公式字符串
   */
  static buildRangeFormula(funcName: string, col: number, startRow: number, endRow: number): string {
    const colLetter = String.fromCharCode(65 + col)
    return `=${funcName}(${colLetter}${startRow + 1}:${colLetter}${endRow + 1})`
  }

  /**
   * 按分类获取公式函数列表
   */
  static getFormulasByCategory(category: FormulaDoc['category']): FormulaDoc[] {
    return FORMULA_DOCS.filter(f => f.category === category)
  }

  /**
   * 搜索公式函数
   */
  static searchFormulas(keyword: string): FormulaDoc[] {
    const lower = keyword.toLowerCase()
    return FORMULA_DOCS.filter(f =>
      f.name.toLowerCase().includes(lower)
      || f.description.includes(keyword),
    )
  }
}

/** 单例 */
export const formulaHelper = new FormulaHelper()
