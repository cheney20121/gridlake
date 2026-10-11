/**
 * 单元格模式数据表的「保留手工调整」：在表里手动改的格子，刷新（换数据、切片器变化、重新排序）后仍然保留。
 *
 * 按主键认行——位置会随数据变化，主键不会：记录「主键值 → 列名 → 改成了什么」。刷新写入后，对每一行按主键找到记录并覆盖。
 * 手改的值如果和数据集里的值一样（改回去了、撤销了），记录没有意义，刷新时顺手清掉；当前结果里没有的行（被筛选掉）记录保留，它们再出现时仍然生效。
 * 改主键列本身不记（主键变了就不是同一行了）。公式写成以 = 开头的文本，原样写回。
 */

export type EditValue = string | number | boolean
/** 主键 → 列名 → 手改的值 */
export type TableEdits = Record<string, Record<string, EditValue>>

type CellValue = string | number | boolean | null | undefined

/** 一行的主键：各主键列的值按文本拼起来（数字 1 与文本 "1" 视为同一个） */
export function rowKey(values: CellValue[]): string {
  return JSON.stringify(values.map(v => (v == null ? '' : String(v))))
}

/** 手改的值与数据集里的值是否一样（数字与数字文本、空与空串视为相同） */
export function sameValue(a: CellValue, b: CellValue): boolean {
  const x = a == null ? '' : a
  const y = b == null ? '' : b
  if (x === y)
    return true
  if (typeof x === 'string' && x.startsWith('='))
    return false
  const nx = typeof x === 'number' ? x : x === '' ? Number.NaN : Number(x)
  const ny = typeof y === 'number' ? y : y === '' ? Number.NaN : Number(y)
  if (!Number.isNaN(nx) && !Number.isNaN(ny))
    return nx === ny
  return String(x) === String(y)
}

/** 记一处手改：value 与数据集里的不同才有意义，调用方保证。返回新的记录（不改原对象） */
export function recordEdit(edits: TableEdits | undefined, key: string, column: string, value: EditValue): TableEdits {
  return { ...edits, [key]: { ...edits?.[key], [column]: value } }
}

export interface AppliedEdits {
  rows: CellValue[][]
  /** 清掉与数据集一致的记录后的结果 */
  edits: TableEdits
  /** 这次覆盖了多少个格子 */
  applied: number
}

/**
 * 把手改覆盖到刚取到的数据上。columns：显示列的列名（与 rows 的列对应）；keyColumns：主键列的列名，必须都在显示列里，
 * 否则无法认行，原样返回
 */
export function applyEdits(rows: CellValue[][], columns: string[], keyColumns: string[], edits: TableEdits | undefined): AppliedEdits {
  const keyIdx = keyColumns.map(k => columns.indexOf(k))
  if (!edits || !Object.keys(edits).length || !keyIdx.length || keyIdx.includes(-1))
    return { rows, edits: edits ?? {}, applied: 0 }
  const next: TableEdits = {}
  for (const [k, cols] of Object.entries(edits))
    next[k] = { ...cols }
  const out = rows.map(r => r.slice())
  let applied = 0
  for (const row of out) {
    const key = rowKey(keyIdx.map(i => row[i]))
    const e = next[key]
    if (!e)
      continue
    for (const [col, value] of Object.entries(e)) {
      const j = columns.indexOf(col)
      if (j < 0)
        continue
      if (sameValue(value, row[j])) {
        delete e[col]
        continue
      }
      row[j] = value
      applied++
    }
    if (!Object.keys(e).length)
      delete next[key]
  }
  return { rows: out, edits: next, applied }
}

/** 记录里一共有多少处手改 */
export function editCount(edits: TableEdits | undefined): number {
  return Object.values(edits ?? {}).reduce((n, cols) => n + Object.keys(cols).length, 0)
}
