import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { autoConfirmText, formatMoney, summarizeSelection } from '../../src/components/excel/recon/recon-pure.ts'

/** 只比较期望里列出的字段 */
const pick = (actual: object, expected: object) => Object.fromEntries(Object.keys(expected).map(k => [k, (actual as Record<string, unknown>)[k]]))

describe('formatMoney', () => {
  it('千分位 + 两位小数，负数保留符号', () => {
    assert.equal(formatMoney(1234567.5), '1,234,567.50')
    assert.equal(formatMoney(-0.1), '-0.10')
    assert.equal(formatMoney(0), '0.00')
  })
})

describe('summarizeSelection', () => {
  it('按「分」合计，避免浮点误差：0.1 + 0.2 等于 0.3', () => {
    const s = summarizeSelection([0.1, 0.2], [0.3], 0)
    assert.equal(s.leftTotal, 0.3)
    assert.equal(s.diff, 0)
    assert.equal(s.balanced, true)
  })

  it('差额在容差内算平衡，超出不算', () => {
    assert.equal(summarizeSelection([100], [100.02], 0.02).balanced, true)
    assert.equal(summarizeSelection([100], [100.03], 0.02).balanced, false)
    assert.equal(summarizeSelection([100], [99.5], 0).diff, 0.5)
  })

  it('两边都要选才算 ready；只选一边不平衡', () => {
    assert.deepEqual(pick(summarizeSelection([100], [], 0), { ready: false, balanced: false, leftCount: 1, rightCount: 0 }), { ready: false, balanced: false, leftCount: 1, rightCount: 0 })
    assert.equal(summarizeSelection([], [], 0).ready, false)
  })

  it('一对多', () => {
    const s = summarizeSelection([300], [100, 120, 80], 0)
    assert.deepEqual(pick(s, { ready: true, balanced: true, leftCount: 1, rightCount: 3, diff: 0 }), { ready: true, balanced: true, leftCount: 1, rightCount: 3, diff: 0 })
  })
})

describe('autoConfirmText', () => {
  const base = { dryRun: true, exact: 0, tolerance: 0, sum: 0, groups: 0, matchedRows: 0, clearedBroken: 0, leftOpen: 5, rightOpen: 7, warnings: [] as string[] }
  const labels = { left: '银行', right: '账面' }

  it('没有可勾对的', () => {
    assert.ok(autoConfirmText(base, labels).includes('没有可以自动勾对的了'))
  })

  it('列出各轮的组数、总笔数和剩余', () => {
    const t = autoConfirmText({ ...base, exact: 300, tolerance: 6, sum: 12, groups: 318, matchedRows: 650, leftOpen: 28, rightOpen: 23 }, labels)
    assert.ok(t.includes('318 组'))
    assert.ok(t.includes('金额相同 300 组'))
    assert.ok(t.includes('容差内 6 组'))
    assert.ok(t.includes('合计相等的多笔 12 组'))
    assert.ok(t.includes('650 笔'))
    assert.ok(t.includes('银行 28 笔，账面 23 笔'))
  })

  it('失效的组与服务端提示也写出来', () => {
    const t = autoConfirmText({ ...base, exact: 1, groups: 1, matchedRows: 2, clearedBroken: 2, warnings: ['候选太多'] }, labels)
    assert.ok(t.includes('清理 2 个失效的勾对组'))
    assert.ok(t.includes('候选太多'))
    assert.ok(autoConfirmText({ ...base, clearedBroken: 3 }, labels).includes('清理 3 个'))
  })
})
