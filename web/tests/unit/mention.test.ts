import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { activeMention, insertMention, mentionsIn, splitMentions } from '../../src/features/excel/comments/mention.ts'

describe('activeMention', () => {
  it('光标前正在输入 @关键字', () => {
    assert.deepEqual(activeMention('请 @al', 5), { start: 2, keyword: 'al' })
    assert.deepEqual(activeMention('@', 1), { start: 0, keyword: '' })
    assert.deepEqual(activeMention('请 @张', 4), { start: 2, keyword: '张' })
  })
  it('邮箱里的 @、关键字后面有空白、没有 @：不算', () => {
    assert.equal(activeMention('a@b.com', 7), null)
    assert.equal(activeMention('请 @Alice 看', 10), null)
    assert.equal(activeMention('没有', 2), null)
  })
  it('只看光标前面的内容', () => {
    assert.deepEqual(activeMention('@al 后面还有字', 3), { start: 0, keyword: 'al' })
  })
})

describe('insertMention', () => {
  it('把「@关键字」换成「@名字 」，光标落在后面', () => {
    const r = insertMention('请 @al 看', 5, 2, 'Alice')
    assert.equal(r.text, '请 @Alice  看')
    assert.equal(r.caret, 9)
  })
})

describe('mentionsIn', () => {
  const picked = [{ id: 'u1', name: 'Alice' }, { id: 'u2', name: 'Bob' }, { id: 'u1', name: 'Alice' }]
  it('只保留文字里还在的，去重', () => {
    assert.deepEqual(mentionsIn('@Alice 你看看', picked).map(m => m.id), ['u1'])
    assert.deepEqual(mentionsIn('没有提到谁', picked), [])
    assert.deepEqual(mentionsIn('@Bob 和 @Alice', picked).map(m => m.id), ['u1', 'u2'])
  })
  it('名字是另一个名字的前缀时不误判；名字里有正则字符', () => {
    assert.deepEqual(mentionsIn('@Alice2 好', [{ id: 'u1', name: 'Alice' }]), [])
    assert.deepEqual(mentionsIn('@a.b+c 好', [{ id: 'u3', name: 'a.b+c' }]).map(m => m.id), ['u3'])
  })
})

describe('splitMentions', () => {
  it('切成普通文字与 @ 片段', () => {
    assert.deepEqual(splitMentions('请 @Alice 和 @Bob 看', [{ id: 'u1', name: 'Alice' }, { id: 'u2', name: 'Bob' }]), [
      { text: '请 ', mention: false },
      { text: '@Alice', mention: true },
      { text: ' 和 ', mention: false },
      { text: '@Bob', mention: true },
      { text: ' 看', mention: false },
    ])
  })
  it('没有 @ 的人时原样返回', () => {
    assert.deepEqual(splitMentions('普通文字', []), [{ text: '普通文字', mention: false }])
  })
})
