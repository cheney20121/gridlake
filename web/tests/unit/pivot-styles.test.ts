import type { StyleDelta, StyleGrid } from '../../src/features/excel/pivot/pivot-styles.ts'
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { cellKey, extractStyles, styleDelta, stylesAt } from '../../src/features/excel/pivot/pivot-styles.ts'

/** 标题 + 表头 + 明细行 + 总计；一个标签列 + 每个值一列（无列字段） */
function grid(shops: string[], values = ['sum:金额:raw', 'sum:成本:raw']): StyleGrid {
  const rowIds = ['T', 'H:fields', ...shops.map(s => `data:${JSON.stringify([s])}`), 'total:[]']
  return {
    rowIds,
    dataRow: rowIds.map(id => id.startsWith('data:')),
    colIds: ['L:0', ...values.map(v => `${v}|Σ`)],
    valueOf: [null, ...values],
  }
}
const empty = (g: StyleGrid): (StyleDelta | null)[][] => g.rowIds.map(() => g.colIds.map(() => null))

describe('styleDelta', () => {
  it('只记不同的项；去掉的项为 null', () => {
    assert.equal(styleDelta({ bl: 1, n: { pattern: '0' } }, { bl: 1, n: { pattern: '0' } }), null)
    assert.deepEqual(styleDelta({ bl: 1, bg: { rgb: '#fff' } }, { bl: 0, cl: { rgb: '#f00' } }), { bl: 0, bg: null, cl: { rgb: '#f00' } })
  })
})

describe('extractStyles', () => {
  it('整列设成百分比 → 记到「值」，新出现的店铺也套用；总计格子按格记', () => {
    const g = grid(['A', 'B'])
    const d = empty(g)
    for (const r of [2, 3, 4]) d[r]![1] = { n: { pattern: '0.0%' } }
    const s = extractStyles(g, d, undefined)
    assert.deepEqual(s.value, { 'sum:金额:raw': { n: { pattern: '0.0%' } } })
    assert.deepEqual(s.cell, { [cellKey('total:[]', 'sum:金额:raw|Σ')]: { n: { pattern: '0.0%' } } })
    // 筛选后多出 C 店：明细格子都是百分比，成本列不受影响
    const g2 = grid(['A', 'B', 'C'])
    assert.deepEqual(stylesAt(s, g2, 4, 1), { n: { pattern: '0.0%' } })
    assert.equal(stylesAt(s, g2, 4, 2), null)
    assert.equal(stylesAt(s, g2, 1, 1), null, '表头不套用值级格式')
  })

  it('整行标红 → 记到「行」，换了位置（排序变化）仍跟着 A 店', () => {
    const g = grid(['A', 'B'])
    const d = empty(g)
    d[2]![1] = { cl: { rgb: '#f00' } }
    d[2]![2] = { cl: { rgb: '#f00' } }
    d[2]![0] = { bl: 1 }
    const s = extractStyles(g, d, undefined)
    assert.deepEqual(s.row, { 'data:["A"]': { cl: { rgb: '#f00' } } })
    assert.deepEqual(s.cell, { [cellKey('data:["A"]', 'L:0')]: { bl: 1 } })
    const g2 = grid(['B', 'A'])
    assert.deepEqual(stylesAt(s, g2, 3, 2), { cl: { rgb: '#f00' } })
    assert.deepEqual(stylesAt(s, g2, 3, 0), { bl: 1 })
    assert.equal(stylesAt(s, g2, 2, 2), null)
  })

  it('整列格式 + 列里一格另加底色：两级分开记，单格只记多出的部分', () => {
    const g = grid(['A', 'B', 'C'])
    const d = empty(g)
    for (const r of [2, 3, 4]) d[r]![1] = { n: { pattern: '0.0%' } }
    d[3]![1] = { n: { pattern: '0.0%' }, bg: { rgb: '#ff0' } }
    const s = extractStyles(g, d, undefined)
    assert.deepEqual(s.value, { 'sum:金额:raw': { n: { pattern: '0.0%' } } })
    assert.deepEqual(s.cell, { [cellKey('data:["B"]', 'sum:金额:raw|Σ')]: { bg: { rgb: '#ff0' } } })
    assert.deepEqual(stylesAt(s, g, 3, 1), { n: { pattern: '0.0%' }, bg: { rgb: '#ff0' } })
  })

  it('筛选到只剩一行：上次是整列格式的仍按整列记，放开筛选后其他行恢复', () => {
    const g = grid(['A', 'B'])
    const d = empty(g)
    for (const r of [2, 3]) d[r]![1] = { n: { pattern: '0.0%' } }
    const s1 = extractStyles(g, d, undefined)
    const g1 = grid(['A'])
    const d1 = empty(g1)
    d1[2]![1] = { n: { pattern: '0.0%' } }
    const s2 = extractStyles(g1, d1, s1)
    assert.deepEqual(s2.value, s1.value)
    assert.equal(s2.cell, undefined)
    assert.deepEqual(stylesAt(s2, grid(['A', 'B', 'C']), 4, 1), { n: { pattern: '0.0%' } })
  })

  it('用户把格式改回去：本次画出来的级别以本次为准（记录被清掉）', () => {
    const g = grid(['A', 'B'])
    const d = empty(g)
    for (const r of [2, 3]) d[r]![1] = { n: { pattern: '0.0%' } }
    const s1 = extractStyles(g, d, undefined)
    assert.deepEqual(extractStyles(g, empty(g), s1), {})
  })

  it('没有画出来的行 / 值字段保留上次的记录', () => {
    const g = grid(['A', 'B'])
    const d = empty(g)
    d[3]![2] = { bl: 1 } // B 店 × 成本
    d[2]![1] = { it: 1 } // A 店 × 金额
    const s1 = extractStyles(g, d, undefined)
    // 筛选掉 B 店、去掉成本值
    const g2 = grid(['A'], ['sum:金额:raw'])
    const d2 = empty(g2)
    d2[2]![1] = { it: 1 }
    const s2 = extractStyles(g2, d2, s1)
    assert.deepEqual(s2.cell?.[cellKey('data:["B"]', 'sum:成本:raw|Σ')], { bl: 1 })
    // A 店 × 金额只有一个格子：上次记在格上，这次仍在格上
    assert.deepEqual(stylesAt(s2, g2, 2, 1), { it: 1 })
  })
})
