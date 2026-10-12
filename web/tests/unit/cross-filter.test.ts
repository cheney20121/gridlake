import type { CrossFilter } from '../../src/features/excel/dashboard/cross-filter-pure.ts'
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { crossFilterConditions, nextCrossFilter } from '../../src/features/excel/dashboard/cross-filter-pure.ts'

const plain = { kind: 'dataset', datasetId: 'ds1', x: { column: '店铺' }, values: [{ column: '金额', agg: 'sum' }] } as never
const monthly = { kind: 'dataset', datasetId: 'ds1', x: { column: '下单时间', granularity: 'month' }, values: [{ column: '金额', agg: 'sum' }] } as never
const split = { kind: 'dataset', datasetId: 'ds1', x: { column: '店铺' }, values: [{ column: '金额', agg: 'sum' }], splitBy: '渠道' } as never

const click = (cur: CrossFilter | null, src: never, category: string, extra: { series?: string, additive?: boolean } = {}, chart = 'c1') => nextCrossFilter(cur, chart, src, { category, ...extra })
const vals = (f: CrossFilter | null) => f && f.values

describe('nextCrossFilter：单击', () => {
  it('选中一个分类；再点同一个取消；点另一个替换', () => {
    const a = click(null, plain, 'A')!
    assert.deepEqual(vals(a), ['A'])
    assert.equal(a.label, '店铺：A')
    assert.equal(click(a, plain, 'A'), null)
    assert.deepEqual(vals(click(a, plain, 'B')), ['B'])
  })

  it('点别的图表：替换（每个工作表只有一个联动）', () => {
    const a = click(null, plain, 'A', {}, 'c1')!
    const b = click(a, plain, 'A', {}, 'c2')!
    assert.equal(b.chartId, 'c2')
    assert.deepEqual(vals(b), ['A'])
  })

  it('空白分类 "（空）" 记为空串，显示「（空白）」', () => {
    const f = click(null, plain, '（空）')!
    assert.deepEqual(f.values, [''])
    assert.equal(f.label, '店铺：（空白）')
  })
})

describe('nextCrossFilter：Ctrl 多选', () => {
  it('在已选的基础上增减；减到没有则取消', () => {
    let f = click(null, plain, 'A')!
    f = click(f, plain, 'B', { additive: true })!
    f = click(f, plain, 'C', { additive: true })!
    assert.deepEqual(f.values, ['A', 'B', 'C'])
    assert.equal(f.label, '店铺：A、B、C')
    f = click(f, plain, 'B', { additive: true })!
    assert.deepEqual(f.values, ['A', 'C'])
    f = click(f, plain, 'A', { additive: true })!
    assert.deepEqual(f.values, ['C'])
    assert.equal(click(f, plain, 'C', { additive: true }), null)
  })

  it('一开始就按 Ctrl：当作新选一个；换了图表时 Ctrl 点击是替换而不是并入', () => {
    assert.deepEqual(vals(click(null, plain, 'A', { additive: true })), ['A'])
    const a = click(null, plain, 'A', {}, 'c1')!
    assert.deepEqual(vals(click(a, plain, 'B', { additive: true }, 'c2')), ['B'])
  })

  it('选了很多项时显示前 3 项和总数', () => {
    let f = click(null, plain, 'A')!
    for (const c of ['B', 'C', 'D', 'E'])
      f = click(f, plain, c, { additive: true })!
    assert.equal(f.label, '店铺：A、B、C 等 5 项')
  })

  it('日期按月分组：显示粒度', () => {
    let f = click(null, monthly, '2026-01')!
    f = click(f, monthly, '2026-03', { additive: true })!
    assert.equal(f.label, '下单时间（月）：2026-01、2026-03')
  })
})

