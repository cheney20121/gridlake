import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import ExcelJS from 'exceljs'
import { excelToIconSet, ICON_SET_SIZE, iconSetToExcel } from '../../src/features/excel/xlsx/icon-set.ts'

const MAX = Number.MAX_SAFE_INTEGER
/** Univer 里的 3 个图标：与它自带的「快速插入」一致 */
function univer3(type = '3Arrows', hi = 67, mid = 33) {
  return [
    { operator: 'greaterThanOrEqual', value: { type: 'percent', value: hi }, iconType: type, iconId: '0' },
    { operator: 'greaterThanOrEqual', value: { type: 'percent', value: mid }, iconType: type, iconId: '1' },
    { operator: 'lessThanOrEqual', value: { type: 'percent', value: MAX }, iconType: type, iconId: '2' },
  ]
}

describe('iconSetToExcel', () => {
  it('3 个图标：阈值从低到高，第一个固定 0%', () => {
    assert.deepEqual(iconSetToExcel(univer3(), true), {
      type: 'iconSet',
      iconSet: '3Arrows',
      reverse: false,
      showValue: true,
      cfvo: [{ type: 'percent', value: 0 }, { type: 'percent', value: 33 }, { type: 'percent', value: 67 }],
    })
  })

  it('图标顺序整体反过来 = reverse；不显示数值', () => {
    const rev = univer3().map((c, k) => ({ ...c, iconId: String(2 - k) }))
    const r = iconSetToExcel(rev, false)
    assert.ok(typeof r !== 'string' && r.reverse === true && r.showValue === false)
  })

  it('5 个图标、数值阈值与公式阈值', () => {
    const cfg = [
      { operator: 'greaterThan', value: { type: 'num', value: 80 }, iconType: '5Quarters', iconId: '0' },
      { operator: 'greaterThan', value: { type: 'percentile', value: 60 }, iconType: '5Quarters', iconId: '1' },
      { operator: 'greaterThan', value: { type: 'formula', value: '=$G$1' }, iconType: '5Quarters', iconId: '2' },
      { operator: 'greaterThan', value: { type: 'num', value: 10 }, iconType: '5Quarters', iconId: '3' },
      { operator: 'lessThanOrEqual', value: { type: 'percent', value: MAX }, iconType: '5Quarters', iconId: '4' },
    ]
    const r = iconSetToExcel(cfg, true)
    assert.ok(typeof r !== 'string')
    assert.deepEqual(r.cfvo, [{ type: 'percent', value: 0 }, { type: 'num', value: 10 }, { type: 'formula', value: '$G$1' }, { type: 'percentile', value: 60 }, { type: 'num', value: 80 }])
  })

  it('做不到的返回原因：混用图标、顺序被自定义、个数不符、Univer 独有的图标集、其他条件', () => {
    const mixed = univer3()
    mixed[1]!.iconType = '3Signs'
    assert.match(String(iconSetToExcel(mixed, true)), /混用/)
    const shuffled = univer3()
    shuffled[0]!.iconId = '1'
    shuffled[1]!.iconId = '0'
    assert.match(String(iconSetToExcel(shuffled, true)), /自定义/)
    assert.match(String(iconSetToExcel(univer3('4Arrows'), true)), /个数/, '4Arrows 应有 4 个图标，只给了 3 个')
    assert.match(String(iconSetToExcel(univer3('_5Felling'), true)), /没有这种图标/)
    const lt = univer3()
    lt[0]!.operator = 'lessThan'
    assert.match(String(iconSetToExcel(lt, true)), /大于等于/)
    assert.match(String(iconSetToExcel([], true)), /没有这种图标/)
  })
})

