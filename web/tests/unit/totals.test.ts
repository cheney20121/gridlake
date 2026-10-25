import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { defaultTotal, formatTotal, pickTotal, totalFuncs, totalLabel } from '../../src/components/excel/big-grid/totals.ts'

const col = (kind: string, extra: object = {}) => ({ key: 'k', title: 't', kind, width: 100, ...extra }) as never

describe('数据表汇总行', () => {
  it('按列类型给可选的汇总方式；数值列默认合计，其他列默认不汇总', () => {
    assert.deepEqual(totalFuncs('money').slice(0, 3), ['sum', 'average', 'median'])
    assert.deepEqual(totalFuncs('datetime'), ['min', 'max', 'count', 'empty', 'distinct'])
    assert.deepEqual(totalFuncs('boolean'), ['trues', 'count', 'empty'])
    assert.deepEqual(totalFuncs('text'), ['count', 'empty', 'distinct'])
    assert.equal(defaultTotal('integer'), 'sum')
    assert.equal(defaultTotal('text'), '')
    assert.equal(defaultTotal('integer', '单号'), '')
    assert.equal(defaultTotal('integer', '用户 ID'), '')
    assert.equal(defaultTotal('integer', 'user_id'), '')
    assert.equal(defaultTotal('integer', '数量'), 'sum')
    assert.equal(defaultTotal('money', '实付金额'), 'sum')
    assert.equal(totalLabel('min', 'datetime'), '最早')
    assert.equal(totalLabel('min', 'money'), '最小')
  })
  it('选过的方式列类型不再支持时回到默认；选了「不汇总」就不汇总', () => {
    assert.equal(pickTotal(col('money'), { k: 'average' }), 'average')
    assert.equal(pickTotal(col('text'), { k: 'sum' }), '')
    assert.equal(pickTotal(col('money'), { k: 'trues' }), 'sum')
    assert.equal(pickTotal(col('money'), { k: '' }), '')
    assert.equal(pickTotal(col('text'), { k: 'distinct' }), 'distinct')
  })
  it('显示：金额两位、计数整数、大整数加千分位、整数列的平均带小数、空为破折号', () => {
    assert.equal(formatTotal(1234.5, 'sum', col('money')), '1,234.50')
    assert.equal(formatTotal(1234, 'count', col('money')), '1,234')
    assert.equal(formatTotal('9007199254741003', 'sum', col('integer')), '9,007,199,254,741,003')
    assert.equal(formatTotal(2.5, 'average', col('integer')), '2.5')
    assert.equal(formatTotal(0.1234, 'average', col('number', { numFmt: 'percent' })), '12.34%')
    assert.equal(formatTotal(null, 'sum', col('money')), '—')
    assert.equal(formatTotal('2026-01-15 10:00:00', 'min', col('datetime')), '2026-01-15 10:00:00')
  })
})