describe('nextCrossFilter：按系列联动', () => {
  it('点在某个系列的柱子上：同时选中分类与系列；点同一个取消', () => {
    const f = click(null, split, 'A', { series: '线上' })!
    assert.deepEqual(f.series, { column: '渠道', values: ['线上'] })
    assert.equal(f.label, '店铺：A · 渠道：线上')
    assert.equal(click(f, split, 'A', { series: '线上' }), null)
  })

  it('同一分类里点另一个系列：替换；只点分类（没有系列）：去掉系列，只按分类', () => {
    const f = click(null, split, 'A', { series: '线上' })!
    const g = click(f, split, 'A', { series: '线下' })!
    assert.deepEqual(g.series?.values, ['线下'])
    const h = click(g, split, 'A')!
    assert.equal(h.series, undefined)
    assert.equal(h.label, '店铺：A')
  })

  it('Ctrl 多选：分类与系列各自增减', () => {
    let f = click(null, split, 'A', { series: '线上' })!
    f = click(f, split, 'B', { series: '线下', additive: true })!
    assert.deepEqual(f.values, ['A', 'B'])
    assert.deepEqual(f.series?.values, ['线上', '线下'])
    f = click(f, split, 'B', { series: '线下', additive: true })!
    assert.deepEqual(f.values, ['A'])
    assert.deepEqual(f.series?.values, ['线上'])
  })

  it('Ctrl 点同一个系列的另一根柱子：系列保持不变，分类增加（不会把系列减掉）', () => {
    let f = click(null, split, 'A', { series: '线下' })!
    f = click(f, split, 'B', { series: '线下', additive: true })!
    assert.deepEqual(f.values, ['A', 'B'])
    assert.deepEqual(f.series?.values, ['线下'])
    assert.equal(f.label, '店铺：A、B · 渠道：线下')
  })

  it('Ctrl 点已选的那根柱子：只去掉这一根', () => {
    let f = click(null, split, 'A', { series: '线上' })!
    f = click(f, split, 'B', { series: '线下', additive: true })!
    f = click(f, split, 'A', { series: '线上', additive: true })!
    assert.deepEqual(f.picks, [['B', '线下']])
    assert.deepEqual(f.series?.values, ['线下'])
  })

  it('已选里有只点了分类（没带系列）的：这个分类要所有系列，不再按系列筛选', () => {
    let f = click(null, split, 'A', { series: '线上' })!
    f = click(f, split, 'B', { additive: true })!
    assert.deepEqual(f.values, ['A', 'B'])
    assert.equal(f.series, undefined)
    assert.deepEqual(crossFilterConditions(f, c => c === '店铺' || c === '渠道'), [{ op: 'in', column: '店铺', values: ['A', 'B'] }])
  })

  it('图表没有拆分系列时，传入的系列被忽略', () => {
    const f = click(null, plain, 'A', { series: '线上' })!
    assert.equal(f.series, undefined)
  })
})

describe('crossFilterConditions', () => {
  const has = (...cols: string[]) => (c: string) => cols.includes(c)

  it('普通分类：in；多个月份：prefixes；单个月份：prefix', () => {
    assert.deepEqual(crossFilterConditions(click(null, plain, 'A')!, has('店铺')), [{ op: 'in', column: '店铺', values: ['A'] }])
    const one = click(null, monthly, '2026-01')!
    assert.deepEqual(crossFilterConditions(one, has('下单时间')), [{ op: 'prefix', column: '下单时间', value: '2026-01' }])
    const two = click(one, monthly, '2026-03', { additive: true })!
    assert.deepEqual(crossFilterConditions(two, has('下单时间')), [{ op: 'prefixes', column: '下单时间', values: ['2026-01', '2026-03'] }])
  })

  it('带系列：再加一个系列字段的 in；部件的数据集没有某一列时那一项不筛选', () => {
    const f = click(null, split, 'A', { series: '线上' })!
    assert.deepEqual(crossFilterConditions(f, has('店铺', '渠道')), [{ op: 'in', column: '店铺', values: ['A'] }, { op: 'in', column: '渠道', values: ['线上'] }])
    assert.deepEqual(crossFilterConditions(f, has('渠道')), [{ op: 'in', column: '渠道', values: ['线上'] }])
    assert.deepEqual(crossFilterConditions(f, has('别的列')), [])
  })
})
