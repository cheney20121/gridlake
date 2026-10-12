import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { compileRules, excelPattern, formatNumber, ruleMatches, safeColor } from '../../src/components/excel/big-grid/column-style.ts'
import { formatCell } from '../../src/components/excel/big-grid/grid-format.ts'

const col = (kind: string, extra: object = {}) => ({ key: 'k', title: 't', kind, width: 100, ...extra }) as never
const red = { color: '#C00000' }

describe('数据表列样式：数字格式', () => {
  it('整数 / 两位小数 / 百分比 / 万元 / 不分组', () => {
    assert.equal(formatNumber(1234567.891, 'int'), '1,234,568')
    assert.equal(formatNumber(1234.5, 'dec2'), '1,234.50')
    assert.equal(formatNumber(0.1234, 'percent'), '12.34%')
    assert.equal(formatNumber(123456, 'wan'), '12.35 万')
    assert.equal(formatNumber(-250000, 'wan'), '-25.00 万')
    assert.equal(formatNumber(2026, 'plain'), '2026')
    assert.equal(formatNumber(1234567, 'plain'), '1234567')
  })
  it('默认格式返回 null，由列类型决定；非有限数原样', () => {
    assert.equal(formatNumber(12, 'default'), null)
    assert.equal(formatNumber(12, undefined), null)
    assert.equal(formatNumber(Number.NaN, 'wan'), 'NaN')
  })
  it('formatCell：有数字格式时优先；文字与空值不受影响；没有设置时保持原来的显示', () => {
    assert.equal(formatCell(123456, col('money', { numFmt: 'wan' })), '12.35 万')
    assert.equal(formatCell(0.5, col('number', { numFmt: 'percent' })), '50.00%')
    assert.equal(formatCell(123456, col('money', { numFmt: 'default' })), '123,456.00')
    assert.equal(formatCell(1234.5, col('money')), '1,234.50')
    assert.equal(formatCell('店铺', col('text', { numFmt: 'wan' })), '店铺')
    assert.equal(formatCell(null, col('money', { numFmt: 'wan' })), '')
  })
  it('Excel 格式串：万元无法表达', () => {
    assert.deepEqual(['int', 'dec2', 'percent', 'plain', 'wan', 'default'].map(f => excelPattern(f as never)), ['#,##0', '#,##0.00', '0.00%', '0', null, null])
  })
})

describe('数据表列样式：条件格式规则', () => {
  it('比较：只比数字，文字与空值不命中', () => {
    const gt = { op: '>', value: 100, style: red } as const
    assert.equal(ruleMatches(gt, 100.01), true)
    assert.equal(ruleMatches(gt, 100), false)
    assert.equal(ruleMatches(gt, '200'), false)
    assert.equal(ruleMatches(gt, null), false)
    assert.equal(ruleMatches({ op: '<=', value: 5, style: red }, 5), true)
    assert.equal(ruleMatches({ op: '>=', value: 5, style: red }, 4.99), false)
    assert.equal(ruleMatches({ op: '<', value: 0, style: red }, -1), true)
  })
  it('区间含两端；负数；空值', () => {
    const b = { op: 'between', value: 1, value2: 9, style: red } as const
    assert.deepEqual([0.9, 1, 5, 9, 9.1].map(v => ruleMatches(b, v)), [false, true, true, true, false])
    assert.equal(ruleMatches({ op: 'negative', style: red }, -0.01), true)
    assert.equal(ruleMatches({ op: 'negative', style: red }, 0), false)
    assert.equal(ruleMatches({ op: 'negative', style: red }, '-1'), false)
    const e = { op: 'empty', style: red } as const
    assert.deepEqual([null, undefined, '', 0, ' '].map(v => ruleMatches(e, v as never)), [true, true, true, false, false])
  })
  it('等于 / 不等于：数字按数值比，文字按字面；不等于不含空值', () => {
    assert.equal(ruleMatches({ op: '=', value: 3, style: red }, 3), true)
    assert.equal(ruleMatches({ op: '=', value: 3, style: red }, '3'), false)
    assert.equal(ruleMatches({ op: '=', value: '已退款', style: red }, '已退款'), true)
    assert.equal(ruleMatches({ op: '!=', value: '已退款', style: red }, '已发货'), true)
    assert.equal(ruleMatches({ op: '!=', value: '已退款', style: red }, null), false)
    assert.equal(ruleMatches({ op: '!=', value: 3, style: red }, 4), true)
  })
  it('包含不区分大小写；等于要完全一致；空比较值不命中', () => {
    assert.equal(ruleMatches({ op: 'contains', value: '退款', style: red }, '部分退款中'), true)
    assert.equal(ruleMatches({ op: 'contains', value: 'abc', style: red }, 'xABCx'), true)
    assert.equal(ruleMatches({ op: 'contains', value: '', style: red }, '任何'), false)
    assert.equal(ruleMatches({ op: 'contains', value: 'a', style: red }, null), false)
    assert.equal(ruleMatches({ op: 'equals', value: 'A', style: red }, 'a'), false)
    assert.equal(ruleMatches({ op: 'equals', value: 'A', style: red }, 'A'), true)
  })
  it('compileRules：第一条命中的生效；颜色只放行十六进制；没有样式的规则被丢弃；无有效规则返回 undefined', () => {
    const m = compileRules([
      { op: '>', value: 100, style: { color: '#C00000', bg: '#FDE8E8', bold: true } },
      { op: '>', value: 10, style: { bg: '#FFF2CC' } },
    ])!
    assert.deepEqual(m(500), { color: '#C00000', bg: '#FDE8E8', bold: true })
    assert.deepEqual(m(50), { color: undefined, bg: '#FFF2CC', bold: undefined })
    assert.equal(m(5), null)
    assert.equal(compileRules(undefined), undefined)
    assert.equal(compileRules([]), undefined)
    assert.equal(compileRules([{ op: '>', value: 1, style: {} }]), undefined)
    const unsafe = compileRules([{ op: '>', value: 1, style: { color: 'red', bg: 'url(javascript:1)' } as never }])
    assert.equal(unsafe, undefined, '非十六进制颜色被清洗掉后规则没有任何样式')
    assert.equal(safeColor('#c00'), '#c00')
    assert.equal(safeColor('#c00000;'), undefined)
    assert.equal(safeColor(12), undefined)
  })
})
