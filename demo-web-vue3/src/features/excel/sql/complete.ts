import { FUNCTIONS, KEYWORDS, tokenize } from './highlight'

/** SQL 补全：根据光标前的文字给出数据集名、列名、关键字、函数（纯函数，编辑器与单测共用） */

export interface SqlDataset {
  name: string
  columns: { title: string, kind: string }[]
  rowCount?: number
}

export type SuggestionType = 'dataset' | 'column' | 'keyword' | 'function'

export interface Suggestion {
  label: string
  /** 替换进文本的内容（名称含空格 / 符号时带双引号） */
  insert: string
  type: SuggestionType
  detail?: string
}

export interface Completion {
  /** 要被替换的区间（含已经输入的前缀与开头的引号） */
  from: number
  to: number
  items: Suggestion[]
}

const MAX_ITEMS = 12
const KIND_LABEL: Record<string, string> = { text: '文本', id: '编号', number: '数值', money: '金额', datetime: '日期时间' }
const WORD_CHAR = /[\p{L}\p{N}_$]/u

/** 名称能直接写就直接写（字母 / 汉字 / 数字 / 下划线，且不以数字开头、不是关键字），否则加双引号 */
export function quoteIfNeeded(name: string): string {
  const plain = /^[\p{L}_][\p{L}\p{N}_$]*$/u.test(name) && !KEYWORDS.includes(name.toUpperCase() as typeof KEYWORDS[number])
  return plain ? name : `"${name.replace(/"/g, '""')}"`
}

const unquote = (id: string) => (id.startsWith('"') ? id.slice(1, id.endsWith('"') && id.length > 1 ? -1 : undefined).replace(/""/g, '"') : id)

interface Ref {
  dataset: SqlDataset
  /** 查询里能用来指代它的名字：别名或数据集名 */
  names: string[]
}

/** 查询里 FROM / JOIN 之后出现的数据集（含别名） */
export function referencedDatasets(text: string, datasets: SqlDataset[]): Ref[] {
  const out: Ref[] = []
  const re = /\b(?:from|join)\s+("(?:""|[^"])*"|[\p{L}_][\p{L}\p{N}_$]*)(?:\s+(?:as\s+)?("(?:""|[^"])*"|[\p{L}_][\p{L}\p{N}_$]*))?/giu
  for (const m of text.matchAll(re)) {
    const name = unquote(m[1]!)
    const ds = datasets.find(d => d.name.toLowerCase() === name.toLowerCase())
    if (!ds)
      continue
    const alias = m[2] && !KEYWORDS.includes(m[2].toUpperCase() as typeof KEYWORDS[number]) ? unquote(m[2]) : ''
    out.push({ dataset: ds, names: alias ? [alias, ds.name] : [ds.name] })
  }
  return out
}

function rank(label: string, prefix: string): number {
  if (!prefix)
    return 1
  const l = label.toLowerCase()
  const p = prefix.toLowerCase()
  if (l === p)
    return 0
  if (l.startsWith(p))
    return 1
  return l.includes(p) ? 2 : -1
}

function pick(items: Suggestion[], prefix: string): Suggestion[] {
  return items
    .map((it, i) => ({ it, r: rank(it.label, prefix), i }))
    .filter(x => x.r >= 0)
    .sort((a, b) => a.r - b.r || a.i - b.i)
    .slice(0, MAX_ITEMS)
    .map(x => x.it)
}

/**
 * 光标处的补全。explicit 为 true（Ctrl + 空格）时没有前缀也给候选；
 * 否则至少输入 1 个字符，或刚输入了「.」才弹出。在字符串 / 注释里不补全
 */
