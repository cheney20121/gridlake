import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import {
  literal,
  renameTableInFormula,
  rewriteCalls,
  tableColumnsUsed,
  tableRef,
  toExcelFormula,
  toPlatformFormula,
} from '@/features/excel/datatable/formula-syntax'

/** 工作簿里有两张数据表：订单表、退款表（名称不区分大小写，返回规范表名） */
const TABLES: Record<string, string> = { 订单表: '订单表', 退款表: '退款表', orders: 'Orders' }
const isTable = (n: string) => TABLES[n] ?? TABLES[Object.keys(TABLES).find(k => k.toLowerCase() === n.toLowerCase()) ?? ''] ?? null

const exportTable = (n: string) => (n === '订单表' ? { name: '订单表', columns: new Set(['金额', '店铺', 'O\'Brien', '日期']) } : null)

describe('toPlatformFormula：Excel 表格引用 → 平台函数', () => {
  const cases: [string, string][] = [
    ['=SUM(订单表[金额])', '=TSUM("订单表[金额]")'],
    ['=average(订单表[金额])', '=TAVERAGE("订单表[金额]")'],
    ['=MIN(订单表[金额])', '=TMIN("订单表[金额]")'],
    ['=MAX(订单表[金额])', '=TMAX("订单表[金额]")'],
    ['=SUMIFS(订单表[金额], 订单表[店铺], B2)', '=TSUM("订单表[金额]", "店铺", B2)'],
    ['=SUMIFS(订单表[金额],订单表[店铺],"A店",订单表[日期],">=2026-01-01")', '=TSUM("订单表[金额]", "店铺", "A店", "日期", ">=2026-01-01")'],
    ['=AVERAGEIFS(订单表[金额], 订单表[店铺], B2)', '=TAVERAGE("订单表[金额]", "店铺", B2)'],
    ['=COUNT(订单表[金额])', '=TCOUNT("订单表", "金额", "<>")'],
    ['=ROWS(订单表[金额])', '=TCOUNT("订单表")'],
    ['=COUNTIFS(订单表[店铺], B2)', '=TCOUNT("订单表", "店铺", B2)'],
    ['=COUNTIF(订单表[店铺], "A店")', '=TCOUNT("订单表", "店铺", "A店")'],
    ['=SUMIF(订单表[店铺], B2, 订单表[金额])', '=TSUM("订单表[金额]", "店铺", B2)'],
    ['=INDEX(订单表[金额], 3)', '=TVALUE("订单表[金额]", 3)'],
    // 嵌套在别的函数里：只改表格引用那一段，其余原样
    ['=ROUND(SUM(订单表[金额])/2, 2)', '=ROUND(TSUM("订单表[金额]")/2, 2)'],
    ['=IF(SUM(订单表[金额])>0, SUM(退款表[金额])/SUM(订单表[金额]), 0)', '=IF(TSUM("订单表[金额]")>0, TSUM("退款表[金额]")/TSUM("订单表[金额]"), 0)'],
    // 表名不区分大小写，输出用规范表名
    ['=SUM(ORDERS[amount])', '=TSUM("Orders[amount]")'],
  ]
  for (const [input, expected] of cases)
    it(input, () => assert.equal(toPlatformFormula(input, isTable), expected))

  it('ROWS(表名) 没有列：不转换（单独的表名不是表格引用）', () => {
    assert.equal(toPlatformFormula('=ROWS(订单表)', isTable), null)
  })

  it('不是数据表 / 没有表格引用 / 不是公式：返回 null，不改动', () => {
    assert.equal(toPlatformFormula('=SUM(未知表[金额])', isTable), null)
    assert.equal(toPlatformFormula('=SUM(A1:A9)', isTable), null)
    assert.equal(toPlatformFormula('订单表[金额]', isTable), null)
    assert.equal(toPlatformFormula('=1+2', isTable), null)
  })

  it('条件列来自不同的表：不转换，避免算错', () => {
    assert.equal(toPlatformFormula('=SUMIFS(订单表[金额], 退款表[店铺], B2)', isTable), null)
  })

  it('字符串里的括号、逗号、方括号不会被当成公式结构', () => {
    assert.equal(toPlatformFormula('=IF(A1="SUM(订单表[金额])", 1, 2)', isTable), null)
    assert.equal(
      toPlatformFormula('=SUMIFS(订单表[金额], 订单表[店铺], "A,B(店)")', isTable),
      '=TSUM("订单表[金额]", "店铺", "A,B(店)")',
    )
  })

  it('参数个数不对的写法原样保留（不猜）', () => {
    assert.equal(toPlatformFormula('=SUMIFS(订单表[金额], 订单表[店铺])', isTable), null)
    assert.equal(toPlatformFormula('=SUM(订单表[金额], 5)', isTable), null)
  })
})

