/**
 * 数据表公式的两种写法互相转换
 *
 * - 输入：用户可以按 Excel 表格引用的习惯写 `=SUM(订单表[金额])`、`=SUMIFS(订单表[金额], 订单表[店铺], B2)`，
 *   提交后自动转成平台函数 `=TSUM("订单表[金额]")`、`=TSUM("订单表[金额]", "店铺", B2)`，由服务端按数据表的筛选计算
 *   （Univer 的表格引用只能指向单元格区域、在浏览器里逐格算，做不到下推）
 * - 导出 xlsx：数据表导出为 Excel 表格（已按数据表的筛选取数），平台函数再转回 `SUM(订单表[金额])` 等原生公式，
 *   在 Excel 里照样能算
 *
 * 只处理函数调用的结构（字符串、括号、逗号），不是完整的公式解析器：认不出的写法原样保留
 */

/** 文本片段，或一次函数调用（name 为空表示普通括号） */
type Seg = { kind: 'text', text: string } | { kind: 'call', name: string, args: string[] }

const NAME_TAIL = /([A-Z_][\w.]*)\s*$/i

/** 把公式文本按最外层的函数调用切开（参数保留原始文本，含空白） */
function split(text: string): Seg[] {
  const out: Seg[] = []
  let buf = ''
  let i = 0
  while (i < text.length) {
    const ch = text[i]!
    if (ch === '"') {
      const end = stringEnd(text, i)
      buf += text.slice(i, end)
      i = end
      continue
    }
    if (ch !== '(') {
      buf += ch
      i++
      continue
    }
    // 函数名（左括号前紧挨着的标识符）；没有就是普通括号
    const m = NAME_TAIL.exec(buf)
    const name = m ? m[1]! : ''
    if (m)
      buf = buf.slice(0, buf.length - m[0].length)
    if (buf)
      out.push({ kind: 'text', text: buf })
    buf = ''
    const { args, end } = readArgs(text, i + 1)
    out.push({ kind: 'call', name, args })
    i = end
  }
  if (buf)
    out.push({ kind: 'text', text: buf })
  return out
}

/** 字符串字面量结束位置（"" 为转义的引号） */
function stringEnd(text: string, start: number) {
  let i = start + 1
  while (i < text.length) {
    if (text[i] === '"') {
      if (text[i + 1] === '"') {
        i += 2
        continue
      }
      return i + 1
    }
    i++
  }
  return text.length
}

/** 从左括号后读参数，返回参数原文与右括号后的位置 */
function readArgs(text: string, start: number): { args: string[], end: number } {
  const args: string[] = []
  let depth = 0
  let cur = ''
  let i = start
  while (i < text.length) {
    const ch = text[i]!
    if (ch === '"') {
      const end = stringEnd(text, i)
      cur += text.slice(i, end)
      i = end
      continue
    }
    if (ch === '(' || ch === '{') {
      depth++
    }
    else if (ch === ')' || ch === '}') {
      if (depth === 0 && ch === ')') {
        args.push(cur)
        return { args: args.length === 1 && !args[0]!.trim() ? [] : args, end: i + 1 }
      }
      depth--
    }
    else if (ch === ',' && depth === 0) {
      args.push(cur)
      cur = ''
      i++
      continue
    }
    cur += ch
    i++
  }
  args.push(cur)
  return { args, end: text.length }
}

/** 由内向外改写函数调用：fn 返回新文本，或 null 保持原样（参数仍按改写后的拼回） */
export function rewriteCalls(text: string, fn: (name: string, args: string[]) => string | null): string {
  return split(text).map((s) => {
    if (s.kind === 'text')
      return s.text
    const args = s.args.map(a => rewriteCalls(a, fn))
    return fn(s.name, args) ?? `${s.name}(${args.join(',')})`
  }).join('')
}

/** 字符串字面量的内容；不是字面量返回 null */
export function literal(arg: string): string | null {
  const m = /^\s*"((?:[^"]|"")*)"\s*$/.exec(arg)
  return m ? m[1]!.replace(/""/g, '"') : null
}

const quote = (s: string) => `"${s.replace(/"/g, '""')}"`

