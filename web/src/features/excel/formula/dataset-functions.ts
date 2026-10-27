import type { FUniver } from '@univerjs/core/facade'
import type { DatasetQuery } from '@/api/dataset'
import type { DataTableSpec } from '@/features/excel/datatable/types'
import { LEDGER_BASE } from '@/api/ledger-base'
import { chartBus } from '@/features/excel/chart/chart-manager'
import { datasetById } from '@/features/excel/dashboard/slicer'
import { findDataTable, hasDataTables, tableQuery } from '@/features/excel/datatable/types'
import { t } from '@/i18n'
import request from '@/utils/request'
import { clearSqlCache, registerSqlFunctions, sqlFormulasBusy } from './sql-functions'

/**
 * 公式下推：DSUMIFS / DCOUNTIFS / DAVERAGEIFS / DMAXIFS / DMINIFS / DCOUNTDISTINCTIFS 在服务端对大数据表（千万行）计算
 *
 *   =DSUMIFS("资金流水", "动账金额", "店铺", B2, "动账场景", "*退款*")
 *   =DCOUNTIFS("资金流水", "动账方向", "入账")
 *   =DAVERAGEIFS("资金流水", "佣金", "佣金", "<0")
 *   =DLOOKUP("商品成本", "成本价", "商品ID", A2)          ← 数据集版 VLOOKUP：返回第一条匹配行的值，找不到为 #N/A
 *
 * 参数与 Excel 的 SUMIFS 一致，只是「区域」换成「数据集名 + 列名」；条件支持 > < = <> 与通配符 * ? ~。
 *
 * 工作表里的数据表按表名引用，按数据表当前显示的行（它的筛选、搜索、切片器）计算：
 *
 *   =TSUM("订单表[订单金额]")                    ← 求和；后面可再跟条件对：TSUM("订单表[订单金额]", "店铺", B2)
 *   =TCOUNT("订单表")  =TAVERAGE(...)  =TMIN(...)  =TMAX(...)
 *   =TVALUE("订单表[订单号]", 1)                  ← 数据表当前排序下第 1 行的订单号
 *
 * Univer 的表格引用（表[列]）只能指向单元格区域，在客户端逐格计算，做不到下推，所以引用写成字符串。
 *
 * 为什么注册为同步函数：Univer 计算公式时逐个 await 异步函数，上千个单元格会变成上千次串行请求。
 * 这里改为「两遍计算」：第一遍命中缓存直接返回、未命中的先记下并返回占位；本轮结束后把所有未命中
 * 合并成一次批量请求（后端按条件签名一次扫表），写入缓存后再触发一次全量重算，第二遍全部命中
 */

type Func = 'sum' | 'count' | 'average' | 'min' | 'max' | 'lookup' | 'nth' | 'distinct'

interface Call {
  func: Func
  column?: string
  criteria: { column: string, value: string | number }[]
  /** nth：第几行（从 1 开始） */
  row?: number
}

/** 一批请求：同一数据集、同一范围（数据表的筛选） */
interface Batch {
  dataset: string
  query?: DatasetQuery
  calls: Map<string, Call>
}

interface Result {
  value?: number | string
  error?: string
}

const PLACEHOLDER = t('计算中…')
const BATCH_LIMIT = 5000

const cache = new Map<string, Result>()
/** 批次签名（数据集 + 范围）→ 批次 */
const pending = new Map<string, Batch>()
/** 工作簿里用到了数据表函数：数据表的筛选 / 切片器变化时需要重算 */
let tableFunctionsUsed = false
let flushTimer: ReturnType<typeof setTimeout> | null = null
let api: FUniver | null = null
let inFlight = 0

/** 单元格引用传进来的可能是 1×1 数组，取第一个值 */
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