describe('toExcelFormula：平台函数 → Excel 原生公式（导出 xlsx）', () => {
  const cases: [string, string][] = [
    ['=TSUM("订单表[金额]")', '=SUM(订单表[金额])'],
    ['=TAVERAGE("订单表[金额]")', '=AVERAGE(订单表[金额])'],
    ['=TSUM("订单表[金额]", "店铺", B2)', '=SUMIFS(订单表[金额], 订单表[店铺], B2)'],
    ['=TMAX("订单表[金额]", "店铺", "A店")', '=MAXIFS(订单表[金额], 订单表[店铺], "A店")'],
    ['=TCOUNT("订单表")', '=ROWS(订单表)'],
    ['=TCOUNT("订单表", "店铺", B2)', '=COUNTIFS(订单表[店铺], B2)'],
    ['=TVALUE("订单表[金额]", 3)', '=INDEX(订单表[金额], 3)'],
    ['=ROUND(TSUM("订单表[金额]")/2, 2)', '=ROUND(SUM(订单表[金额])/2, 2)'],
    // 没有平台函数：原样返回
    ['=SUM(A1:A9)', '=SUM(A1:A9)'],
    ['=1+2', '=1+2'],
  ]
  for (const [input, expected] of cases)
    it(input, () => assert.equal(toExcelFormula(input, exportTable), expected))

  it('列名里的特殊字符按 Excel 规则转义', () => {
    assert.equal(toExcelFormula('=TSUM("订单表[O\'Brien]")', exportTable), '=SUM(订单表[O\'\'Brien])')
  })

  it('任何一处转不了就整条返回 null（调用方改为导出数值）', () => {
    assert.equal(toExcelFormula('=TSUM("未知表[金额]")', exportTable), null)
    assert.equal(toExcelFormula('=TSUM("订单表[不存在的列]")', exportTable), null)
    assert.equal(toExcelFormula('=TSUM("订单表[金额]", "不存在的列", B2)', exportTable), null)
    assert.equal(toExcelFormula('=TSUM(A1)', exportTable), null)
    assert.equal(toExcelFormula('=TSUM("订单表[金额]")+TSUM("未知表[金额]")', exportTable), null)
  })

  it('往返：输入写法 → 平台函数 → 导出写法，与原公式一致', () => {
    for (const f of ['=SUM(订单表[金额])', '=SUMIFS(订单表[金额], 订单表[店铺], B2)', '=COUNTIFS(订单表[店铺], B2)', '=INDEX(订单表[金额], 3)', '=ROUND(SUM(订单表[金额])/2, 2)']) {
      const platform = toPlatformFormula(f, isTable)
      assert.ok(platform, f)
      assert.equal(toExcelFormula(platform, exportTable), f)
    }
  })
})

describe('renameTableInFormula：改名同步公式', () => {
  it('改写引用旧表名的数据表函数，保留其余参数与空白', () => {
    assert.equal(renameTableInFormula('=TSUM("订单表[金额]", "店铺", B2)', '订单表', '销售表'), '=TSUM("销售表[金额]", "店铺", B2)')
    assert.equal(renameTableInFormula('=TCOUNT("订单表")', '订单表', '销售表'), '=TCOUNT("销售表")')
  })

  it('表名不区分大小写、前后空白忽略；同一公式里多处都改', () => {
    assert.equal(renameTableInFormula('=TSUM("ORDERS[a]")+TSUM("orders[b]")', ' Orders ', 'Sales'), '=TSUM("Sales[a]")+TSUM("Sales[b]")')
  })

  it('没有引用这张表 / 没有数据表函数：返回 null', () => {
    assert.equal(renameTableInFormula('=TSUM("退款表[金额]")', '订单表', '销售表'), null)
    assert.equal(renameTableInFormula('=SUM(A1:A3)', '订单表', '销售表'), null)
  })
})

describe('tableColumnsUsed：公式用到数据表的哪些列', () => {
  it('收集求值列与条件列，按小写表名归类', () => {
    const m = new Map<string, Set<string>>()
    tableColumnsUsed('=TSUM("订单表[金额]", "店铺", B2, "日期", ">=1")+TCOUNT("订单表", "状态", "完成")+TVALUE("退款表[金额]", 1)', m)
    assert.deepEqual([...m.get('订单表')!].sort(), ['日期', '状态', '店铺', '金额'].sort())
    assert.deepEqual([...m.get('退款表')!], ['金额'])
  })
})

describe('基础解析', () => {
  it('literal：只认字符串字面量，"" 还原为引号', () => {
    assert.equal(literal(' "a""b" '), 'a"b')
    assert.equal(literal('B2'), null)
    assert.equal(literal('"a" & "b"'), null)
  })

  it('tableRef：表名[列名]', () => {
    assert.deepEqual(tableRef(' 订单表[ 金额 ] '), { table: '订单表', column: '金额' })
    assert.equal(tableRef('A1:B2'), null)
    assert.equal(tableRef('"订单表[金额]"'), null)
  })

  it('rewriteCalls：括号不平衡 / 空参数也不会死循环', () => {
    assert.equal(rewriteCalls('=SUM(', () => null), '=SUM()')
    assert.equal(rewriteCalls('=SUM()', (n, a) => `${n}:${a.length}`), '=SUM:0')
    assert.equal(rewriteCalls('=((1+2))', () => null), '=((1+2))')
    assert.equal(rewriteCalls('=A1&"("&B1', () => null), '=A1&"("&B1')
  })
})
