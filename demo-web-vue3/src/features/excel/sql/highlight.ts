/** SQL 语法高亮：把文本切成带类型的片段（纯函数，编辑器与单测共用） */

export type TokenType = 'keyword' | 'function' | 'string' | 'number' | 'comment' | 'quoted' | 'word' | 'punct' | 'space'

export interface Token {
  type: TokenType
  text: string
}

export const KEYWORDS = [
  'SELECT',
  'FROM',
  'WHERE',
  'GROUP',
  'BY',
  'ORDER',
  'HAVING',
  'LIMIT',
  'OFFSET',
  'JOIN',
  'LEFT',
  'RIGHT',
  'INNER',
  'OUTER',
  'FULL',
  'CROSS',
  'ON',
  'USING',
  'AS',
  'AND',
  'OR',
  'NOT',
  'IN',
  'IS',
  'NULL',
  'LIKE',
  'ILIKE',
  'BETWEEN',
  'CASE',
  'WHEN',
  'THEN',
  'ELSE',
  'END',
  'DISTINCT',
  'UNION',
  'ALL',
  'INTERSECT',
  'EXCEPT',
  'WITH',
  'ASC',
  'DESC',
  'NULLS',
  'FIRST',
  'LAST',
  'OVER',
  'PARTITION',
  'ROWS',
  'RANGE',
  'TRUE',
  'FALSE',
  'CAST',
  'EXISTS',
  'FILTER',
  'QUALIFY',
  'INTERVAL',
] as const

export const FUNCTIONS = [
  'sum',
  'count',
  'avg',
  'min',
  'max',
  'median',
  'quantile_cont',
  'approx_count_distinct',
  'round',
  'floor',
  'ceil',
  'abs',
  'coalesce',
  'nullif',
  'ifnull',
  'length',
  'lower',
  'upper',
  'trim',
  'substr',
  'replace',
  'concat',
  'strftime',
  'date_trunc',
  'date_part',
  'date_diff',
  'year',
  'month',
  'day',
  'row_number',
  'rank',
  'dense_rank',
  'lag',
  'lead',
  'first_value',
  'last_value',
  'string_agg',
  'list',
  'regexp_matches',
  'try_cast',
] as const

const KEYWORD_SET = new Set<string>(KEYWORDS)
const FUNCTION_SET = new Set<string>(FUNCTIONS)

// 依次尝试：行注释、块注释、字符串、带引号的标识符、数字、单词（含中文）、空白、其他符号
const TOKEN_RE = /(--[^\n]*|\/\*[\s\S]*?(?:\*\/|$))|('(?:''|[^'])*'?)|("(?:""|[^"])*"?)|(\d+(?:\.\d+)?)|([\p{L}_][\p{L}\p{N}_$]*)|(\s+)|(\S)/gu

export function tokenize(sql: string): Token[] {
  const out: Token[] = []
  for (const m of sql.matchAll(TOKEN_RE)) {
    const [text, comment, str, quoted, num, word, space] = m
    if (comment) {
      out.push({ type: 'comment', text })
    }
    else if (str) {
      out.push({ type: 'string', text })
    }
    else if (quoted) {
      out.push({ type: 'quoted', text })
    }
    else if (num) {
      out.push({ type: 'number', text })
    }
    else if (word) {
      const lower = word.toLowerCase()
      out.push({ type: KEYWORD_SET.has(word.toUpperCase()) ? 'keyword' : FUNCTION_SET.has(lower) ? 'function' : 'word', text })
    }
    else if (space) {
      out.push({ type: 'space', text })
    }
    else {
      out.push({ type: 'punct', text })
    }
  }
  return out
}

const ESC: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;' }
const escapeHtml = (s: string) => s.replace(/[&<>]/g, c => ESC[c]!)

/**
 * 高亮后的 HTML。函数名只有后面紧跟 `(` 时才着色（避免把叫 count 的列名染成函数）；
 * 末尾补一个换行占位，让叠在下面的 <pre> 与上面的 <textarea> 高度一致
 */
export function highlightHtml(sql: string): string {
  const tokens = tokenize(sql)
  let html = ''
  tokens.forEach((t, i) => {
    let type: TokenType = t.type
    if (type === 'function') {
      const next = tokens.slice(i + 1).find(x => x.type !== 'space')
      if (next?.text !== '(')
        type = 'word'
    }
    html += type === 'word' || type === 'space' || type === 'punct' ? escapeHtml(t.text) : `<span class="tk-${type}">${escapeHtml(t.text)}</span>`
  })
  return `${html}\n`
}
