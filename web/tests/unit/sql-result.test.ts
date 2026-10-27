import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { parseQuery, sqlTable, sqlValue } from '../../src/features/excel/formula/sql-result.ts'

function result(rows: unknown[][], truncated = false) {
  return {
    columns: [{ name: '店铺', kind: 'text' as const, sqlType: 'VARCHAR' }, { name: '金额', kind: 'money' as const, sqlType: 'DOUBLE' }],
    rows: rows as (string | number | boolean | null)[][],
    truncated,
    elapsedMs: 1,
    datasets: ['订单'],
  }
}

describe('单元格里的 SQL', () => {
  it('参数：名称与值成对；数字按数字、其他按文本；单元格引用传来的 1×1 数组取第一个值', () => {
    assert.deepEqual(parseQuery(['SELECT 1']), { sql: 'SELECT 1', params: [], values: {} })
    assert.deepEqual(parseQuery(['SELECT {{a}}, {{b}}, {{c}}', 'a', [[3]], ' b ', '上海', 'c', true]), {
      sql: 'SELECT {{a}}, {{b}}, {{c}}',
      params: [{ name: 'a', kind: 'number' }, { name: 'b', kind: 'text' }, { name: 'c', kind: 'text' }],
      values: { a: '3', b: '上海', c: 'TRUE' },
    })
    assert.equal(parseQuery(['']), '#缺少查询')
    assert.equal(parseQuery(['SELECT 1', 'a']), '#参数应为「名称, 值」成对出现')
    assert.equal(parseQuery(['SELECT 1', '', 1]), '#参数名不能为空')
  })
  it('SQL：表头 + 数据行，空值为空白；超过上限给出说明', () => {
    assert.deepEqual(sqlTable(result([['A', 10], ['B', null]])), [['店铺', '金额'], ['A', 10], ['B', '']])
    assert.match(String(sqlTable(result([['A', 1]], true))), /^#结果超过 1000 行/)
  })
  it('SQLVALUE：第一行第一列，没有行时为 #N/A', () => {
    assert.equal(sqlValue(result([['A', 10]])), 'A')
    assert.equal(sqlValue(result([])), '#N/A')
  })
})