/** 表格引用 `表名[列名]`（不带引号）；不是返回 null */
export function tableRef(arg: string): { table: string, column: string } | null {
  const m = /^\s*([^\s"'[\](),:!+\-*/&=<>^]+)\[([^[\]]+)\]\s*$/.exec(arg)
  return m ? { table: m[1]!, column: m[2]!.trim() } : null
}

// ── 输入：表格引用 → 平台函数 ──

const AGG: Record<string, string> = { SUM: 'TSUM', AVERAGE: 'TAVERAGE', MIN: 'TMIN', MAX: 'TMAX' }
const IFS: Record<string, string> = { SUMIFS: 'TSUM', AVERAGEIFS: 'TAVERAGE', MINIFS: 'TMIN', MAXIFS: 'TMAX' }
const IF1: Record<string, string> = { SUMIF: 'TSUM', AVERAGEIF: 'TAVERAGE' }

/**
 * 把公式里对数据表的 Excel 表格引用改写为平台函数；isTable 判断表名是不是工作簿里的数据表（返回规范的表名）
 * 没有可改写的内容时返回 null
 */
export function toPlatformFormula(formula: string, isTable: (name: string) => string | null): string | null {
  if (!formula.startsWith('=') || !formula.includes('['))
    return null
  let changed = false
  const out = rewriteCalls(formula, (name, args) => {
    const N = name.toUpperCase()
    const refs = args.map(tableRef)
    const tableOf = (i: number) => {
      const r = refs[i]
      return r ? isTable(r.table) : null
    }
    const ref = (i: number) => refs[i]!
    const done = (s: string) => {
      changed = true
      return s
    }
    // 条件对：(表[列], 条件) …，所有列须属于同一张数据表
    const pairs = (from: number, table: string): string[] | null => {
      const out: string[] = []
      for (let i = from; i < args.length; i += 2) {
        if (i + 1 >= args.length || tableOf(i) !== table)
          return null
        out.push(quote(ref(i).column), args[i + 1]!.trim())
      }
      return out
    }
    if (AGG[N] && args.length === 1 && tableOf(0))
      return done(`${AGG[N]}(${quote(`${tableOf(0)}[${ref(0).column}]`)})`)
    if ((N === 'COUNT' || N === 'COUNTA') && args.length === 1 && tableOf(0))
      return done(`TCOUNT(${quote(tableOf(0)!)}, ${quote(ref(0).column)}, "<>")`)
    if (N === 'ROWS' && args.length === 1 && tableOf(0))
      return done(`TCOUNT(${quote(tableOf(0)!)})`)
    if (IFS[N] && args.length >= 3 && args.length % 2 === 1 && tableOf(0)) {
      const p = pairs(1, tableOf(0)!)
      if (p)
        return done(`${IFS[N]}(${[quote(`${tableOf(0)}[${ref(0).column}]`), ...p].join(', ')})`)
    }
    if (N === 'COUNTIFS' && args.length >= 2 && args.length % 2 === 0 && tableOf(0)) {
      const p = pairs(0, tableOf(0)!)
      if (p)
        return done(`TCOUNT(${[quote(tableOf(0)!), ...p].join(', ')})`)
    }
    if (N === 'COUNTIF' && args.length === 2 && tableOf(0))
      return done(`TCOUNT(${quote(tableOf(0)!)}, ${quote(ref(0).column)}, ${args[1]!.trim()})`)
    // SUMIF(条件列, 条件, 求和列) / AVERAGEIF
    if (IF1[N] && args.length === 3 && tableOf(0) && tableOf(2) === tableOf(0))
      return done(`${IF1[N]}(${quote(`${tableOf(2)}[${ref(2).column}]`)}, ${quote(ref(0).column)}, ${args[1]!.trim()})`)
    if (N === 'INDEX' && args.length === 2 && tableOf(0))
      return done(`TVALUE(${quote(`${tableOf(0)}[${ref(0).column}]`)}, ${args[1]!.trim()})`)
    return null
  })
  return changed ? out : null
}

// ── 导出：平台函数 → Excel 原生公式 ──

const BACK: Record<string, [string, string]> = { TSUM: ['SUM', 'SUMIFS'], TAVERAGE: ['AVERAGE', 'AVERAGEIFS'], TMIN: ['MIN', 'MINIFS'], TMAX: ['MAX', 'MAXIFS'] }

/** Excel 结构化引用里列名的特殊字符要用 ' 转义 */
function excelColumn(c: string) {
  return c.replace(/(['#[\]])/g, '\'$1')
}

/**
 * 平台函数（TSUM 等）转回 Excel 原生公式；table(名称) 返回导出后的 Excel 表名与列名集合，不能导出时返回 null
 * 有任何一处转不了时返回 null（调用方改为导出数值）；没有平台函数时原样返回
 */
export function toExcelFormula(formula: string, table: (name: string) => { name: string, columns: Set<string> } | null): string | null {
  let failed = false
  const out = rewriteCalls(formula, (name, args) => {
    const N = name.toUpperCase()
    if (!BACK[N] && N !== 'TCOUNT' && N !== 'TVALUE')
      return null
    const fail = () => {
      failed = true
      return null
    }
    const first = literal(args[0] ?? '')
    if (first == null)
      return fail()
    const m = /^([^[\]]+)(?:\[([^[\]]+)\])?$/.exec(first.trim())
    const t = m ? table(m[1]!.trim()) : null
    if (!m || !t)
      return fail()
    const col = m[2]?.trim()
    const ref = (c: string) => (t.columns.has(c) ? `${t.name}[${excelColumn(c)}]` : null)
    const crit: string[] = []
    const critEnd = N === 'TVALUE' ? 1 : args.length
    for (let i = 1; i < critEnd; i += 2) {
      const c = literal(args[i] ?? '')
      const r = c == null ? null : ref(c.trim())
      if (!r || i + 1 >= args.length)
        return fail()
      crit.push(r, args[i + 1]!.trim())
    }
    if (N === 'TCOUNT')
      return crit.length ? `COUNTIFS(${crit.join(', ')})` : `ROWS(${t.name})`
    const target = col ? ref(col) : null
    if (!target)
      return fail()
    if (N === 'TVALUE')
      return args.length === 2 ? `INDEX(${target}, ${args[1]!.trim()})` : fail()
    const [plain, ifs] = BACK[N]!
    return crit.length ? `${ifs}(${[target, ...crit].join(', ')})` : `${plain}(${target})`
  })
  return failed ? null : out
}

/** 公式里是否有数据表函数 */
export const TABLE_FUNCTION = /\b(?:TSUM|TCOUNT|TAVERAGE|TMIN|TMAX|TVALUE)\s*\(/i

/** 数据表改名：把数据表函数里引用旧表名的字符串改成新表名；没有引用时返回 null */
export function renameTableInFormula(formula: string, from: string, to: string): string | null {
  if (!TABLE_FUNCTION.test(formula))
    return null
  const old = from.trim().toLowerCase()
  let changed = false
  const out = rewriteCalls(formula, (name, args) => {
    if (!TABLE_FUNCTION.test(`${name}(`))
      return null
    const first = literal(args[0] ?? '')
    if (first == null)
      return null
    const m = /^([^[\]]+)(\[[^[\]]+\])?$/.exec(first.trim())
    if (!m || m[1]!.trim().toLowerCase() !== old)
      return null
    changed = true
    const lead = /^\s*/.exec(args[0]!)![0]
    return `${name}(${[`${lead}${quote(`${to}${m[2] ?? ''}`)}`, ...args.slice(1)].join(',')})`
  })
  return changed ? out : null
}

/** 公式里数据表函数用到的列（求值列与条件列），按表名（小写）归类 */
export function tableColumnsUsed(formula: string, into: Map<string, Set<string>>) {
  if (!TABLE_FUNCTION.test(formula))
    return
  rewriteCalls(formula, (name, args) => {
    if (!TABLE_FUNCTION.test(`${name}(`))
      return null
    const first = literal(args[0] ?? '')
    const m = first == null ? null : /^([^[\]]+)(?:\[([^[\]]+)\])?$/.exec(first.trim())
    if (!m)
      return null
    const key = m[1]!.trim().toLowerCase()
    const set = into.get(key) ?? new Set<string>()
    if (m[2])
      set.add(m[2].trim())
    if (name.toUpperCase() !== 'TVALUE') {
      for (let i = 1; i < args.length; i += 2) {
        const c = literal(args[i] ?? '')
        if (c != null)
          set.add(c.trim())
      }
    }
    into.set(key, set)
    return null
  })
}
