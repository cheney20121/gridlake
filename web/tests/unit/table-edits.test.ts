import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { applyEdits, editCount, recordEdit, rowKey, sameValue } from '../../src/features/excel/datatable/edits.ts'

const cols = ['订单号', '店铺', '金额']

describe('rowKey / sameValue', () => {
  it('主键按文本拼接：数字与文本数字相同、空与 null 相同', () => {
    assert.equal(rowKey([1, 'A']), rowKey(['1', 'A']))
    assert.equal(rowKey([null, 'A']), rowKey(['', 'A']))
    assert.notEqual(rowKey(['1', '2']), rowKey(['12', '']), '分隔不会混淆')
  })

  it('sameValue：数字与数字文本、空与空串相同；公式不与值相同', () => {
    assert.ok(sameValue(7, '7'))
    assert.ok(sameValue(7.5, '7.50'))
    assert.ok(sameValue(null, ''))
    assert.ok(sameValue('A', 'A'))
    assert.ok(!sameValue('A', 'B'))
    assert.ok(!sameValue('=B2*2', 14))
    assert.ok(!sameValue('', 0), '空白与 0 不同')
  })
})

describe('applyEdits', () => {
  const rows = [['A1', 'A店', 100], ['A2', 'B店', 200], ['A3', 'A店', 300]]

  it('按主键覆盖，行顺序变了也对得上', () => {
    const edits = recordEdit(recordEdit(undefined, rowKey(['A2']), '金额', 999), rowKey(['A3']), '店铺', '改名店')
    const shuffled = [rows[2]!, rows[0]!, rows[1]!]
    const r = applyEdits(shuffled, cols, ['订单号'], edits)
    assert.deepEqual(r.rows, [['A3', '改名店', 300], ['A1', 'A店', 100], ['A2', 'B店', 999]])
    assert.equal(r.applied, 2)
    assert.deepEqual(r.edits, edits)
    assert.deepEqual(shuffled[2], ['A2', 'B店', 200], '不改传入的数据')
  })

  it('当前结果里没有的行：记录保留，等它再出现时生效', () => {
    const edits = recordEdit(undefined, rowKey(['A2']), '金额', 999)
    const first = applyEdits([rows[0]!, rows[2]!], cols, ['订单号'], edits)
    assert.equal(first.applied, 0)
    assert.deepEqual(first.edits, edits)
    assert.equal(applyEdits(rows, cols, ['订单号'], first.edits).rows[1]![2], 999)
  })

  it('手改的值与数据集里的一样（改回去了）：记录被清掉；数据集里的值变了而手改不同：仍覆盖', () => {
    let edits = recordEdit(undefined, rowKey(['A1']), '金额', 100)
    edits = recordEdit(edits, rowKey(['A2']), '金额', '150')
    const r = applyEdits(rows, cols, ['订单号'], edits)
    assert.deepEqual(r.rows[0], ['A1', 'A店', 100])
    assert.deepEqual(r.rows[1], ['A2', 'B店', '150'])
    assert.deepEqual(r.edits, { [rowKey(['A2'])]: { 金额: '150' } }, 'A1 的记录与数据集一致，已清掉')
    assert.equal(editCount(r.edits), 1)
  })

  it('联合主键；公式原样写回；显示列里没有的列名忽略', () => {
    const edits = recordEdit(recordEdit(undefined, rowKey(['A1', 'A店']), '金额', '=C2*1.1'), rowKey(['A1', 'A店']), '不存在的列', 1)
    const r = applyEdits(rows, cols, ['订单号', '店铺'], edits)
    assert.equal(r.rows[0]![2], '=C2*1.1')
    assert.equal(r.applied, 1)
  })

  it('主键列不在显示列里：无法认行，原样返回', () => {
    const edits = recordEdit(undefined, rowKey(['A1']), '金额', 5)
    const r = applyEdits(rows, ['店铺', '金额'].concat(), ['订单号'], edits)
    assert.equal(r.applied, 0)
    assert.equal(r.rows, rows)
    assert.deepEqual(r.edits, edits)
    assert.equal(applyEdits(rows, cols, [], edits).applied, 0, '没有主键')
  })
})
