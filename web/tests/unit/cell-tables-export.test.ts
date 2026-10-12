import type { CellTableCandidate } from '../../src/features/excel/xlsx/cell-tables.ts'
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { toExcelFormula } from '../../src/features/excel/datatable/formula-syntax.ts'
import { planCellTables } from '../../src/features/excel/xlsx/cell-tables.ts'

function cand(over: Partial<CellTableCandidate> = {}): CellTableCandidate {
  return {
    sheetId: 's1',
    name: '订单格',
    row: 2,
    col: 1,
    extent: { rows: 4, cols: 3 },
    total: 3,
    edited: false,
    columns: ['订单号', '店铺', '金额'],
    header: ['订单号', '店铺', '金额'],
    occupied: [],
    ...over,
  }
}
const plan = (list: CellTableCandidate[]) => planCellTables(list, n => `X_${n}`)

describe('planCellTables', () => {
  it('写全、没手改、表头对得上：在原位置包成表格（表头行 + 3 行数据）', () => {
    const { exports, reasons } = plan([cand()])
    assert.equal(reasons.size, 0)
    assert.deepEqual(exports[0]!.range, { startRow: 2, startColumn: 1, endRow: 5, endColumn: 3 })
    assert.equal(exports[0]!.excelName, 'X_订单格')
    // 公式据此转成 Excel 写法
    const t = (name: string) => name === '订单格' ? { name: exports[0]!.excelName, columns: exports[0]!.columns } : null
    assert.equal(toExcelFormula('=TSUM("订单格[金额]", "店铺", B1)', t), '=SUMIFS(X_订单格[金额], X_订单格[店铺], B1)')
  })

  it('各种不能包的情形给出各自的原因（按表名小写）', () => {
    const cases: [Partial<CellTableCandidate>, RegExp][] = [
      [{ extent: undefined }, /还没有数据/],
      [{ extent: { rows: 1, cols: 3 } }, /还没有数据/],
      [{ total: 25000, extent: { rows: 10001, cols: 3 } }, /只写了前 10,000 行（共 25,000 行）/],
      [{ edited: true }, /手工调整/],
      [{ header: ['订单号', '门店', '金额'] }, /表头被改过/],
      [{ columns: ['订单号', '店铺'] }, /表头被改过/],
      [{ occupied: [{ startRow: 5, startColumn: 3, endRow: 9, endColumn: 6 }] }, /重叠/],
    ]
    for (const [over, re] of cases) {
      const { exports, reasons } = plan([cand({ ...over, name: 'ABC' })])
      assert.equal(exports.length, 0, String(re))
      assert.match(reasons.get('abc') ?? '', re)
    }
  })

  it('不重叠的表格不影响；没有 total 时按写入的行数算', () => {
    const { exports } = plan([cand({ total: undefined, occupied: [{ startRow: 6, startColumn: 1, endRow: 9, endColumn: 3 }] })])
    assert.equal(exports.length, 1)
  })
})
