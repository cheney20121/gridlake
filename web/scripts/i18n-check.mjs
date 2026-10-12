#!/usr/bin/env node
/**
 * 国际化检查：`pnpm i18n:check`
 *
 *  - 代码里用到的 `t('中文')` / `$t('中文')` 的键，必须都在 src/i18n/locales/en-US.json 里（缺了英文界面会显示中文）
 *  - 键和译文里的占位符（{0} {name}）必须一致（译文可以是空串：中文里被拆开的零碎词，英文不需要）
 *  - 英文表里没人用的键（改了原文没同步删旧的）列出来，加 --strict 时也算失败
 *  - 顺带统计还没迁移的界面中文（仅供参考，不影响退出码）
 *
 * 动态拼出来的键（模板字符串里带 ${}）认不出来，请改成带占位符的写法：t('共 {0} 行', [n])
 */
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const SRC = path.join(ROOT, 'src')
const strict = process.argv.includes('--strict')
const en = JSON.parse(fs.readFileSync(path.join(SRC, 'i18n/locales/en-US.json'), 'utf8'))

function walk(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name)
    return e.isDirectory() ? walk(p) : /\.(?:vue|ts)$/.test(e.name) ? [p] : []
  })
}

// 前面不是字母 / 数字 / 下划线 / 点（排除 split( .at( 之类），第一个参数是字符串字面量。
// tr 是文件里已有局部变量 t 时的别名（import { t as tr } from '@/i18n'）
const CALL = /(?<![\w.])\$?(?:t|tr)\(\s*(['"`])((?:\\.|(?!\1)[^\\])*)\1/g
const CJK = /\p{Script=Han}/u
const CALL_START = /(?<![\w.])\$?(?:t|tr)\(\s*['"`]/
const used = new Map() // 键 → 第一次出现的位置
const dynamic = []

const files = walk(SRC).filter(f => !f.includes(`${path.sep}i18n${path.sep}`))
for (const file of files) {
  const text = fs.readFileSync(file, 'utf8')
  for (const m of text.matchAll(CALL)) {
    const quote = m[1]
    const raw = m[2]
    const line = text.slice(0, m.index).split('\n').length
    const where = `${path.relative(ROOT, file)}:${line}`
    if (quote === '`' && raw.includes('${')) {
      dynamic.push(where)
      continue
    }
    const key = raw.replace(/\\(['"`\\])/g, '$1').replace(/\\n/g, '\n')
    if (!CJK.test(key))
      continue // t('OK') 之类不含中文的不是我们的键
    if (!used.has(key))
      used.set(key, where)
  }
}

const holders = s => [...s.matchAll(/\{(\w+)\}/g)].map(m => m[1]).sort().join(',')
const missing = [...used].filter(([k]) => !(k in en))
const mismatch = Object.entries(en).filter(([k, v]) => holders(k) !== holders(v))
const unused = Object.keys(en).filter(k => !used.has(k))

function report(title, rows, fmt) {
  if (!rows.length)
    return
  console.log(`\n${title}（${rows.length}）`)
  for (const r of rows.slice(0, 40)) console.log(`  ${fmt(r)}`)
  if (rows.length > 40)
    console.log(`  … 还有 ${rows.length - 40} 条`)
}

report('缺少英文的键', missing, ([k, w]) => `${w}  ${JSON.stringify(k)}`)
report('占位符和原文不一致的译文', mismatch, ([k, v]) => `${JSON.stringify(k)} → ${JSON.stringify(v)}`)
report('动态拼接的键（认不出来，请改成带占位符的写法）', dynamic, w => w)
report('英文表里没有被代码用到的键', unused, k => JSON.stringify(k))
const bad = missing.length + mismatch.length + dynamic.length + (strict ? unused.length : 0)

// 还没迁移的界面中文：含中文、不是注释、也不在 t() 调用里的行，粗略统计
let lines = 0
let nfiles = 0
for (const file of files) {
  let n = 0
  for (const l of fs.readFileSync(file, 'utf8').split('\n')) {
    const s = l.trim()
    const hasCall = CALL_START.test(s)
    if (CJK.test(s) && !/^(?:\/\/|\/\*|\*|<!--)/.test(s) && !hasCall)
      n++
  }
  if (n) {
    lines += n
    nfiles++
  }
}
console.log(`\n已翻译 ${used.size} 条；还有约 ${lines} 行含中文的代码（${nfiles} 个文件）没有迁移到 t()，英文界面里显示中文。`)
if (bad) {
  console.log(`\n国际化检查失败：${bad} 个问题`)
  process.exit(1)
}
console.log('国际化检查通过')