function evaluate(func: Func, args: unknown[]): string | number {
  const withColumn = func !== 'count'
  const minArgs = withColumn ? 2 : 1
  if (args.length < minArgs || (args.length - minArgs) % 2 !== 0)
    return '#参数个数不正确'
  const dataset = scalar(args[0])
  if (dataset == null || dataset === '')
    return '#缺少数据集'
  const column = withColumn ? String(scalar(args[1]) ?? '') : undefined
  const criteria: Call['criteria'] = []
  for (let i = minArgs; i < args.length; i += 2) {
    const col = scalar(args[i])
    const val = scalar(args[i + 1])
    if (col == null || col === '')
      return '#条件列不能为空'
    criteria.push({ column: String(col), value: typeof val === 'number' ? val : val == null ? '' : String(val) })
  }
  return lookup(String(dataset), undefined, { func, column, criteria })
}

/** 查缓存；未命中记入批次并返回占位 */
function lookup(dataset: string, query: DatasetQuery | undefined, call: Call): string | number {
  const scope = JSON.stringify([dataset, query ?? null])
  const key = JSON.stringify([scope, call])
  const hit = cache.get(key)
  if (hit)
    return hit.error ?? hit.value ?? 0
  let batch = pending.get(scope)
  if (!batch) {
    batch = { dataset, query, calls: new Map() }
    pending.set(scope, batch)
  }
  batch.calls.set(key, call)
  scheduleFlush()
  return PLACEHOLDER
}

/** 解析 "表名[列名]" / "表名" */
function parseRef(v: unknown): { table: string, column?: string } | null {
  const s = scalar(v)
  if (s == null || s === '')
    return null
  const text = String(s).trim()
  const m = /^([^[\]]+)\[([^[\]]+)\]$/.exec(text)
  return m ? { table: m[1]!.trim(), column: m[2]!.trim() } : text.includes('[') ? null : { table: text }
}

/** 数据表函数：TSUM / TCOUNT / TAVERAGE / TMIN / TMAX / TVALUE */
function evaluateTable(func: Func, args: unknown[]): string | number {
  tableFunctionsUsed = true
  if (!api)
    return '#服务不可用'
  const ref = parseRef(args[0])
  if (!ref)
    return '#引用格式应为 "表名[列名]"'
  const needColumn = func !== 'count'
  if (needColumn && !ref.column)
    return `#请写明列，如 "${ref.table}[列名]"`
  const loc = findDataTable(api, ref.table)
  if (!loc)
    return `#找不到数据表「${ref.table}」`
  const spec: DataTableSpec = loc.spec
  const ds = datasetById(spec.datasetId)
  if (!ds)
    return `#数据表「${spec.name}」的数据集已删除`
  // 只是没显示的列也可以引用（隐藏列不应让公式出错）
  const colOf = (name: string) => ds.columns.find(c => c.title === name || c.key === name)
  if (ref.column && !colOf(ref.column))
    return `#数据表「${spec.name}」没有列「${ref.column}」`
  const query = tableQuery(api, spec, loc.sheet.getSheetId())
  if (func === 'nth') {
    const n = Number(scalar(args[1]))
    if (args.length !== 2 || !Number.isInteger(n) || n < 1)
      return '#行号应为大于 0 的整数'
    return lookup(spec.datasetId, query, { func, column: ref.column, criteria: [], row: n })
  }
  if ((args.length - 1) % 2 !== 0)
    return '#参数个数不正确'
  const criteria: Call['criteria'] = []
  for (let i = 1; i < args.length; i += 2) {
    const col = scalar(args[i])
    const val = scalar(args[i + 1])
    if (col == null || col === '')
      return '#条件列不能为空'
    if (!colOf(String(col)))
      return `#数据表「${spec.name}」没有列「${col}」`
    criteria.push({ column: String(col), value: typeof val === 'number' ? val : val == null ? '' : String(val) })
  }
  return lookup(spec.datasetId, query, { func, column: ref.column, criteria })
}

