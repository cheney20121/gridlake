import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { findCycles } from '../../src/features/excel/formula/circular.ts'
import { parseRefs } from '../../src/features/excel/formula/formula-refs.ts'

const size = () => ({ rows: 1000, cols: 26 })
function cell(sheet: string, addr: string, formula: string) {
  const col = addr.charCodeAt(0) - 65
  return { sheet, row: Number(addr.slice(1)) - 1, col, formula }
}
const addr = (c: { row: number, col: number }) => `${String.fromCharCode(65 + c.col)}${c.row + 1}`
const names = (r: ReturnType<typeof findCycles>) => r.cycles.map(c => c.path.map(p => `${p.sheet}!${addr(p)}`).join(' → '))

describe('parseRefs', () => {
  it('单元格、区域、整列、整行、跨工作表、绝对引用', () => {
    assert.deepEqual(parseRefs('=A1+$B$2'), [
      { sheet: null, c1: 0, c2: 0, r1: 0, r2: 0 },
      { sheet: null, c1: 1, c2: 1, r1: 1, r2: 1 },
    ])
    assert.deepEqual(parseRefs('=SUM(D5:A1)'), [{ sheet: null, c1: 0, c2: 3, r1: 0, r2: 4 }])
    assert.deepEqual(parseRefs('=SUM(C:D)'), [{ sheet: null, c1: 2, c2: 3, r1: null, r2: null }])
    assert.deepEqual(parseRefs('=SUM(3:5)'), [{ sheet: null, c1: null, c2: null, r1: 2, r2: 4 }])
    assert.deepEqual(parseRefs(`='Sheet 2'!B3+Sheet3!C4`).map(r => r.sheet), ['Sheet 2', 'Sheet3'])
  })

  it('字符串里的文字不算引用；函数名不算', () => {
    assert.deepEqual(parseRefs('=IF(A1="B2","C3",LOG10(2))'), [{ sheet: null, c1: 0, c2: 0, r1: 0, r2: 0 }])
  })
})

describe('findCycles', () => {
  it('没有循环', () => {
    const r = findCycles([cell('S', 'A1', '=B1'), cell('S', 'B1', '=C1+1'), cell('S', 'C1', '=SUM(D1:D9)')], size)
    assert.deepEqual(r, { cycles: [], partial: false })
  })

  it('自己引用自己', () => {
    assert.deepEqual(names(findCycles([cell('S', 'A1', '=A1+1')], size)), ['S!A1 → S!A1'])
  })

  it('两个格子互相引用：路径从左上角开始，首尾相同', () => {
    const r = findCycles([cell('S', 'B2', '=A1'), cell('S', 'A1', '=B2*2')], size)
    assert.deepEqual(names(r), ['S!A1 → S!B2 → S!A1'])
    assert.equal(r.cycles[0]!.size, 2)
  })

  it('三个格子的环，旁边还有不在环里的公式', () => {
    const r = findCycles([cell('S', 'A1', '=B1'), cell('S', 'B1', '=C1'), cell('S', 'C1', '=A1'), cell('S', 'D1', '=A1+1'), cell('S', 'E1', '=D1')], size)
    assert.deepEqual(names(r), ['S!A1 → S!B1 → S!C1 → S!A1'])
    assert.equal(r.cycles[0]!.size, 3, '只有环里的 3 个，D1、E1 引用了环但不在环里')
  })

  it('公式引用了包含自己的区域（整列求和写在这一列里）', () => {
    assert.deepEqual(names(findCycles([cell('S', 'A5', '=SUM(A:A)')], size)), ['S!A5 → S!A5'])
    assert.deepEqual(names(findCycles([cell('S', 'A5', '=SUM(A1:A4)')], size)), [], '区域不含自己就没事')
    assert.deepEqual(names(findCycles([cell('S', 'A5', '=SUM(A1:A10)')], size)), ['S!A5 → S!A5'])
    assert.deepEqual(names(findCycles([cell('S', 'C3', '=SUM(3:3)')], size)), ['S!C3 → S!C3'], '整行也一样')
  })

  it('跨工作表的环；同样的地址在不同的表里是不同的格子', () => {
    const r = findCycles([cell('甲', 'A1', '=乙!A1+1'), cell('乙', 'A1', '=甲!A1+1')], size)
    assert.deepEqual(names(r), ['甲!A1 → 乙!A1 → 甲!A1'])
    // 同地址不同表：甲!A1 引用 乙!A1，乙!A1 不引用回来
    assert.deepEqual(names(findCycles([cell('甲', 'A1', '=乙!A1'), cell('乙', 'A1', '=1+1')], size)), [])
    // 没写表名的引用指本表
    assert.deepEqual(names(findCycles([cell('甲', 'A1', '=B1'), cell('乙', 'B1', '=A1')], size)), [])
  })

  it('引用没有公式的格子不构成循环；引用不存在的工作表被忽略', () => {
    assert.deepEqual(names(findCycles([cell('S', 'A1', '=B1'), cell('S', 'C1', '=Nope!A1')], size)), [])
  })

  it('多个独立的环按大小排序；一个很长的环路径被截短', () => {
    const cells = [cell('S', 'A1', '=B1'), cell('S', 'B1', '=A1')]
    // 20 个格子的长环（C 列 3..22）
    for (let i = 0; i < 20; i++)
      cells.push(cell('S', `C${i + 3}`, `=C${i === 19 ? 3 : i + 4}`))
    const r = findCycles(cells, size)
    assert.equal(r.cycles.length, 2)
    assert.equal(r.cycles[0]!.size, 20)
    assert.ok(r.cycles[0]!.truncated && r.cycles[0]!.path.length === 12)
    assert.equal(r.cycles[1]!.size, 2)
  })

  it('十万个公式的链（每个引用上一个）：不爆栈，没有循环，很快', () => {
    const cells = [cell('S', 'A1', '=1')]
    const big = { rows: 200_000, cols: 1 }
    for (let i = 2; i <= 100_000; i++)
      cells.push({ sheet: 'S', row: i - 1, col: 0, formula: `=A${i - 1}+1` })
    const t0 = Date.now()
    const r = findCycles(cells, () => big)
    assert.deepEqual(r, { cycles: [], partial: false })
    assert.ok(Date.now() - t0 < 3000, `用了 ${Date.now() - t0} 毫秒`)
    // 把链首改成引用链尾：整条链是一个环
    cells[0] = { sheet: 'S', row: 0, col: 0, formula: '=A100000' }
    const r2 = findCycles(cells, () => big)
    assert.equal(r2.cycles.length, 1)
    assert.equal(r2.cycles[0]!.size, 100_000)
  })

  it('引用关系太多时只检查一部分并标记', () => {
    // 每个格子都引用整列：n² 条边
    const cells = Array.from({ length: 200 }, (_, i) => ({ sheet: 'S', row: i, col: 1, formula: '=SUM(B:B)' }))
    const r = findCycles(cells, size, 1000)
    assert.equal(r.partial, true)
  })
})
