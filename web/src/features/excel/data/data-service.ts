import { t } from '@/i18n'
/**
 * Mock 数据生成器 — 迁移自早期的单文件演示页
 * 生成工业自动化订单数据，用于填充 Univer Worksheet
 */

const CUSTOMERS = [
  '示例客户01',
  '示例客户02',
  '示例客户03',
  '示例客户04',
  '示例客户05',
  '示例客户06',
  '示例客户07',
  '示例客户08',
  '示例客户09',
  '示例客户10',
  '示例客户11',
  '示例客户12',
  '示例客户13',
  '示例客户14',
  '示例客户15',
  '示例客户16',
  '示例客户17',
  '示例客户18',
  '示例客户19',
  '示例客户20',
  '示例客户21',
  '示例客户22',
  '示例客户23',
  '示例客户24',
  '示例客户25',
  '示例客户26',
  '示例客户27',
  '示例客户28',
  '示例客户29',
  '示例客户30',
  '示例客户31',
  '示例客户32',
  '示例客户33',
  '示例客户34',
  '示例客户35',
]

const PRODUCTS = [
  '示例产品01',
  '示例产品02',
  '示例产品03',
  '示例产品04',
  '示例产品05',
  '示例产品06',
  '示例产品07',
  '示例产品08',
  '示例产品09',
  '示例产品10',
  '示例产品11',
  '示例产品12',
  '示例产品13',
  '示例产品14',
  '示例产品15',
  '示例产品16',
  '示例产品17',
  '示例产品18',
  '示例产品19',
  '示例产品20',
]

const ORDER_STATUSES = ['待处理', '处理中', '已发货', '已完成', '已取消']

/** 表头定义 */
export const COL_HEADERS = [
  '订单编号',
  '客户名称',
  '产品名称',
  '数量',
  '单价',
  '金额',
  '订单日期',
  '订单状态',
]

/** 审批人列表 */
export const APPROVERS = [
  { name: '张经理', title: t('销售部经理') },
  { name: '李总监', title: t('运营部总监') },
  { name: '王总', title: t('副总裁') },
]

function randItem<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)] as T
}

function randInt(min: number, max: number): number {
  return Math.floor(Math.random() * (max - min + 1)) + min
}

function randFloat(min: number, max: number): number {
  return Math.round((Math.random() * (max - min) + min) * 100) / 100
}

function pad(n: number, w: number): string {
  return String(n).padStart(w, '0')
}

function formatDate(d: Date): string {
  const y = d.getFullYear()
  const m = pad(d.getMonth() + 1, 2)
  const day = pad(d.getDate(), 2)
  return `${y}-${m}-${day}`
}

/** 生成一行订单数据 (8 列) */
function generateRow(i: number): (string | number)[] {
  const today = new Date()
  const date = new Date(today)
  date.setDate(date.getDate() - randInt(0, 90))
  const qty = randInt(10, 500)
  const price = randFloat(50, 2000)

  return [
    `ORD-2026-${pad(i + 1, 4)}`,
    randItem(CUSTOMERS),
    randItem(PRODUCTS),
    qty,
    price,
    0, // 金额列由公式计算，先占位
    formatDate(date),
    randItem(ORDER_STATUSES),
  ]
}

/**
 * 生成 Mock 数据二维数组
 * @param count 数据行数
 * @returns [rows][cols] 二维数组
 */
export function generateMockData(count = 80): (string | number)[][] {
  const data: (string | number)[][] = []
  for (let i = 0; i < count; i++)
    data.push(generateRow(i))
  return data
}

/**
 * 生成 Univer Workbook 初始化数据
 * 包含表头行 + 数据行，金额列使用公式 =D{row}*E{row}
 */
export function generateWorkbookData(rowCount = 80) {
  const rows: (string | number | { f: string })[][] = []

  // 第 1 行：表头
  rows.push(COL_HEADERS)

  // 数据行
  for (let i = 0; i < rowCount; i++) {
    const row: (string | number | { f: string })[] = generateRow(i)
    const excelRow = i + 2 // 数据从第 2 行开始
    // 金额列 (index 5) 使用公式
    row[5] = { f: `=D${excelRow}*E${excelRow}` }
    rows.push(row)
  }

  return {
    sheetCount: 1,
    name: 'Sheet1',
    cellData: rows,
    rowCount: rowCount + 1,
    columnCount: 8,
  }
}

export { ORDER_STATUSES }
