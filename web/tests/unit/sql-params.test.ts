import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { cleanValues, paramProblems, placeholderNames, syncDefs, valueError } from '../../src/features/excel/sql/params.ts'

describe('SQL 参数：占位符', () => {
  it('按出现顺序去重，允许空格', () => {
    assert.deepEqual(placeholderNames('WHERE 店铺 = {{店铺}} AND 日期 >= {{ 起 }} OR 店铺 = {{店铺}}'), ['店铺', '起'])
    assert.deepEqual(placeholderNames('SELECT 1'), [])
    assert.deepEqual(placeholderNames('{{}} {{ }} {{a'), [])
  })

  it('定义跟着占位符走：新的默认文本，没了的去掉，已有的保留类型与默认值', () => {
    const prev = [{ name: '起', kind: 'date' as const, default: '2026-09-01' }, { name: '旧', kind: 'number' as const }]
    assert.deepEqual(syncDefs(['店铺', '起'], prev), [{ name: '店铺', kind: 'text' }, { name: '起', kind: 'date', default: '2026-09-01' }])
  })
})

describe('SQL 参数：校验', () => {
  it('数字与日期格式', () => {
    assert.equal(valueError('number', '1,000.5'), '')
    assert.equal(valueError('number', 'abc'), '应为数字')
    assert.equal(valueError('date', '2026-09-01'), '')
    assert.equal(valueError('date', '2026/09/01'), '日期格式如 2026-09-01')
    assert.equal(valueError('date', '2026-02-30'), '没有这一天')
    assert.equal(valueError('text', 'a\'b'), '')
    assert.equal(valueError('number', '  '), '', '空值不在这里检查')
  })

  it('运行前列出缺值与格式问题，默认值算有值', () => {
    const defs = [{ name: '店铺', kind: 'text' as const }, { name: '起', kind: 'date' as const, default: '2026-09-01' }, { name: '下限', kind: 'number' as const }]
    assert.deepEqual(paramProblems(defs, { 店铺: 'A', 下限: 'x' }), ['参数「下限」应为数字'])
    assert.deepEqual(paramProblems(defs, {}), ['请填写参数「店铺」', '请填写参数「下限」'])
    assert.deepEqual(paramProblems(defs, { 店铺: 'A', 下限: '5', 起: '2026-13-01' }), ['参数「起」没有这一天'])
  })

  it('清理取值：只留还在用的、去掉空白', () => {
    assert.deepEqual(cleanValues([{ name: 'a', kind: 'text' }, { name: 'b', kind: 'text' }], { a: ' x ', b: ' ', c: 'y' }), { a: 'x' })
  })
})
