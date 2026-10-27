import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { columnLetter, formatCell } from '../../src/components/excel/big-grid/grid-format.ts'
import { colAt, hitTest } from '../../src/components/excel/big-grid/grid-hit.ts'

const col = (kind: string, width = 100) => ({ key: 'k', title: 't', kind, width }) as never

describe('大数据表：单元格显示格式', () => {
  it('金额两位小数加千分位，数值不补小数，文本原样，空值为空', () => {
    assert.equal(formatCell(1234.5, col('money')), '1,234.50')
    assert.equal(formatCell(-0.5, col('money')), '-0.50')
    assert.equal(formatCell(12, col('number')), '12')
    assert.equal(formatCell(0.123456, col('number')), '0.1235')
    assert.equal(formatCell('店铺A', col('text')), '店铺A')
    assert.equal(formatCell(null, col('money')), '')
    assert.equal(formatCell('', col('text')), '')
  })
  it('整数加千分位不带小数，是否列显示是 / 否，超出安全整数的编号按文本原样显示', () => {
    assert.equal(formatCell(1234567, col('integer')), '1,234,567')
    assert.equal(formatCell(true, col('boolean')), '是')
    assert.equal(formatCell(false, col('boolean')), '否')
    assert.equal(formatCell('9007199254740993', col('integer')), '9007199254740993')
  })
  it('列字母：A … Z、AA、AZ、BA、ZZ、AAA', () => {
    assert.deepEqual([0, 25, 26, 51, 52, 701, 702].map(columnLetter), ['A', 'Z', 'AA', 'AZ', 'BA', 'ZZ', 'AAA'])
  })
})

describe('大数据表：鼠标命中测试', () => {
  // 行号列 48 宽；三列各 100；字母行 20 + 字段行 28 = 表头 48；第一行是第 10 行
  const g = { numW: 48, scrollLeft: 0, colX: [0, 100, 200, 300], colCount: 3, letterH: 20, headH: 48, firstRow: 10, total: 50 }

  it('列号、行号、单元格', () => {
    assert.deepEqual(hitTest(g, 10, 10), { area: 'corner' })
    assert.deepEqual(hitTest(g, 100, 60), { area: 'cell', row: 10, col: 0 })
    assert.deepEqual(hitTest(g, 48 + 150, 60 + 24 * 3), { area: 'cell', row: 13, col: 1 })
    assert.deepEqual(hitTest(g, 20, 60 + 24 * 2), { area: 'row', row: 12 })
    assert.deepEqual(hitTest(g, 48 + 50, 10), { area: 'col', col: 0 })
  })
  it('筛选按钮在字段行、列右侧 22px 内；字母行里同位置不算', () => {
    assert.deepEqual(hitTest(g, 48 + 100 - 10, 30), { area: 'filter', col: 0 })
    assert.deepEqual(hitTest(g, 48 + 100 - 10, 10), { area: 'col', col: 0 })
  })
  it('列右边缘 4px 内是调整列宽', () => {
    assert.deepEqual(hitTest(g, 48 + 100 + 3, 30), { area: 'resize', col: 0 })
    assert.deepEqual(hitTest(g, 48 + 200 - 4, 10), { area: 'resize', col: 1 })
  })
  it('横向滚动后按滚动后的位置换算', () => {
    const s = { ...g, scrollLeft: 150 }
    assert.equal(colAt(s, 48), 1)
    assert.deepEqual(hitTest(s, 48 + 10, 60), { area: 'cell', row: 10, col: 1 })
  })
  it('最后一列之外、最后一行之后什么也没命中', () => {
    assert.equal(colAt(g, 48 + 301), -1)
    assert.deepEqual(hitTest(g, 48 + 301, 60), { area: 'none' })
    assert.deepEqual(hitTest(g, 100, 48 + 24 * 41), { area: 'none' })
  })
})
