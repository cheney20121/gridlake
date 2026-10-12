import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { formatRowCount } from '../../src/utils/format-count.ts'

describe('行数简写', () => {
  it('万以内写完整数字', () => {
    assert.equal(formatRowCount(0), '0')
    assert.equal(formatRowCount(6), '6')
    assert.equal(formatRowCount(9999), '9,999')
  })
  it('1 万 ~ 10 万保留一位小数，去掉多余的 .0', () => {
    assert.equal(formatRowCount(10_000), '1万')
    assert.equal(formatRowCount(12_345), '1.2万')
    assert.equal(formatRowCount(99_999), '10万')
  })
  it('10 万 ~ 1 亿用整数万，千位加逗号', () => {
    assert.equal(formatRowCount(200_000), '20万')
    assert.equal(formatRowCount(1_000_000), '100万')
    assert.equal(formatRowCount(99_990_000), '9,999万')
  })
  it('1 亿以上写成亿（1 亿行不是「10000万」）', () => {
    assert.equal(formatRowCount(100_000_000), '1亿')
    assert.equal(formatRowCount(123_000_000), '1.2亿')
    assert.equal(formatRowCount(10_000_000_000), '100亿')
  })
})