function scheduleFlush() {
  if (flushTimer)
    clearTimeout(flushTimer)
  // 等本轮计算把所有单元格都过一遍再发请求
  flushTimer = setTimeout(flush, 30)
}

/** 服务端计算的统计（诊断面板用）：请求次数、计算个数、累计 / 最近一次耗时 */
const stats = { requests: 0, calls: 0, totalMs: 0, lastMs: 0, lastAt: '' }
export function datasetFormulaStats() {
  return { ...stats }
}

async function flush() {
  flushTimer = null
  const batches = [...pending.values()]
  pending.clear()
  if (!batches.length)
    return
  const started = performance.now()
  inFlight++
  try {
    for (const { dataset, query, calls } of batches) {
      const entries = [...calls.entries()]
      for (let i = 0; i < entries.length; i += BATCH_LIMIT) {
        const chunk = entries.slice(i, i + BATCH_LIMIT)
        try {
          const res = await request<{ values: (number | string | null)[], errors: (string | null)[] }>({
            url: `${LEDGER_BASE}/api/ledger/datasets/${encodeURIComponent(dataset)}/aggregate`,
            method: 'post',
            data: { calls: chunk.map(([, c]) => c), query },
            timeout: 5 * 60 * 1000,
          })
          stats.requests++
          stats.calls += chunk.length
          chunk.forEach(([key], j) => {
            const err = res.errors[j]
            cache.set(key, err ? { error: err.startsWith('#') ? err : `#${err}` } : { value: res.values[j] ?? '' })
          })
        }
        catch (e) {
          console.warn('[dataset-formula] 计算失败:', e)
          // 失败也写入缓存，避免重算时反复请求；数据变更或重新加载时清空缓存后会重试
          chunk.forEach(([key]) => cache.set(key, { error: '#服务不可用' }))
        }
      }
    }
  }
  finally {
    inFlight--
    stats.lastMs = Math.round(performance.now() - started)
    stats.totalMs += stats.lastMs
    stats.lastAt = new Date().toLocaleTimeString()
  }
  api?.getFormula().executeCalculation()
}

/** 数据变更（导入、删除、改名）或重新加载汇总时调用：清空结果缓存并重算 */
export function invalidateDatasetFormulas(recalculate = true) {
  cache.clear()
  clearSqlCache()
  if (recalculate)
    api?.getFormula().executeCalculation()
}

/** 是否还有未完成的下推计算（加载汇总时等待用） */
export function datasetFormulasBusy() {
  return inFlight > 0 || pending.size > 0 || flushTimer != null || sqlFormulasBusy()
}

const DESCRIPTIONS: Record<string, string> = {
  DSUMIFS: t('对数据集（服务端大数据表）按多个条件求和：DSUMIFS(数据集, 求和列, 条件列1, 条件1, ...)'),
  DCOUNTIFS: t('对数据集按多个条件计数：DCOUNTIFS(数据集, 条件列1, 条件1, ...)'),
  DAVERAGEIFS: t('对数据集按多个条件求平均：DAVERAGEIFS(数据集, 平均列, 条件列1, 条件1, ...)'),
  DMAXIFS: t('对数据集按多个条件取最大值：DMAXIFS(数据集, 取值列, 条件列1, 条件1, ...)，没有符合条件的数值时为 0（与 Excel 的 MAXIFS 一致）'),
  DMINIFS: t('对数据集按多个条件取最小值：DMINIFS(数据集, 取值列, 条件列1, 条件1, ...)，没有符合条件的数值时为 0（与 Excel 的 MINIFS 一致）'),
  DCOUNTDISTINCTIFS: t('对数据集按多个条件统计不重复的取值个数：DCOUNTDISTINCTIFS(数据集, 去重列, 条件列1, 条件1, ...)，空值不算一个取值，区分大小写'),
  DLOOKUP: t('在数据集中查找：DLOOKUP(数据集, 返回列, 条件列1, 条件1, ...)，返回第一条匹配行的值，找不到为 #N/A'),
  TSUM: t('对工作表里的数据表求和（按数据表当前的筛选）：TSUM("表名[列名]", 条件列1, 条件1, ...)'),
  TCOUNT: t('数据表的行数（按数据表当前的筛选）：TCOUNT("表名", 条件列1, 条件1, ...)'),
  TAVERAGE: t('数据表某列的平均值：TAVERAGE("表名[列名]", 条件列1, 条件1, ...)'),
  TMIN: t('数据表某列的最小值：TMIN("表名[列名]", 条件列1, 条件1, ...)'),
  TMAX: t('数据表某列的最大值：TMAX("表名[列名]", 条件列1, 条件1, ...)'),
  TVALUE: t('数据表当前排序下第 N 行某列的值：TVALUE("表名[列名]", N)'),
}