describe('excelToIconSet', () => {
  it('3 个图标，还原成 Univer 的从高到低', () => {
    const r = excelToIconSet({ iconSet: '3TrafficLights2', cfvo: [{ type: 'percent', value: 0 }, { type: 'percent', value: 33 }, { type: 'percent', value: 67 }] })
    assert.deepEqual(r, { isShowValue: true, config: univer3('3TrafficLights2') })
  })

  it('exceljs 缺省的名字 3TrafficLights 就是 3TrafficLights1；showValue=false；reverse', () => {
    const r = excelToIconSet({ cfvo: [{ type: 'percent', value: 0 }, { type: 'num', value: 5 }, { type: 'num', value: 9 }], reverse: true, showValue: false })
    assert.ok(typeof r !== 'string')
    assert.equal(r.isShowValue, false)
    assert.deepEqual(r.config.map(c => [c.iconType, c.iconId, c.value.value]), [['3TrafficLights1', '2', 9], ['3TrafficLights1', '1', 5], ['3TrafficLights1', '0', MAX]])
  })

  it('公式阈值补上等号；number 别名；阈值个数不对 / 不认识的图标集返回原因', () => {
    const r = excelToIconSet({ iconSet: '3Flags', cfvo: [{ type: 'percent', value: 0 }, { type: 'formula', value: '$A$1' }, { type: 'number', value: 7 }] })
    assert.ok(typeof r !== 'string')
    assert.deepEqual(r.config.slice(0, 2).map(c => c.value), [{ type: 'num', value: 7 }, { type: 'formula', value: '=$A$1' }])
    assert.match(String(excelToIconSet({ iconSet: '3Flags', cfvo: [{ type: 'percent', value: 0 }] })), /阈值个数/)
    assert.match(String(excelToIconSet({ iconSet: 'Whatever', cfvo: [] })), /Whatever/)
  })
})

describe('经 xlsx 文件往返（exceljs 写出再读回）', () => {
  it('20 种图标集 × 是否反向，导出再导入与原配置一致', async () => {
    const wb = new ExcelJS.Workbook()
    const ws = wb.addWorksheet('S')
    const cases: { ref: string, name: string, reverse: boolean, show: boolean }[] = []
    let row = 1
    for (const [name, size] of Object.entries(ICON_SET_SIZE)) {
      for (const reverse of [false, true]) {
        // 阈值：num 10、20、30…，Univer 从高到低
        const config = Array.from({ length: size }, (_, k) => ({
          operator: k === size - 1 ? 'lessThanOrEqual' : 'greaterThanOrEqual',
          value: k === size - 1 ? { type: 'percent', value: MAX } : { type: 'num', value: (size - 1 - k) * 10 },
          iconType: name,
          iconId: String(reverse ? size - 1 - k : k),
        }))
        const show = row % 3 !== 0
        const ex = iconSetToExcel(config, show)
        assert.ok(typeof ex !== 'string', `${name} ${String(ex)}`)
        const ref = `A${row}:A${row}`
        ws.addConditionalFormatting({ ref, rules: [{ ...ex, priority: row } as never] })
        cases.push({ ref, name, reverse, show })
        row++
      }
    }
    const buf = await wb.xlsx.writeBuffer()
    const wb2 = new ExcelJS.Workbook()
    await wb2.xlsx.load(buf)
    const read = (wb2.getWorksheet('S') as unknown as { conditionalFormattings: { ref: string, rules: Record<string, any>[] }[] }).conditionalFormattings
    assert.equal(read.length, cases.length)
    const bad: string[] = []
    for (const c of cases) {
      const rule = read.find(x => x.ref === c.ref)!.rules[0]!
      const back = excelToIconSet(rule)
      if (typeof back === 'string') {
        bad.push(`${c.name}: ${back}`)
        continue
      }
      const size = ICON_SET_SIZE[c.name]!
      const expectIds = Array.from({ length: size }, (_, k) => String(c.reverse ? size - 1 - k : k))
      const ok = back.config.every(x => x.iconType === c.name)
        && back.config.map(x => x.iconId).join() === expectIds.join()
        && back.config.slice(0, -1).map(x => `${x.value.type}:${x.value.value}`).join() === Array.from({ length: size - 1 }, (_, k) => `num:${(size - 1 - k) * 10}`).join()
        && back.isShowValue === c.show
      if (!ok)
        bad.push(`${c.name} reverse=${c.reverse}: ${JSON.stringify(back)}`)
    }
    assert.deepEqual(bad, [])
  })
})
