/**
 * 公式文本里的单元格引用：公式诊断（估算开销）和循环引用检测共用。不依赖 Univer，可直接单元测试。
 * 只认 A1 写法的引用（含跨工作表、整列 / 整行）；命名区域、INDIRECT / OFFSET 拼出来的引用、结构化引用看不到
 */

export function colIndex(s: string) {
  let n = 0
  for (const ch of s.toUpperCase())
    n = n * 26 + ch.charCodeAt(0) - 64
  return n - 1
}

/** 去掉字符串常量，避免把 "A1" 这类文字当成引用 */
export function stripStrings(f: string) {
  return f.replace(/"(?:[^"]|"")*"/g, '""')
}

const SHEET = String.raw`(?:(?:'(?:[^']|'')+'|[^\s!(),:'"+\-*/^&=<>]+)!)?`
const CELL = String.raw`\$?[A-Z]{1,3}\$?\d+`
export const REF = new RegExp(String.raw`${SHEET}(?:${CELL}(?::${CELL})?|\$?[A-Z]{1,3}:\$?[A-Z]{1,3}|\$?\d+:\$?\d+)(?![\w(])`, 'gi')

/** 一处引用：工作表名（没写表示公式所在的表）与区域；整列的行号、整行的列号为 null */
export interface RefRange {
  sheet: string | null
  /** 起止列 / 行（从 0 开始，已按大小排好）；整列没有行（null），整行没有列（null） */
  c1: number | null
  c2: number | null
  r1: number | null
  r2: number | null
}

/** 公式里的全部引用 */
export function parseRefs(formula: string): RefRange[] {
  const out: RefRange[] = []
  for (const m of stripStrings(formula).matchAll(REF)) {
    const text = m[0]
    const bang = text.lastIndexOf('!')
    const sheet = bang >= 0 ? text.slice(0, bang).replace(/^'|'$/g, '').replace(/''/g, '\'') : null
    const addr = (bang >= 0 ? text.slice(bang + 1) : text).replace(/\$/g, '')
    const [a, b = a] = addr.split(':')
    const ca = /^[A-Z]+/i.exec(a!)?.[0]
    const cb = /^[A-Z]+/i.exec(b!)?.[0]
    const ra = /\d+$/.exec(a!)?.[0]
    const rb = /\d+$/.exec(b!)?.[0]
    const cols = ca && cb ? [colIndex(ca), colIndex(cb)].sort((x, y) => x - y) : null
    const rows = ra && rb ? [Number(ra) - 1, Number(rb) - 1].sort((x, y) => x - y) : null
    if (!cols && !rows)
      continue
    out.push({ sheet, c1: cols?.[0] ?? null, c2: cols?.[1] ?? null, r1: rows?.[0] ?? null, r2: rows?.[1] ?? null })
  }
  return out
}