let recalcTimer: ReturnType<typeof setTimeout> | null = null
/** 数据表的筛选 / 切片器变了：重算（缓存键含范围，变了的自然重新请求） */
function recalcTables() {
  // 打开工作簿时单元格用的是保存的结果、公式还没算过，标记不可靠：工作簿里有数据表就重算
  if (!api || (!tableFunctionsUsed && !hasDataTables(api)))
    return
  if (recalcTimer)
    clearTimeout(recalcTimer)
  recalcTimer = setTimeout(() => {
    recalcTimer = null
    api?.getFormula().executeCalculation()
  }, 150)
}

/** 注册数据集公式函数（工作簿创建前后调用均可，一个 Univer 实例只需一次） */
export function registerDatasetFunctions(univerAPI: FUniver) {
  if (api === univerAPI)
    return
  api = univerAPI
  const formula = univerAPI.getFormula()
  formula.registerFunction('DSUMIFS', (...args) => evaluate('sum', args), DESCRIPTIONS.DSUMIFS)
  formula.registerFunction('DCOUNTIFS', (...args) => evaluate('count', args), DESCRIPTIONS.DCOUNTIFS)
  formula.registerFunction('DAVERAGEIFS', (...args) => evaluate('average', args), DESCRIPTIONS.DAVERAGEIFS)
  formula.registerFunction('DMAXIFS', (...args) => evaluate('max', args), DESCRIPTIONS.DMAXIFS)
  formula.registerFunction('DMINIFS', (...args) => evaluate('min', args), DESCRIPTIONS.DMINIFS)
  formula.registerFunction('DCOUNTDISTINCTIFS', (...args) => evaluate('distinct', args), DESCRIPTIONS.DCOUNTDISTINCTIFS)
  formula.registerFunction('DLOOKUP', (...args) => evaluate('lookup', args), DESCRIPTIONS.DLOOKUP)
  formula.registerFunction('TSUM', (...args) => evaluateTable('sum', args), DESCRIPTIONS.TSUM)
  formula.registerFunction('TCOUNT', (...args) => evaluateTable('count', args), DESCRIPTIONS.TCOUNT)
  formula.registerFunction('TAVERAGE', (...args) => evaluateTable('average', args), DESCRIPTIONS.TAVERAGE)
  formula.registerFunction('TMIN', (...args) => evaluateTable('min', args), DESCRIPTIONS.TMIN)
  formula.registerFunction('TMAX', (...args) => evaluateTable('max', args), DESCRIPTIONS.TMAX)
  formula.registerFunction('TVALUE', (...args) => evaluateTable('nth', args), DESCRIPTIONS.TVALUE)
  registerSqlFunctions(univerAPI)
  chartBus.on('spec', (s: { kind?: string }) => {
    if (s?.kind === 'datatable')
      recalcTables()
  })
  // 单元格模式数据表刷新、改定义后
  chartBus.on('cell-table', recalcTables)
  chartBus.on('slicer', recalcTables)
}

/** 数据表增删 / 改名后重算引用它的公式 */
export function recalculateTableFormulas() {
  recalcTables()
}
