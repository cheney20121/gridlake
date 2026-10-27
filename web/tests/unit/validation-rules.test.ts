import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { emptyDraft, fromRule, parseValues, toRule } from '../../src/features/excel/dataset/validation-rules.ts'

describe('数据校验规则的编辑草稿', () => {
  it('草稿转规则：范围按数字或原文、允许的取值按逗号 / 顿号 / 换行拆开去重', () => {
    assert.deepEqual(toRule({ ...emptyDraft('金额'), kind: 'range', min: '0', max: '10,000' }), { column: '金额', kind: 'range', min: 0, max: 10000 })
    assert.deepEqual(toRule({ ...emptyDraft('日期'), kind: 'range', min: '2026-09-01', max: '' }), { column: '日期', kind: 'range', min: '2026-09-01', max: null })
    assert.deepEqual(toRule({ ...emptyDraft('手机'), kind: 'format', format: 'phone', pattern: 'x' }), { column: '手机', kind: 'format', format: 'phone' })
    assert.deepEqual(toRule({ ...emptyDraft('码'), kind: 'format', format: 'regex', pattern: ' A\\d+ ' }), { column: '码', kind: 'format', format: 'regex', pattern: 'A\\d+' })
    assert.deepEqual(parseValues('金卡，银卡、金卡\n 铜卡 ,'), ['金卡', '银卡', '铜卡'])
    assert.deepEqual(toRule({ ...emptyDraft('编号'), kind: 'unique' }), { column: '编号', kind: 'unique' })
  })
  it('规则转回草稿', () => {
    for (const r of [
      { column: '金额', kind: 'range', min: 0, max: 10000 },
      { column: '手机', kind: 'format', format: 'regex', pattern: '1\\d{10}' },
      { column: '等级', kind: 'allowed', values: ['金卡', '银卡'] },
      { column: '编号', kind: 'required' },
    ] as const)
      assert.deepEqual(toRule(fromRule(r as never)), r)
  })
})
