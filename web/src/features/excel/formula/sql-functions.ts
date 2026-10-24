import type { FUniver } from '@univerjs/core/facade'
import type { Cell, Query } from './sql-result'
import type { SqlResult } from '@/api/sql'
import { runSqlApi } from '@/api/sql'
import { t } from '@/i18n'
import { errorText } from '@/utils/busy-hint'
import { parseQuery, sqlTable, sqlValue } from './sql-result'

/**
 * 单元格里写 SQL：在服务端对数据集执行只读查询（与 SQL 工作台同一套检查与时限）
 *
 *   =SQL("SELECT 店铺, sum(金额) AS 金额 FROM 订单 GROUP BY 1 ORDER BY 2 DESC")     ← 结果带表头，铺到右下方的格子里
 *   =SQLVALUE("SELECT sum(金额) FROM 订单 WHERE 店铺 = {{店铺}}", "店铺", B2)        ← 只取第一行第一列，适合指标
 *
 * 查询后面可以跟「参数名, 值」对，代入 SQL 里的 {{参数名}}：数字按数字、其他按文本安全代入（不会拼接成 SQL）。
 * 日期单元格是序号，请写成 TEXT(B2, "yyyy-mm-dd")。结果最多 1000 行，超过时报错，请先 GROUP BY 汇总或 LIMIT。
 *
 * 与 DSUMIFS 一样按「两遍计算」：第一遍命中缓存直接返回、没命中的先返回占位并记下；本轮结束后并发请求，写进缓存再重算
 */

const PLACEHOLDER = t('计算中…')
/** 同时进行的查询数 */
const CONCURRENCY = 4

type Result = { result: SqlResult } | { error: string }

const cache = new Map<string, Result>()
const pending = new Map<string, Query>()
let flushTimer: ReturnType<typeof setTimeout> | null = null
let inFlight = 0
let api: FUniver | null = null

function lookup(query: Query): Result | null {
  const key = JSON.stringify([query.sql, query.params, query.values])
  const hit = cache.get(key)
  if (hit)
    return hit
  pending.set(key, query)
  if (flushTimer)
    clearTimeout(flushTimer)
  flushTimer = setTimeout(flush, 30)
  return null
}

async function flush() {
  flushTimer = null
  const jobs = [...pending.entries()]
  pending.clear()
  if (!jobs.length)
    return
  inFlight++
  try {
    let next = 0
    const worker = async () => {
      while (next < jobs.length) {
        const [key, q] = jobs[next++]!
        try {
          cache.set(key, { result: await runSqlApi(q.sql, undefined, q.params, q.values) })
        }
        catch (e) {
          // 失败也写进缓存，避免重算时反复请求；数据变了或重新加载时清空后会重试
          cache.set(key, { error: `#${errorText(e).replace(/^#/, '')}`.slice(0, 200) })
        }
      }
    }
    await Promise.all(Array.from({ length: Math.min(CONCURRENCY, jobs.length) }, worker))
  }
  finally {
    inFlight--
  }
  api?.getFormula().executeCalculation()
}

function evaluate(args: unknown[], shape: (r: SqlResult) => Cell[][] | Cell) {
  const q = parseQuery(args)
  if (typeof q === 'string')
    return q
  const hit = lookup(q)
  if (!hit)
    return PLACEHOLDER
  return 'error' in hit ? hit.error : shape(hit.result)
}

/** 数据变了（导入、删除、改名）：清空结果缓存（由调用方触发重算） */
export function clearSqlCache() {
  cache.clear()
}

export function sqlFormulasBusy() {
  return inFlight > 0 || pending.size > 0 || flushTimer != null
}

export function registerSqlFunctions(univerAPI: FUniver) {
  if (api === univerAPI)
    return
  api = univerAPI
  const formula = univerAPI.getFormula()
  formula.registerFunction('SQL', (...args) => evaluate(args, sqlTable), t('在服务端对数据集执行只读 SQL，结果（带表头）铺到右下方：SQL("SELECT 店铺, sum(金额) FROM 订单 GROUP BY 1", 参数名, 值, …)，SQL 里用 {0} 引用参数', ['{{参数名}}']))
  formula.registerFunction('SQLVALUE', (...args) => evaluate(args, sqlValue), t('执行只读 SQL，取第一行第一列：SQLVALUE("SELECT sum(金额) FROM 订单")；参数的写法与 SQL 函数相同'))
}
