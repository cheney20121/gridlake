import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { BUILTIN_FUNCTIONS, UNSUPPORTED_FUNCTIONS } from '../../src/features/excel/formula/function-catalog.ts'
import { calledFunctions, functionProblems } from '../../src/features/excel/formula/function-check.ts'

describe('函数兼容：公式里用到的函数', () => {
  it('找出调用的函数名：大小写、前缀、嵌套、字符串里的不算', () => {
    assert.deepEqual(calledFunctions('=SUM(A1:A3)+_xlfn.XLOOKUP(B1, C:C, D:D)+if(x, "SUM(1)", 2)'), ['SUM', 'XLOOKUP', 'IF'])
    assert.deepEqual(calledFunctions('=_xlfn._xlws.FILTER(A:A, B:B>0)'), ['FILTER'])
    assert.deepEqual(calledFunctions('=Sheet1!A1+1'), [])
  })
  it('区分不支持的 Excel 函数（附原因）和不认识的名字；平台函数、LET 里的变量不算', () => {
    const p = functionProblems('=CUBEVALUE("x")+SUMM(A1)+DSUMIFS("订单","金额")+LET(f, LAMBDA(x, x+1), f(2))+XLOOKUP(1,A:A,B:B)')
    assert.deepEqual(p.unsupported.map(x => x.name), ['CUBEVALUE'])
    assert.match(p.unsupported[0]!.reason, /OLAP/)
    assert.deepEqual(p.unknown, ['SUMM'])
    assert.deepEqual(functionProblems('=SUM(1)'), { unsupported: [], unknown: [] })
  })
  it('生成的名单：常用的新函数都在，不支持的有原因', () => {
    for (const f of ['XLOOKUP', 'FILTER', 'LET', 'LAMBDA', 'VSTACK', 'TEXTSPLIT', 'REGEXTEST', 'SUMIFS'])
      assert.ok(BUILTIN_FUNCTIONS.has(f), f)
    assert.ok(Object.values(UNSUPPORTED_FUNCTIONS).every(r => r.length > 0))
  })
})
