import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { cellKeys, retarget, shouldRelay, withoutPending } from '../../src/features/excel/collaboration/ops.ts'

const setValues = (cells: Record<string, Record<string, unknown>>, sheet = 's1') => ({ id: 'sheet.mutation.set-range-values', params: { unitId: 'wb', subUnitId: sheet, cellValue: cells } })

describe('shouldRelay', () => {
  it('工作表的变更转发', () => {
    assert.ok(shouldRelay('sheet.mutation.set-range-values', {}))
    assert.ok(shouldRelay('sheet.mutation.insert-row', undefined))
  })
  it('公式计算结果、只在本地生效的、从协作收到的不转发', () => {
    assert.ok(!shouldRelay('sheet.mutation.set-range-values', { fromFormula: true }))
    assert.ok(!shouldRelay('sheet.mutation.set-range-values', { onlyLocal: true }))
    assert.ok(!shouldRelay('sheet.mutation.set-range-values', { fromCollab: true }))
  })
  it('不是工作表变更（命令、操作、公式、文档）与跳过的不转发', () => {
    for (const id of ['sheet.command.set-range-values', 'sheet.operation.set-selections', 'formula.mutation.set-formula-calculation-result', 'doc.mutation.rich-text-editing', 'sheet.mutation.set-worksheet-permission-points', 'sheet.mutation.mark-dirty-filter-change'])
      assert.ok(!shouldRelay(id, {}), id)
  })
})

describe('cellKeys', () => {
  it('列出设置单元格动到的格子；其他变更没有', () => {
    assert.deepEqual(cellKeys(setValues({ 0: { 0: { v: 1 }, 2: { v: 2 } }, 5: { 1: { v: 3 } } })), ['s1|0|0', 's1|0|2', 's1|5|1'])
    assert.deepEqual(cellKeys({ id: 'sheet.mutation.insert-row', params: { subUnitId: 's1' } }), [])
  })
})

describe('withoutPending', () => {
  const op = setValues({ 0: { 0: { v: 'a' }, 1: { v: 'b' } }, 1: { 0: { v: 'c' } } })
  it('没有待回显的格子：原样', () => {
    assert.equal(withoutPending(op, new Set()), op)
    assert.equal(withoutPending(op, new Set(['s1|9|9'])), op)
  })
  it('去掉我已经改了的格子，保留其他', () => {
    const r = withoutPending(op, new Set(['s1|0|1']))!
    assert.deepEqual(r.params.cellValue, { 0: { 0: { v: 'a' } }, 1: { 0: { v: 'c' } } })
    assert.deepEqual((op.params.cellValue as Record<string, unknown>)[0], { 0: { v: 'a' }, 1: { v: 'b' } }, '不改原对象')
  })
  it('全部都是我改过的：整批丢弃；别的工作表的同位置格子不受影响', () => {
    assert.equal(withoutPending(op, new Set(['s1|0|0', 's1|0|1', 's1|1|0'])), null)
    assert.equal(withoutPending(op, new Set(['s2|0|0'])), op)
  })
  it('不是设置单元格的变更不处理', () => {
    const ins = { id: 'sheet.mutation.insert-row', params: { subUnitId: 's1' } }
    assert.equal(withoutPending(ins, new Set(['s1|0|0'])), ins)
  })
})

describe('retarget', () => {
  it('换成本地工作簿的 unitId；没有 unitId 的不加', () => {
    assert.equal(retarget(setValues({}), 'local').params.unitId, 'local')
    assert.deepEqual(retarget({ id: 'x', params: { a: 1 } }, 'local').params, { a: 1 })
  })
})
