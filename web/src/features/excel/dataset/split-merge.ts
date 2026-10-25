/**
 * 拆分 / 合并列 → 计算列公式（服务端 calc.rs 的 SPLITPART / TEXTAFTER / TEXTJOIN / CONCAT）。
 * 做成计算列而不是改数据：之后追加、同步进来的行也自动拆好，删掉计算列就回到原样
 */

/** 公式里的文本：Excel 写法，双引号写两遍 */
export function quoteText(s: string): string {
  return `"${s.replace(/"/g, '""')}"`
}

/** 公式里的列引用 */
export function columnRef(title: string): string {
  return `[${title}]`
}

export interface SplitOptions {
  /** 每段去掉首尾空白 */
  trim: boolean
  /** 最后一列保留剩下的全部（段数多于列数时不丢内容） */
  rest: boolean
}

/** 拆成 names.length 列，每列一个公式 */
export function splitFormulas(column: string, separator: string, count: number, opts: SplitOptions): string[] {
  const c = columnRef(column)
  const d = quoteText(separator)
  return Array.from({ length: count }, (_, i) => {
    const n = i + 1
    let f = opts.rest && n === count && n > 1
      ? `TEXTAFTER(${c},${d},${n - 1})`
      : `SPLITPART(${c},${d},${n})`
    if (opts.rest && n === count && n === 1)
      f = c
    return `=${opts.trim ? `TRIM(${f})` : f}`
  })
}

/** 合并几列：有分隔符用 TEXTJOIN，没有用 CONCAT（空值按空字符串） */
export function mergeFormula(columns: string[], separator: string, ignoreEmpty: boolean): string {
  const refs = columns.map(columnRef).join(',')
  return separator ? `=TEXTJOIN(${quoteText(separator)},${ignoreEmpty ? 'TRUE' : 'FALSE'},${refs})` : `=CONCAT(${refs})`
}

/** 拆出来的各段按列数整理：rest 时多出来的段并进最后一列 */
export function fitParts(parts: string[], count: number, separator: string, rest: boolean): string[] {
  const out = parts.slice(0, count)
  if (rest && parts.length > count && count > 0)
    out[count - 1] = parts.slice(count - 1).join(separator)
  while (out.length < count)
    out.push('')
  return out
}

/** 默认的新列名：原列名加序号，避开已有的列名 */
export function defaultNames(title: string, count: number, taken: string[]): string[] {
  const used = new Set(taken)
  const out: string[] = []
  let i = 1
  while (out.length < count) {
    const name = `${title}${i}`
    if (!used.has(name)) {
      out.push(name)
      used.add(name)
    }
    i++
  }
  return out
}

/** 新列名的问题：空的、重名的、和已有列重名的、太长的、含 [ ] " 的；没问题返回 '' */
export function nameProblem(names: string[], taken: string[]): string {
  const seen = new Set<string>()
  for (const raw of names) {
    const n = raw.trim()
    if (!n)
      return 'empty'
    if (n.length > 40)
      return `long:${n}`
    if (/[[\]"]/.test(n))
      return `char:${n}`
    if (taken.includes(n))
      return `taken:${n}`
    if (seen.has(n))
      return `dup:${n}`
    seen.add(n)
  }
  return ''
}
