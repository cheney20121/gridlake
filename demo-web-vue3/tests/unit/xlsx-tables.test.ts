import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import ExcelJS from 'exceljs'
import { excelTableName, exportTables, readExcelTables } from '../../src/features/excel/xlsx/tables.ts'

describe('excelTableName', () => {
  it('合法的中文名原样保留；空格等符号换成下划线；数字开头、像单元格地址的加前缀；重名加序号', () => {
    const used = new Set<string>()
    assert.equal(excelTableName('订单表', used), '订单表')
    assert.equal(excelTableName('订单 表-1', used), '订单_表_1')
    assert.equal(excelTableName('2026销售', used), '_2026销售')
    assert.equal(excelTableName('A1', used), '_A1')
    assert.equal(excelTableName('R', used), '_R')
    assert.equal(excelTableName('订单表', used), '订单表_2')
    assert.equal(excelTableName('订单表', used), '订单表_3')
    assert.equal(excelTableName('', used), '_表')
  })
})

describe('表格导出 → xlsx → 读回', () => {
  function sheet() {
    const wb = new ExcelJS.Workbook()
    const ws = wb.addWorksheet('S')
    ws.getCell('B2').value = '订单号'
    ws.getCell('C2').value = '金额'
    ws.getCell('B3').value = 'A1'
    ws.getCell('C3').value = 100
    ws.getCell('B4').value = 'A2'
    ws.getCell('C4').value = { formula: 'C3*2', result: 200 }
    return { wb, ws }
  }
  const range = { startRow: 1, startColumn: 1, endRow: 3, endColumn: 2 }

  it('名称、位置、列名、筛选按钮保留，单元格的公式不变', async () => {
    const { wb, ws } = sheet()
    const skipped: string[] = []
    const n = exportTables([{ name: '订单表', range, options: { showAutoFilter: true } }], ws, 'S', new Set(), skipped)
    assert.equal(n, 1)
    assert.deepEqual(skipped, [])
    const wb2 = new ExcelJS.Workbook()
    await wb2.xlsx.load(await wb.xlsx.writeBuffer())
    const ws2 = wb2.getWorksheet('S')!
    const tables = readExcelTables(ws2, 'S', skipped)
    assert.deepEqual(tables, [{ name: '订单表', range, showAutoFilter: true, note: undefined }])
    assert.deepEqual(ws2.getCell('C4').value, { formula: 'C3*2', result: 200 })
    assert.equal(ws2.getCell('B2').value, '订单号')
  })

  it('空标题、重复标题补成合法的列名；筛选按钮关闭', async () => {
    const { wb, ws } = sheet()
    ws.getCell('C2').value = '订单号'
    const skipped: string[] = []
    exportTables([{ name: 'T', range, options: { showAutoFilter: false } }], ws, 'S', new Set(), skipped)
    const wb2 = new ExcelJS.Workbook()
    await wb2.xlsx.load(await wb.xlsx.writeBuffer())
    const ws2 = wb2.getWorksheet('S')!
    assert.equal(ws2.getCell('B2').value, '订单号')
    assert.equal(ws2.getCell('C2').value, '订单号2', '重复的标题在表格里必须唯一')
    assert.equal(readExcelTables(ws2, 'S', skipped)[0]!.showAutoFilter, false)
  })

  it('没有标题行 / 只有标题行的不导出并给出原因；多个表格同名时自动改名', () => {
    const { ws } = sheet()
    const skipped: string[] = []
    const used = new Set<string>()
    assert.equal(exportTables([
      { name: 'A', range, options: { showHeader: false } },
      { name: 'B', range: { ...range, endRow: 1 } },
      { name: 'Sales', range },
      { name: 'Sales', range: { startRow: 1, startColumn: 5, endRow: 2, endColumn: 6 } },
    ], ws, 'S', used, skipped), 2)
    assert.equal(skipped.length, 2)
    assert.match(skipped[0]!, /没有标题行/)
    assert.match(skipped[1]!, /只有标题行/)
    assert.deepEqual([...used].sort(), ['sales', 'sales_2'])
  })
})

describe('读取 Excel 的表格', () => {
  it('汇总行不带入并给出说明；没有标题行的跳过', () => {
    const wb = new ExcelJS.Workbook()
    const ws = wb.addWorksheet('S')
    ws.addTable({ name: 'T1', ref: 'A1', headerRow: true, totalsRow: true, columns: [{ name: 'a', totalsRowLabel: '合计' }, { name: 'b', totalsRowFunction: 'sum' }], rows: [[1, 2], [3, 4]] })
    ws.addTable({ name: 'T2', ref: 'E1', headerRow: false, columns: [{ name: 'x' }, { name: 'y' }], rows: [[1, 2], [3, 4]] })
    const skipped: string[] = []
    const got = readExcelTables(ws, 'S', skipped)
    assert.deepEqual(got.map(t => [t.name, t.range.endRow, t.note]), [['T1', 2, '汇总行未带入']])
    assert.match(skipped.join(), /T2.*没有标题行/)
  })
})
