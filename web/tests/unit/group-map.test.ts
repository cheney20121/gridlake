import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { GroupMap } from '../../src/components/excel/big-grid/group-map.ts'

describe('分类汇总的行号换算', () => {
  // 三组：2 行、3 行、1 行 → 显示行：d0 d1 S0 d2 d3 d4 S1 d5 S2
  const m = new GroupMap([2, 3, 1])
  it('总数', () => {
    assert.equal(m.dataTotal, 6)
    assert.equal(m.virtualTotal, 9)
  })
  it('显示行 → 数据行 / 小计，往返一致', () => {
    const layout = Array.from({ length: 9 }, (_, v) => {
      const r = m.at(v)
      return r.kind === 'data' ? `d${r.row}` : `S${r.group}`
    })
    assert.deepEqual(layout, ['d0', 'd1', 'S0', 'd2', 'd3', 'd4', 'S1', 'd5', 'S2'])
    for (let d = 0; d < 6; d++) {
      const v = m.toVirtual(d)
      assert.deepEqual(m.at(v), { kind: 'data', row: d, group: m.at(v).group })
    }
    assert.deepEqual([0, 1, 2].map(g => m.subRow(g)), [2, 6, 8])
  })
  it('显示行区间里的数据行区间：跳过小计；只选了小计时为空', () => {
    assert.deepEqual(m.dataRange(0, 8), [0, 5])
    assert.deepEqual(m.dataRange(2, 3), [2, 2])
    assert.deepEqual(m.dataRange(1, 2), [1, 1])
    assert.equal(m.dataRange(2, 2), null)
    assert.deepEqual(m.dataRange(6, 7), [5, 5])
  })
  it('空的组、只有一组', () => {
    const one = new GroupMap([3])
    assert.deepEqual([0, 1, 2, 3].map(v => one.at(v).kind), ['data', 'data', 'data', 'sub'])
    const none = new GroupMap([])
    assert.equal(none.virtualTotal, 0)
  })
})
