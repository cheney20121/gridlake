import assert from 'node:assert/strict'
import test from 'node:test'
import { allFields, fixedEntries, fixedText, withFixed } from '../../src/features/excel/writeback/fixed-columns.ts'

const fixed = { 填报单位: '店铺A', 期次: '2026-10' }

test('固定列排在表格里的列前面，列和值的顺序一致', () => {
  assert.deepEqual(allFields(['营收', '备注'], fixed), ['填报单位', '期次', '营收', '备注'])
  assert.deepEqual(withFixed([[100, '初稿'], [200, null]], fixed), [['店铺A', '2026-10', 100, '初稿'], ['店铺A', '2026-10', 200, null]])
})

test('没有固定列时原样返回（普通填报区不受影响）', () => {
  const rows = [[1, 2]]
  assert.deepEqual(allFields(['a', 'b']), ['a', 'b'])
  assert.deepEqual(allFields(['a', 'b'], {}), ['a', 'b'])
  assert.equal(withFixed(rows), rows)
  assert.equal(withFixed(rows, null), rows)
  assert.deepEqual(fixedEntries(undefined), [])
  assert.equal(fixedText(undefined), '')
})

test('说明文字', () => {
  assert.equal(fixedText(fixed), '填报单位 = 店铺A，期次 = 2026-10')
})

test('每一行的值个数与列数一致（服务端按位置对应）', () => {
  const cols = allFields(['营收'], fixed)
  for (const r of withFixed([[1], [2], [3]], fixed))
    assert.equal(r.length, cols.length)
})
