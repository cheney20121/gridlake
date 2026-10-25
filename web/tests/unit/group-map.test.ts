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
  it('折叠：只显示小计行；数据行跳到小计；折叠组的小计代表整组；取数跳过折叠的组', () => {
    // 第 2 组（3 行）折叠：d0 d1 S0 S1 d5 S2
    const c = new GroupMap([2, 3, 1], [false, true, false])
    assert.equal(c.virtualTotal, 6)
    const layout = Array.from({ length: 6 }, (_, v) => {
      const r = c.at(v)
      return r.kind === 'data' ? `d${r.row}` : `S${r.group}`
    })
    assert.deepEqual(layout, ['d0', 'd1', 'S0', 'S1', 'd5', 'S2'])
    assert.equal(c.toVirtual(3), 3, '折叠组里的行跳到它的小计')
    assert.equal(c.toVirtual(5), 4)
    assert.deepEqual(c.dataRange(3, 3), [2, 4], '选中折叠组的小计 = 整组')
    assert.deepEqual(c.dataRange(1, 4), [1, 5])
    assert.deepEqual(c.dataSpans(0, 5), [[0, 1], [5, 5]])
    assert.deepEqual(m.dataSpans(0, 8), [[0, 5]], '没有折叠时连成一段')
    assert.deepEqual(m.dataSpans(3, 4), [[2, 3]])
    const all = new GroupMap([1000000, 2000000], [true, true])
    assert.equal(all.virtualTotal, 2)
    assert.deepEqual(all.dataSpans(0, 1), [], '全部折叠：一行数据都不取')
  })
})
