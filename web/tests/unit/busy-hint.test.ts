import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { busyNote, errorText, formatElapsed } from '../../src/utils/busy-hint.ts'

describe('等待时间', () => {
  it('8 秒 / 1 分 05 秒', () => {
    assert.equal(formatElapsed(0), '0 秒')
    assert.equal(formatElapsed(8.9), '8 秒')
    assert.equal(formatElapsed(65), '1 分 05 秒')
    assert.equal(formatElapsed(600), '10 分 00 秒')
    assert.equal(formatElapsed(-3), '0 秒')
  })
})

describe('等待说明', () => {
  it('3 秒内不打扰', () => {
    assert.equal(busyNote(0, null), '')
    assert.equal(busyNote(2, { limit: 2, running: 2, waiting: 3 }), '')
  })
  it('名额占满且有人排队时说明在排队', () => {
    const note = busyNote(4, { limit: 2, running: 2, waiting: 3 })
    assert.match(note, /2 个大计算/)
    assert.match(note, /3 个在排队/)
  })
  it('没有排队时解释为什么慢；很久以后建议缩小范围', () => {
    assert.match(busyNote(5, { limit: 2, running: 1, waiting: 0 }), /首次排序/)
    assert.match(busyNote(5, null), /首次排序/)
    assert.match(busyNote(31, null), /缩小筛选范围/)
  })
  it('名额占满但没人排队（就是自己在算）不说排队', () => {
    assert.doesNotMatch(busyNote(5, { limit: 1, running: 1, waiting: 0 }), /排队/)
  })
})

describe('错误文案', () => {
  const axiosErr = (extra: object) => ({ isAxiosError: true, message: 'Request failed with status code 503', ...extra })
  it('优先用服务端的说明', () => {
    assert.equal(errorText(axiosErr({ response: { data: { msg: '服务繁忙：已有 2 个大计算在进行' } } })), '服务繁忙：已有 2 个大计算在进行')
  })
  it('没有响应：区分超时与连不上', () => {
    assert.match(errorText(axiosErr({ code: 'ECONNABORTED' })), /等待超时/)
    assert.match(errorText(axiosErr({ code: 'ERR_NETWORK' })), /无法连接服务/)
  })
  it('字符串、普通错误与兜底', () => {
    assert.equal(errorText('登录失效'), '登录失效')
    assert.equal(errorText(new Error('boom')), 'boom')
    assert.equal(errorText(null, '计算失败'), '计算失败')
    assert.equal(errorText(''), '操作失败，请重试')
  })
})
