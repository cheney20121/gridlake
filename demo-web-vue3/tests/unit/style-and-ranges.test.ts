import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { fromArgb, mergeRanges, toArgb } from '@/features/excel/xlsx/sheet-features'
import { excelColorToHex, excelToUniverStyle, excelWidthToPx, hexToArgb, ptToPx, pxToExcelWidth, pxToPt, univerToExcelStyle } from '@/features/excel/xlsx/style-map'

describe('颜色转换', () => {
  it('hexToArgb：#RGB / #RRGGBB / rgb()', () => {
    assert.equal(hexToArgb('#fff'), 'FFFFFFFF')
    assert.equal(hexToArgb('#fde2e1'), 'FFFDE2E1')
    assert.equal(hexToArgb('rgb(253, 226, 225)'), 'FFFDE2E1')
    assert.equal(hexToArgb('rgb(300,0,0)'), 'FFFF0000')
    assert.equal(hexToArgb('red'), undefined)
    assert.equal(hexToArgb(''), undefined)
    assert.equal(hexToArgb(null), undefined)
  })

  it('excelColorToHex：argb / 6 位 / 主题色 / 无法识别', () => {
    assert.equal(excelColorToHex({ argb: 'FFFF0000' }), '#FF0000')
    assert.equal(excelColorToHex({ argb: '00ff00' }), '#00FF00')
    assert.match(excelColorToHex({ theme: 1 }) ?? '', /^#[0-9A-F]{6}$/)
    assert.match(excelColorToHex({ theme: 4, tint: 0.4 }) ?? '', /^#[0-9A-F]{6}$/)
    assert.notEqual(excelColorToHex({ theme: 4, tint: 0.4 }), excelColorToHex({ theme: 4 }))
    assert.equal(excelColorToHex({}), undefined)
    assert.equal(excelColorToHex(undefined), undefined)
  })

  it('toArgb / fromArgb（条件格式、数据验证用）', () => {
    assert.equal(toArgb('#FDE2E1'), 'FFFDE2E1')
    assert.equal(toArgb('rgb(253,226,225)'), 'FFFDE2E1')
    assert.equal(toArgb('80FDE2E1'), '80FDE2E1')
    assert.equal(toArgb('nope'), undefined)
    assert.equal(fromArgb('FFFDE2E1'), '#FDE2E1')
    assert.equal(fromArgb('FDE2E1'), '#FDE2E1')
    assert.equal(fromArgb('xyz'), undefined)
    assert.equal(fromArgb(undefined), undefined)
  })
})

describe('尺寸换算', () => {
  it('列宽：Excel 字符数 ↔ 像素，往返稳定', () => {
    assert.equal(excelWidthToPx(8.43), 64)
    assert.equal(pxToExcelWidth(64), 8.43)
    for (const w of [8.43, 12, 20, 30.5])
      assert.ok(Math.abs(pxToExcelWidth(excelWidthToPx(w)) - w) < 0.1, String(w))
    assert.equal(pxToExcelWidth(0), 1)
  })

  it('行高：磅 ↔ 像素', () => {
    assert.equal(ptToPx(12), 16)
    assert.equal(pxToPt(16), 12)
    assert.equal(pxToPt(ptToPx(15)), 15)
  })
})

describe('单元格样式往返', () => {
  it('Univer → Excel：字体、颜色、填充、对齐、换行、数字格式', () => {
    const st = univerToExcelStyle({ ff: '微软雅黑', fs: 12, bl: 1, it: 1, ul: { s: 1 }, st: { s: 1 }, cl: { rgb: '#FF0000' }, bg: { rgb: '#FFFF00' }, ht: 2, vt: 2, tb: 3, n: { pattern: '#,##0.00' } })
    assert.deepEqual(st?.font, { name: '微软雅黑', size: 12, bold: true, italic: true, underline: true, strike: true, color: { argb: 'FFFF0000' } })
    assert.equal((st?.fill as { fgColor: { argb: string } }).fgColor.argb, 'FFFFFF00')
    assert.deepEqual(st?.alignment, { horizontal: 'center', vertical: 'middle', wrapText: true })
    assert.equal(st?.numFmt, '#,##0.00')
  })

  it('没有样式返回 null', () => {
    assert.equal(univerToExcelStyle(null), null)
    assert.equal(univerToExcelStyle({}), null)
    assert.equal(excelToUniverStyle(undefined), null)
    assert.equal(excelToUniverStyle({}), null)
  })

  it('Excel → Univer：默认字体（Calibri 11）与黑色不写入，常规格式不写入', () => {
    assert.equal(excelToUniverStyle({ font: { name: 'Calibri', size: 11, color: { argb: 'FF000000' } }, numFmt: 'General' }), null)
    const u = excelToUniverStyle({ font: { name: 'Arial', size: 14, bold: true, color: { argb: 'FFFF0000' } }, alignment: { horizontal: 'right', vertical: 'top', wrapText: true }, numFmt: '0.00%' })
    assert.deepEqual(u, { ff: 'Arial', fs: 14, bl: 1, cl: { rgb: '#FF0000' }, ht: 3, vt: 1, tb: 3, n: { pattern: '0.00%' } })
  })

  it('边框：往返保持样式与颜色，无边框不输出', () => {
    const excel = univerToExcelStyle({ bd: { t: { s: 1, cl: { rgb: '#FF0000' } }, b: { s: 8, cl: { rgb: '#0000FF' } } } as never })
    assert.deepEqual(excel?.border, { top: { style: 'thin', color: { argb: 'FFFF0000' } }, bottom: { style: 'medium', color: { argb: 'FF0000FF' } } })
    const back = excelToUniverStyle({ border: excel!.border })
    assert.deepEqual(back?.bd, { t: { s: 1, cl: { rgb: '#FF0000' } }, b: { s: 8, cl: { rgb: '#0000FF' } } })
    assert.equal(univerToExcelStyle({ bd: { t: { s: 0 } } as never }), null)
  })

  it('Univer → Excel → Univer 保持不变', () => {
    const original = { ff: 'Arial', fs: 14, bl: 1 as const, it: 1 as const, cl: { rgb: '#FF0000' }, bg: { rgb: '#FFFF00' }, ht: 2, vt: 3, tb: 3, n: { pattern: '0.00' } }
    assert.deepEqual(excelToUniverStyle(univerToExcelStyle(original)!), original)
  })
})

describe('mergeRanges：exceljs 逐格列出的区域合并回整块', () => {
  const r = (startRow: number, endRow: number, startColumn: number, endColumn: number) => ({ startRow, endRow, startColumn, endColumn })

  it('同一列纵向相邻合并，再把等高的相邻列横向合并', () => {
    const cells = [r(0, 0, 0, 0), r(1, 1, 0, 0), r(2, 2, 0, 0), r(0, 0, 1, 1), r(1, 1, 1, 1), r(2, 2, 1, 1)]
    assert.deepEqual(mergeRanges(cells), [r(0, 2, 0, 1)])
  })

  it('不相邻的区域保持分开', () => {
    assert.deepEqual(mergeRanges([r(0, 0, 0, 0), r(5, 5, 0, 0)]), [r(0, 0, 0, 0), r(5, 5, 0, 0)])
    assert.deepEqual(mergeRanges([r(0, 2, 0, 0), r(0, 3, 1, 1)]), [r(0, 2, 0, 0), r(0, 3, 1, 1)])
  })

  it('乱序输入、空输入、不改动入参', () => {
    const input = [r(2, 2, 0, 0), r(0, 0, 0, 0), r(1, 1, 0, 0)]
    const copy = JSON.stringify(input)
    assert.deepEqual(mergeRanges(input), [r(0, 2, 0, 0)])
    assert.equal(JSON.stringify(input), copy)
    assert.deepEqual(mergeRanges([]), [])
  })
})
