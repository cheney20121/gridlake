import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { COLUMN_KINDS, formatBool, isNumericKind, kindLabel } from '../../src/features/excel/dataset/column-kinds.ts'
import { compatible } from '../../src/features/excel/template/match.ts'

describe('列类型', () => {
  it('整数、数值、金额都是数值列；编号、是否、日期不是', () => {
    assert.deepEqual(COLUMN_KINDS.filter(isNumericKind), ['integer', 'number', 'money'])
    assert.equal(isNumericKind(undefined), false)
  })
  it('名称与是否值的显示', () => {
    assert.equal(kindLabel('integer'), '整数')
    assert.equal(kindLabel('boolean'), '是/否')
    assert.equal(kindLabel('unknown'), '文本')
    // 筛选候选项是文本 'true' / 'false'，单元格是布尔值
    assert.deepEqual([true, 'false', 'x'].map(v => formatBool(v)), ['是', '否', 'x'])
  })
  it('模板列与数据集列的类型相容：数值类互通，是否列只对是否列', () => {
    assert.equal(compatible('number', 'integer'), true)
    assert.equal(compatible('money', 'integer'), true)
    assert.equal(compatible('integer', 'text'), false)
    assert.equal(compatible('boolean', 'text'), false)
    assert.equal(compatible('boolean', 'boolean'), true)
    assert.equal(compatible('text', 'boolean'), true)
  })
})
