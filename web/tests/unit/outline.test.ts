import type { SheetOutline } from '@/features/excel/outline/outline'
import assert from 'node:assert/strict'
import { beforeEach, describe, it } from 'node:test'
import { emptyOutline, getOutline, group, groupsOf, hasOutline, MAX_LEVEL, maxLevel, saveOutline, shiftOutline, showLevel, toggleGroup, ungroup } from '@/features/excel/outline/outline'

/** 够用的工作表替身：元数据 + 行列显示隐藏（outline.ts 只用到这些） */
function fakeSheet(id = 's1') {
  let meta: Record<string, unknown> = {}
  const hidden = { rows: new Set<number>(), cols: new Set<number>() }
  const set = (s: Set<number>, from: number, n: number, hide: boolean) => {
    for (let i = from; i < from + n; i++)
      hide ? s.add(i) : s.delete(i)
  }
  return {
    hidden,
    getSheetId: () => id,
    getCustomMetadata: () => meta,
    setCustomMetadata: (m: Record<string, unknown>) => {
      meta = m
    },
    hideRows: (i: number, n: number) => set(hidden.rows, i, n, true),
    showRows: (i: number, n: number) => set(hidden.rows, i, n, false),
    hideColumns: (i: number, n: number) => set(hidden.cols, i, n, true),
    showColumns: (i: number, n: number) => set(hidden.cols, i, n, false),
  }
}
type Sheet = ReturnType<typeof fakeSheet>
const asSheet = (s: Sheet) => s as never
const rowsOf = (s: Sheet) => getOutline(asSheet(s)).rows
const sorted = (s: Set<number>) => [...s].sort((a, b) => a - b)

describe('groupsOf：由级别推出分组', () => {
  it('同级连续的行是一组；内层嵌在外层里', () => {
    const o: SheetOutline = { rows: { 2: 1, 3: 1, 4: 2, 5: 2, 6: 1 }, cols: {}, collapsed: { rows: [], cols: [] } }
    assert.deepEqual(groupsOf(o, 'rows'), [
      { level: 1, start: 2, end: 6, collapsed: false },
      { level: 2, start: 4, end: 5, collapsed: false },
    ])
    assert.equal(maxLevel(o, 'rows'), 2)
    assert.equal(maxLevel(o, 'cols'), 0)
  })

  it('中间断开的行是两个分组；折叠状态按「级别@末行」对应', () => {
    const o: SheetOutline = { rows: { 1: 1, 2: 1, 5: 1 }, cols: {}, collapsed: { rows: ['1@2'], cols: [] } }
    assert.deepEqual(groupsOf(o, 'rows'), [
      { level: 1, start: 1, end: 2, collapsed: true },
      { level: 1, start: 5, end: 5, collapsed: false },
    ])
  })

  it('没有分组 / 级别为 0 的不算', () => {
    assert.deepEqual(groupsOf(emptyOutline(), 'rows'), [])
    assert.deepEqual(groupsOf({ rows: { 3: 0 }, cols: {}, collapsed: { rows: [], cols: [] } }, 'rows'), [])
    assert.equal(hasOutline(emptyOutline()), false)
  })
})

