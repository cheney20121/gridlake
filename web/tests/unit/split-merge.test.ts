import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { defaultNames, fitParts, mergeFormula, nameProblem, quoteText, splitFormulas } from '../../src/features/excel/dataset/split-merge.ts'

describe('拆分 / 合并列', () => {
  it('拆分：每段一个 SPLITPART，最后一列可以保留剩下的全部，可去空白', () => {
    assert.deepEqual(splitFormulas('收货地址', '-', 3, { trim: false, rest: false }), [
      '=SPLITPART([收货地址],"-",1)',
      '=SPLITPART([收货地址],"-",2)',
      '=SPLITPART([收货地址],"-",3)',
    ])
    assert.deepEqual(splitFormulas('收货地址', '-', 2, { trim: true, rest: true }), [
      '=TRIM(SPLITPART([收货地址],"-",1))',
      '=TRIM(TEXTAFTER([收货地址],"-",1))',
    ])
    assert.deepEqual(splitFormulas('a', ',', 1, { trim: false, rest: true }), ['=[a]'])
  })
  it('分隔符里的双引号按 Excel 写法转义', () => {
    assert.equal(quoteText('"'), '""""')
    assert.equal(splitFormulas('a', 'x"y', 1, { trim: false, rest: false })[0], '=SPLITPART([a],"x""y",1)')
  })
  it('合并：有分隔符用 TEXTJOIN，没有用 CONCAT', () => {
    assert.equal(mergeFormula(['省', '市'], '-', true), '=TEXTJOIN("-",TRUE,[省],[市])')
    assert.equal(mergeFormula(['省', '市'], '/', false), '=TEXTJOIN("/",FALSE,[省],[市])')
    assert.equal(mergeFormula(['省', '市'], '', true), '=CONCAT([省],[市])')
  })
  it('样例按列数整理；默认列名避开已有的；列名检查', () => {
    assert.deepEqual(fitParts(['a', 'b', 'c'], 2, '-', true), ['a', 'b-c'])
    assert.deepEqual(fitParts(['a', 'b', 'c'], 2, '-', false), ['a', 'b'])
    assert.deepEqual(fitParts(['a'], 3, '-', true), ['a', '', ''])
    assert.deepEqual(defaultNames('地址', 3, ['地址1']), ['地址2', '地址3', '地址4'])
    assert.equal(nameProblem(['省', '市'], ['地址']), '')
    assert.equal(nameProblem(['省', ' '], []), 'empty')
    assert.equal(nameProblem(['省', '省'], []), 'dup:省')
    assert.equal(nameProblem(['地址'], ['地址']), 'taken:地址')
    assert.equal(nameProblem(['a]'], []), 'char:a]')
  })
})
