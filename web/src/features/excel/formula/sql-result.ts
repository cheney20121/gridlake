import type { ParamDef, SqlResult } from '@/api/sql'
import { t } from '@/i18n'

/** 单元格里的 SQL：参数解析与结果整形（不依赖网络，便于单元测试；调用见 sql-functions.ts） */

/** 服务端单次查询的行数上限（与 SQL 工作台一致） */
const MAX_ROWS = 1000

export interface Query {
  sql: string
  params: ParamDef[]
  values: Record<string, string>
}

function scalar(v: unknown): string | number | boolean | null {
  let x = v
  while (Array.isArray(x))
    x = x[0]
  if (x == null)
    return null
  if (typeof x === 'number' || typeof x === 'string' || typeof x === 'boolean')
    return x
  return String(x)
}

/** 参数：查询后面的「名称, 值」对；返回错误说明或查询 */
export function parseQuery(args: unknown[]): Query | string {
  const sql = scalar(args[0])
  if (sql == null || String(sql).trim() === '')
    return '#缺少查询'
  if ((args.length - 1) % 2 !== 0)
    return '#参数应为「名称, 值」成对出现'
  const params: ParamDef[] = []
  const values: Record<string, string> = {}
  for (let i = 1; i < args.length; i += 2) {
    const name = scalar(args[i])
    if (name == null || String(name).trim() === '')
      return '#参数名不能为空'
    const v = scalar(args[i + 1])
    params.push({ name: String(name).trim(), kind: typeof v === 'number' ? 'number' : 'text' })
    values[String(name).trim()] = v == null ? '' : typeof v === 'boolean' ? (v ? 'TRUE' : 'FALSE') : String(v)
  }
  return { sql: String(sql), params, values }
}

export type Cell = string | number | boolean
/** 结果单元格：空值写成空串（铺开的区域里显示为空白） */
const cellOf = (v: unknown): Cell => (v == null ? '' : typeof v === 'number' || typeof v === 'boolean' ? v : String(v))

/** =SQL(...)：表头 + 数据行 */
export function sqlTable(r: SqlResult): Cell[][] | string {
  if (r.truncated)
    return t('#结果超过 {0} 行：请先 GROUP BY 汇总或加 LIMIT', [MAX_ROWS])
  if (!r.columns.length)
    return '#查询没有返回列'
  return [r.columns.map(c => c.name), ...r.rows.map(row => row.map(cellOf))]
}

/** =SQLVALUE(...)：第一行第一列；没有行时为 #N/A */
export function sqlValue(r: SqlResult): Cell {
  const row = r.rows[0]
  return row ? cellOf(row[0]) : '#N/A'
}
