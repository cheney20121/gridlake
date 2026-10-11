import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { safeFileName } from '../../src/features/excel/dashboard/export-image.ts'

describe('safeFileName', () => {
  it('文件名里不能出现的字符与空白换成下划线，首尾的去掉', () => {
    assert.equal(safeFileName('店铺 × 渠道'), '店铺_×_渠道')
    assert.equal(safeFileName('a/b\\c:d*e?f"g<h>i|j'), 'a_b_c_d_e_f_g_h_i_j')
    assert.equal(safeFileName('  月度金额  '), '月度金额')
  })

  it('空名字用默认名', () => {
    assert.equal(safeFileName(''), '看板部件')
    assert.equal(safeFileName('///', '图表'), '图表')
  })
})
