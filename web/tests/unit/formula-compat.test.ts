import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { fromExcelFormula, toExcelCompatFormula } from '@/features/excel/xlsx/formula-compat'

describe('toExcelCompatFormula：表格里的写法 → xlsx 文件里的写法', () => {
  const cases: [string, string][] = [
    ['XLOOKUP(A1,B:B,C:C)', '_xlfn.XLOOKUP(A1,B:B,C:C)'],
    ['IFS(A1>1,"a",TRUE,"b")', '_xlfn.IFS(A1>1,"a",TRUE,"b")'],
    ['FILTER(A1:A9,B1:B9>1)', '_xlfn._xlws.FILTER(A1:A9,B1:B9>1)'],
    ['SORT(A1:A9)', '_xlfn._xlws.SORT(A1:A9)'],
    // 嵌套：内外层各自处理
    ['SUM(FILTER(A1:A9,B1:B9>1))', 'SUM(_xlfn._xlws.FILTER(A1:A9,B1:B9>1))'],
    ['IFERROR(XLOOKUP(A1,B:B,C:C),0)', 'IFERROR(_xlfn.XLOOKUP(A1,B:B,C:C),0)'],
    // LET / LAMBDA：参数名带 _xlpm.
    ['LET(x,1,x+1)', '_xlfn.LET(_xlpm.x,1,_xlpm.x+1)'],
    ['LET(单价,10,数量,3,单价*数量)', '_xlfn.LET(_xlpm.单价,10,_xlpm.数量,3,_xlpm.单价*_xlpm.数量)'],
    ['LAMBDA(x,y,x+y)', '_xlfn.LAMBDA(_xlpm.x,_xlpm.y,_xlpm.x+_xlpm.y)'],
  ]
  for (const [input, expected] of cases)
    it(input, () => assert.equal(toExcelCompatFormula(input), expected))

  it('老函数、已带前缀的、字符串里的函数名都不动', () => {
    assert.equal(toExcelCompatFormula('SUM(A1:A3)'), 'SUM(A1:A3)')
    assert.equal(toExcelCompatFormula('VLOOKUP(A1, B:C, 2, FALSE)'), 'VLOOKUP(A1, B:C, 2, FALSE)')
    assert.equal(toExcelCompatFormula('_xlfn.IFS(A1,1)'), '_xlfn.IFS(A1,1)')
    assert.equal(toExcelCompatFormula('A1&"XLOOKUP(1)"'), 'A1&"XLOOKUP(1)"')
  })

  it('LET 的参数名只替换整词，不误伤同名前缀、工作表名与字符串', () => {
    assert.equal(toExcelCompatFormula('LET(x,1,xy+x)'), '_xlfn.LET(_xlpm.x,1,xy+_xlpm.x)')
    assert.equal(toExcelCompatFormula('LET(x,1,x+"x")'), '_xlfn.LET(_xlpm.x,1,_xlpm.x+"x")')
  })
})

describe('fromExcelFormula：xlsx 文件里的写法 → 表格里的写法', () => {
  const cases: [string, string][] = [
    ['_xlfn.XLOOKUP(A1,B:B,C:C)', 'XLOOKUP(A1,B:B,C:C)'],
    ['_xlfn._xlws.FILTER(A1:A9,B1:B9>1)', 'FILTER(A1:A9,B1:B9>1)'],
    ['_xlfn.LET(_xlpm.x,1,_xlpm.x+1)', 'LET(x,1,x+1)'],
    ['_xlfn.SINGLE(A1:A3)', 'A1:A3'],
    ['SUM(_xlfn._xlws.FILTER(A1:A9,B1:B9>1))', 'SUM(FILTER(A1:A9,B1:B9>1))'],
    ['IFERROR(_xlfn.XLOOKUP(A1,B:B,C:C),0)', 'IFERROR(XLOOKUP(A1,B:B,C:C),0)'],
  ]
  for (const [input, expected] of cases)
    it(input, () => assert.equal(fromExcelFormula(input), expected))

  it('没有前缀的公式原样返回；字符串里的 _xlpm. 保留', () => {
    assert.equal(fromExcelFormula('SUM(A1:A3)'), 'SUM(A1:A3)')
    assert.equal(fromExcelFormula('_xlfn.IFS(A1,"_xlpm.x")'), 'IFS(A1,"_xlpm.x")')
  })
})

describe('往返不丢失', () => {
  it('表格写法 → 文件写法 → 表格写法 与原来一致', () => {
    const formulas = [
      'XLOOKUP(A1,B:B,C:C)',
      'FILTER(A1:A9,B1:B9>1)',
      'SUM(FILTER(A1:A9,B1:B9>1))',
      'LET(x,1,x+1)',
      'LET(单价,10,数量,3,单价*数量)',
      'IFS(A1>1,"a",TRUE,"b")',
      'UNIQUE(A1:A9)',
      'TEXTJOIN(",",TRUE,A1:A3)',
      'SUM(A1:A3)',
    ]
    for (const f of formulas)
      assert.equal(fromExcelFormula(toExcelCompatFormula(f)), f, f)
  })
})