export function complete(text: string, caret: number, datasets: SqlDataset[], explicit = false): Completion | null {
  const before = text.slice(0, caret)
  // 光标在字符串或注释里（最后一个片段没有结束）：不补全
  const bt = tokenize(before)
  const last = bt[bt.length - 1]
  if (last && ((last.type === 'comment' && (last.text.startsWith('--') || !last.text.endsWith('*/'))) || (last.type === 'string' && (last.text.length < 2 || !last.text.endsWith('\'') || last.text === '\''))))
    return null

  // 正在输入的词（可能以双引号开头）
  let start = caret
  while (start > 0 && WORD_CHAR.test(text[start - 1]!))
    start--
  let from = start
  if (from > 0 && text[from - 1] === '"') {
    // 引号开头的标识符：往前确认这个引号是开引号（前面的双引号个数为偶数）
    const quotes = (before.slice(0, from - 1).match(/"/g) ?? []).length
    if (quotes % 2 === 0)
      from--
  }
  const prefix = text.slice(start, caret)
  const quoted = from < start

  // 「别名.」或「数据集.」之后：只给这个数据集的列
  const dotAt = from - 1
  if (dotAt >= 0 && text[dotAt] === '.') {
    let q = dotAt
    while (q > 0 && WORD_CHAR.test(text[q - 1]!))
      q--
    let qualifier = text.slice(q, dotAt)
    if (!qualifier && text[dotAt - 1] === '"') {
      const open = text.lastIndexOf('"', dotAt - 2)
      qualifier = unquote(text.slice(open, dotAt))
    }
    const refs = referencedDatasets(text, datasets)
    const ref = refs.find(r => r.names.some(n => n.toLowerCase() === qualifier.toLowerCase()))
    const ds = ref?.dataset ?? datasets.find(d => d.name.toLowerCase() === qualifier.toLowerCase())
    if (!ds)
      return null
    const items = pick(ds.columns.map(c => ({ label: c.title, insert: quoteIfNeeded(c.title), type: 'column' as const, detail: KIND_LABEL[c.kind] ?? c.kind })), prefix)
    return items.length ? { from, to: caret, items } : null
  }

  if (!prefix && !explicit && !quoted)
    return null

  // 前一个词是 FROM / JOIN：只给数据集
  const prevTokens = tokenize(text.slice(0, from)).filter(t => t.type !== 'space')
  const prev = prevTokens[prevTokens.length - 1]
  const afterFrom = !!prev && prev.type === 'keyword' && ['FROM', 'JOIN'].includes(prev.text.toUpperCase())
  const datasetItems: Suggestion[] = datasets.map(d => ({ label: d.name, insert: quoteIfNeeded(d.name), type: 'dataset', detail: d.rowCount != null ? `${d.rowCount.toLocaleString()} 行` : '数据集' }))
  if (afterFrom) {
    const items = pick(datasetItems, prefix)
    return items.length ? { from, to: caret, items } : null
  }
  if (quoted) {
    // 引号里只可能是数据集名或列名
    const refs = referencedDatasets(text, datasets)
    const cols = (refs.length ? refs.map(r => r.dataset) : datasets).flatMap(d => d.columns.map(c => ({ label: c.title, insert: quoteIfNeeded(c.title), type: 'column' as const, detail: `${d.name} · ${KIND_LABEL[c.kind] ?? c.kind}` })))
    const items = pick([...cols, ...datasetItems], prefix)
    return items.length ? { from, to: caret, items } : null
  }

  // 其余位置：查询里用到的数据集的列 > 数据集 > 关键字 > 函数
  const refs = referencedDatasets(text, datasets)
  const seen = new Set<string>()
  const columns: Suggestion[] = []
  for (const r of refs) {
    for (const c of r.dataset.columns) {
      if (seen.has(c.title))
        continue
      seen.add(c.title)
      columns.push({ label: c.title, insert: quoteIfNeeded(c.title), type: 'column', detail: `${r.dataset.name} · ${KIND_LABEL[c.kind] ?? c.kind}` })
    }
  }
  const keywords: Suggestion[] = KEYWORDS.map(k => ({ label: k, insert: k, type: 'keyword' }))
  const functions: Suggestion[] = FUNCTIONS.map(f => ({ label: f, insert: `${f}(`, type: 'function', detail: '函数' }))
  const items = pick([...columns, ...datasetItems, ...keywords, ...functions], prefix)
  // 已经完整输入的唯一候选（如刚敲完 SELECT）不再打扰
  if (items.length === 1 && items[0]!.label.toLowerCase() === prefix.toLowerCase() && items[0]!.type === 'keyword')
    return null
  return items.length ? { from, to: caret, items } : null
}
