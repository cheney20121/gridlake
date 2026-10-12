import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { complete, quoteIfNeeded, referencedDatasets } from '../../src/features/excel/sql/complete.ts'
import { highlightHtml, tokenize } from '../../src/features/excel/sql/highlight.ts'

const datasets = [
  { name: '订单', rowCount: 6, columns: [{ title: '店铺', kind: 'text' }, { title: '金额', kind: 'money' }, { title: '下单时间', kind: 'datetime' }] },
  { name: '店铺信息', rowCount: 3, columns: [{ title: '店铺', kind: 'text' }, { title: '负责人', kind: 'text' }] },
  { name: 'daily sales', rowCount: 10, columns: [{ title: 'order id', kind: 'id' }, { title: 'amount', kind: 'money' }] },
]
const at = (text: string, explicit = false) => complete(text.replace('|', ''), text.indexOf('|'), datasets, explicit)
const labels = (c: ReturnType<typeof at>) => c?.items.map(i => i.label) ?? []

describe('SQL 高亮', () => {
  it('关键字 / 字符串 / 数字 / 注释 / 带引号的标识符', () => {
    const t = tokenize('SELECT "店 铺", 1.5, \'a\'\'b\' -- 注释\nFROM 订单').filter(x => x.type !== 'space' && x.type !== 'punct')
    assert.deepEqual(t.map(x => x.type), ['keyword', 'quoted', 'number', 'string', 'comment', 'keyword', 'word'])
    assert.equal(t[3]!.text, '\'a\'\'b\'')
  })
  it('函数名只有后面跟括号才着色；HTML 转义；末尾补换行占位', () => {
    assert.match(highlightHtml('select sum(金额) from t'), /<span class="tk-function">sum<\/span>\(/)
    assert.doesNotMatch(highlightHtml('select count from t'), /tk-function/)
    assert.equal(highlightHtml('a < b & c'), 'a &lt; b &amp; c\n')
    assert.ok(highlightHtml('x').endsWith('\n'))
  })
  it('没写完的字符串 / 块注释不会让后面的内容消失', () => {
    assert.equal(tokenize('select \'abc').map(x => x.text).join(''), 'select \'abc')
    assert.equal(tokenize('/* 没结束\nselect 1').map(x => x.text).join(''), '/* 没结束\nselect 1')
  })
  it('任何输入切片后拼回去都等于原文', () => {
    const s = 'WITH a AS (SELECT 1) SELECT "x""y", \'it\'\'s\', 3.14 FROM 订单 -- 末尾'
    assert.equal(tokenize(s).map(x => x.text).join(''), s)
  })
})

describe('SQL 补全', () => {
  it('名称含空格或符号才加引号；汉字名直接写', () => {
    assert.equal(quoteIfNeeded('订单'), '订单')
    assert.equal(quoteIfNeeded('daily sales'), '"daily sales"')
    assert.equal(quoteIfNeeded('a"b'), '"a""b"')
    assert.equal(quoteIfNeeded('select'), '"select"')
    assert.equal(quoteIfNeeded('1abc'), '"1abc"')
  })
  it('FROM / JOIN 之后只给数据集，包含匹配（输入「息」能找到「店铺信息」）', () => {
    assert.deepEqual(labels(at('SELECT * FROM |', true)), ['订单', '店铺信息', 'daily sales'])
    assert.deepEqual(labels(at('select * from 店|')), ['店铺信息'])
    assert.deepEqual(labels(at('select * from 息|')), ['店铺信息'])
    const c = at('select * from dai|')!
    assert.equal(c.items[0]!.insert, '"daily sales"')
    assert.equal(c.from, 'select * from '.length)
  })
  it('查询里用到的数据集的列排在前面，带数据集名与类型', () => {
    const c = at('SELECT 金| FROM 订单')!
    assert.equal(c.items[0]!.label, '金额')
    assert.equal(c.items[0]!.detail, '订单 · 金额')
    assert.equal(c.from, 7)
  })
  it('「别名.」与「数据集名.」之后只给对应数据集的列', () => {
    assert.deepEqual(labels(at('SELECT o.| FROM 订单 o')), ['店铺', '金额', '下单时间'])
    assert.deepEqual(labels(at('SELECT s.负| FROM 订单 AS o JOIN 店铺信息 AS s ON o.店铺 = s.店铺')), ['负责人'])
    assert.deepEqual(labels(at('SELECT 订单.金| FROM 订单')), ['金额'])
    assert.equal(at('SELECT x.| FROM 订单'), null)
  })
  it('双引号里补全：替换区间包含开头的引号', () => {
    const c = at('SELECT "店| FROM 订单')!
    assert.equal(c.from, 7)
    assert.equal(c.items[0]!.label, '店铺')
    const d = at('SELECT * FROM "dai|')!
    assert.equal(d.items[0]!.insert, '"daily sales"')
  })
  it('关键字与函数；输入完整的关键字不再打扰；没前缀不弹（Ctrl+空格除外）', () => {
    assert.ok(labels(at('SEL|')).includes('SELECT'))
    assert.ok(labels(at('SELECT su|')).includes('sum'))
    assert.equal(at('SELECT su|')!.items.find(i => i.label === 'sum')!.insert, 'sum(')
    assert.equal(at('SELECT|'), null)
    assert.equal(at('SELECT |'), null)
    assert.ok(labels(at('SELECT |', true)).length > 0)
  })
  it('字符串和注释里不补全', () => {
    assert.equal(at('SELECT \'金|'), null)
    assert.equal(at('SELECT 1 -- 金|'), null)
    assert.equal(at('/* 金|'), null)
  })
  it('解析别名：AS 可省略；JOIN 与关键字不被当成别名', () => {
    const refs = referencedDatasets('SELECT * FROM 订单 o JOIN 店铺信息 s ON o.店铺 = s.店铺 WHERE 1=1', datasets)
    assert.deepEqual(refs.map(r => r.names), [['o', '订单'], ['s', '店铺信息']])
    const r2 = referencedDatasets('SELECT * FROM 订单 WHERE x', datasets)
    assert.deepEqual(r2.map(r => r.names), [['订单']])
    const r3 = referencedDatasets('SELECT * FROM 订单 LEFT JOIN 店铺信息 ON 1=1', datasets)
    assert.deepEqual(r3.map(r => r.names), [['订单'], ['店铺信息']])
  })
})