describe('组合 / 取消组合 / 折叠', () => {
  let s: Sheet
  beforeEach(() => {
    s = fakeSheet()
  })

  it('组合 = 级别 +1，可嵌套；超过最大级别报错', () => {
    assert.equal(group(asSheet(s), 'rows', 2, 5), null)
    assert.equal(group(asSheet(s), 'rows', 3, 4), null)
    assert.deepEqual(rowsOf(s), { 2: 1, 3: 2, 4: 2, 5: 1 })
    for (let i = 2; i <= MAX_LEVEL; i++)
      group(asSheet(s), 'rows', 3, 3)
    assert.match(group(asSheet(s), 'rows', 3, 3) ?? '', new RegExp(`${MAX_LEVEL} 级`))
  })

  it('折叠隐藏分组内的行，展开恢复；外层展开时内层保持折叠', () => {
    group(asSheet(s), 'rows', 1, 6)
    group(asSheet(s), 'rows', 3, 4)
    const [outer, inner] = groupsOf(getOutline(asSheet(s)), 'rows')
    toggleGroup(asSheet(s), 'rows', inner!)
    assert.deepEqual(sorted(s.hidden.rows), [3, 4])
    toggleGroup(asSheet(s), 'rows', outer!)
    assert.deepEqual(sorted(s.hidden.rows), [1, 2, 3, 4, 5, 6])
    toggleGroup(asSheet(s), 'rows', groupsOf(getOutline(asSheet(s)), 'rows')[0]!)
    assert.deepEqual(sorted(s.hidden.rows), [3, 4], '外层展开后内层仍折叠')
  })

  it('showLevel：只显示到第 n 级', () => {
    group(asSheet(s), 'rows', 1, 6)
    group(asSheet(s), 'rows', 3, 4)
    showLevel(asSheet(s), 'rows', 2)
    assert.deepEqual(sorted(s.hidden.rows), [3, 4])
    showLevel(asSheet(s), 'rows', 1)
    assert.deepEqual(sorted(s.hidden.rows), [1, 2, 3, 4, 5, 6])
    showLevel(asSheet(s), 'rows', 99)
    assert.deepEqual(sorted(s.hidden.rows), [])
  })

  it('取消组合：级别 -1；折叠着的取消后重新显示；没有分组时提示', () => {
    group(asSheet(s), 'rows', 2, 4)
    toggleGroup(asSheet(s), 'rows', groupsOf(getOutline(asSheet(s)), 'rows')[0]!)
    assert.deepEqual(sorted(s.hidden.rows), [2, 3, 4])
    assert.equal(ungroup(asSheet(s), 'rows', 2, 4), null)
    assert.deepEqual(rowsOf(s), {})
    assert.deepEqual(sorted(s.hidden.rows), [])
    assert.match(ungroup(asSheet(s), 'rows', 2, 4) ?? '', /没有分组/)
    assert.match(ungroup(asSheet(s), 'cols', 0, 1) ?? '', /列没有分组/)
  })

  it('行与列互不影响；全部取消后元数据清空', () => {
    group(asSheet(s), 'cols', 1, 2)
    assert.deepEqual(rowsOf(s), {})
    assert.equal(maxLevel(getOutline(asSheet(s)), 'cols'), 1)
    ungroup(asSheet(s), 'cols', 1, 2)
    assert.equal(hasOutline(getOutline(asSheet(s))), false)
    assert.equal('outline' in s.getCustomMetadata(), false)
  })

  it('保存与读取互不共享引用（改读出来的对象不影响已保存的）', () => {
    group(asSheet(s), 'rows', 1, 2)
    const o = getOutline(asSheet(s))
    o.rows[9] = 5
    assert.deepEqual(rowsOf(s), { 1: 1, 2: 1 })
    saveOutline(asSheet(s), o)
    assert.equal(rowsOf(s)[9], 5)
  })
})

describe('shiftOutline：插入 / 删除行时级别跟着移动', () => {
  let s: Sheet
  beforeEach(() => {
    s = fakeSheet()
    group(asSheet(s), 'rows', 3, 5)
  })

  it('在分组之前插入：整体后移', () => {
    shiftOutline(asSheet(s), 'rows', 1, 2)
    assert.deepEqual(rowsOf(s), { 5: 1, 6: 1, 7: 1 })
  })

  it('在分组中间插入：新行继承两侧的级别；在分组外边缘插入：不继承', () => {
    shiftOutline(asSheet(s), 'rows', 4, 1)
    assert.deepEqual(rowsOf(s), { 3: 1, 4: 1, 5: 1, 6: 1 })
    shiftOutline(asSheet(s), 'rows', 3, 1)
    assert.deepEqual(rowsOf(s), { 4: 1, 5: 1, 6: 1, 7: 1 }, '紧挨分组上方插入的行不属于分组')
  })

  it('删除行：被删的行级别消失，后面的前移；删除再撤销（原位插回同样多的行）恢复级别', () => {
    shiftOutline(asSheet(s), 'rows', 4, -1)
    assert.deepEqual(rowsOf(s), { 3: 1, 4: 1 })
    shiftOutline(asSheet(s), 'rows', 4, 1)
    assert.deepEqual(rowsOf(s), { 3: 1, 4: 1, 5: 1 })
  })

  it('折叠记录随行移动，删掉分组末行后失效的记录被清掉', () => {
    toggleGroup(asSheet(s), 'rows', groupsOf(getOutline(asSheet(s)), 'rows')[0]!)
    shiftOutline(asSheet(s), 'rows', 0, 2)
    assert.deepEqual(getOutline(asSheet(s)).collapsed.rows, ['1@7'])
    shiftOutline(asSheet(s), 'rows', 7, -1)
    assert.deepEqual(getOutline(asSheet(s)).collapsed.rows.length, 0)
  })

  it('没有分组时什么都不做', () => {
    const t = fakeSheet('s2')
    shiftOutline(asSheet(t), 'rows', 1, 3)
    assert.equal(hasOutline(getOutline(asSheet(t))), false)
  })
})
