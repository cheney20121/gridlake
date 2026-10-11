import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { initialsOf, matchInitials } from '../../src/utils/pinyin-initials.ts'

const first = (text: string) => initialsOf(text).map(x => x[0]).join('')

/** 界面文字里出现的单音字（280 个）及其首字母，来自独立的拼音库 pypinyin，用来校验排序规则的分界 */
const CHARS = '一上下与业个中为义习书了交享人他令仪企作你保修偏像元入全公关再写冲冻出击函分切列则创删别到制刷办功加务动劲勾化千协历发取口另只史同名后含员命和哪售器回围图在处复外多头好如始子字存定实审对导就展工布带常并序库店建开异引当录往待微心必志快态性总恢惯意慢成户手打执批报持指按据推操收改放数整文断新日时明易显更未本权条板构染柱标栏样案档检模正步水汇没流测渲演点照片版牌状现理生用由界留略登的盘目看稍突站等答签算管簿索级线组细结编联自色范菜表要视计订认议记设诊词试询误读谁负责败账资起跨载辑过近返这进连退送选通邮都里量金钮钻铺链销锁错键问阅隐集面页顶项顺题颜额飞饼首骤默'
const EXPECT = 'ysxyygzwyxsljxrtlyqznbxpxyrqggzxcdcjhfqlzcsbdzsbgjwdjghqxlfqklzstmhhymhnsqhwtzcfwdthrszzcdssddjzgbdcbxkdjkyydlwdwxbzktxzhgymchsdzpbczajtcsgfszwdxrsmyxgwbqtbgrzblyadjmzbshmlcxydzpbpzxlsyyjllddpmkstzddqsgbsjxzxjblzsfcbysjdryjszcsxwdsfzbzzqkzjgjfzjltsxtydlljnzplxscjwyyjmydxstyefbszm'

describe('initialsOf', () => {
  it('界面文字里的 280 个单音字逐字与独立拼音库一致', () => {
    const wrong = [...CHARS].filter((ch, i) => first(ch) !== EXPECT[i]).map(ch => `${ch}→${first(ch)}`)
    assert.deepEqual(wrong, [])
  })

  it('业务词', () => {
    assert.equal(first('数据集'), 'sjj')
    assert.equal(first('透视表'), 'tsb')
    assert.equal(first('逐笔对账'), 'zbdz')
    assert.equal(first('异常发现'), 'ycfx')
    assert.equal(first('仪表盘'), 'ybp')
    assert.equal(first('定时推送'), 'dsts')
  })

  it('字母数字取自身，符号与空格跳过', () => {
    assert.deepEqual(initialsOf('SQL 查询 (1)'), [['s'], ['q'], ['l'], ['c', 'z'], ['x'], ['1']])
  })

  it('多音字给出多个首字母', () => {
    const [h] = initialsOf('行')
    assert.ok(h!.includes('h') && h!.includes('x'))
  })
})

describe('matchInitials', () => {
  it('首字母连续匹配，可从中间开始', () => {
    assert.ok(matchInitials('数据集', 'sjj'))
    assert.ok(matchInitials('数据集', 'sj'))
    assert.ok(matchInitials('导入数据集', 'sjj'))
    assert.ok(!matchInitials('导入数据集', 'sjj', true))
    assert.ok(matchInitials('导入数据集', 'drs', true))
    assert.ok(!matchInitials('数据集', 'sjk'))
    assert.ok(!matchInitials('数据集', 'sjjj'))
  })

  it('多音字两种读音都能匹配', () => {
    assert.ok(matchInitials('隐藏行', 'ycx'))
    assert.ok(matchInitials('隐藏行', 'ych'))
  })

  it('大小写不敏感；含汉字的查询不处理', () => {
    assert.ok(matchInitials('数据集', 'SJJ'))
    assert.ok(!matchInitials('数据集', '数据'))
    assert.ok(!matchInitials('数据集', ''))
  })

  it('混合中英文标题', () => {
    assert.ok(matchInitials('SQL 查询', 'sqlcx'))
    assert.ok(matchInitials('SQL 查询', 'cx'))
  })
})
