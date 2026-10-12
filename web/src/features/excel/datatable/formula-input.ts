import type { FUniver } from '@univerjs/core/facade'
import { renameTableInFormula, toPlatformFormula } from './formula-syntax'
import { findDataTable, hasDataTables } from './types'

/**
 * 数据表公式的输入与维护
 *
 * - 用户按 Excel 习惯输入 `=SUM(订单表[金额])`（输入、粘贴都算）：提交后自动改写为 `=TSUM("订单表[金额]")`
 * - 数据表改名：把引用旧表名的数据表函数一并改成新表名
 */

type Matrix = Record<string, Record<string, { f?: string | null } | null | undefined> | undefined>

let registered: FUniver | null = null

/** 监听单元格写入；onRewrite(改写个数, 示例) 用于页面提示 */
export function registerTableFormulaInput(api: FUniver, onRewrite: (count: number, example: { from: string, to: string }) => void) {
  if (registered === api)
    return
  registered = api
  api.addEvent(api.Event.CommandExecuted, (event) => {
    const cmd = event as unknown as { id: string, params?: { subUnitId?: string, cellValue?: Matrix } }
    if (cmd.id !== 'sheet.mutation.set-range-values' || !cmd.params?.cellValue || !cmd.params.subUnitId)
      return
    // 没有数据表时不必检查
    if (!hasDataTables(api))
      return
    const isTable = (name: string) => findDataTable(api, name)?.spec.name ?? null
    const changes: { row: number, col: number, from: string, to: string }[] = []
    for (const [r, cols] of Object.entries(cmd.params.cellValue)) {
      for (const [c, cell] of Object.entries(cols ?? {})) {
        const f = cell?.f
        if (typeof f !== 'string' || !f.includes('['))
          continue
        const to = toPlatformFormula(f, isTable)
        if (to)
          changes.push({ row: Number(r), col: Number(c), from: f, to })
      }
    }
    if (!changes.length)
      return
    const sheetId = cmd.params.subUnitId
    // 在当前命令执行完之后再改写（命令执行期间不能再发起写入）
    setTimeout(() => {
      const sheet = api.getActiveWorkbook()?.getSheetBySheetId(sheetId)
      if (!sheet)
        return
      for (const ch of changes) {
        const range = sheet.getRange(ch.row, ch.col)
        // 同一次写入会收到不止一次通知：已改写过（或已被改成别的）就跳过
        if (range.getFormula() !== ch.from)
          continue
        range.setValue(ch.to)
        notify(ch, onRewrite)
      }
    }, 0)
  })
}

/** 连续改写（粘贴一片、逐格输入）合并成一次提示 */
let pendingCount = 0
let pendingExample: { from: string, to: string } | null = null
let notifyTimer: ReturnType<typeof setTimeout> | null = null
function notify(ch: { from: string, to: string }, onRewrite: (count: number, example: { from: string, to: string }) => void) {
  pendingCount++
  pendingExample ??= ch
  if (notifyTimer)
    clearTimeout(notifyTimer)
  notifyTimer = setTimeout(() => {
    onRewrite(pendingCount, pendingExample!)
    pendingCount = 0
    pendingExample = null
    notifyTimer = null
  }, 300)
}

/** 数据表改名后同步公式，返回改动的单元格数 */
export function renameTableFormulas(api: FUniver, from: string, to: string): number {
  const wb = api.getActiveWorkbook()
  if (!wb || !from.trim() || from.trim().toLowerCase() === to.trim().toLowerCase())
    return 0
  let n = 0
  for (const sheet of wb.getSheets()) {
    const lastRow = sheet.getLastRow()
    const lastCol = sheet.getLastColumn()
    if (lastRow < 0 || lastCol < 0)
      continue
    const formulas = sheet.getRange(0, 0, lastRow + 1, lastCol + 1).getFormulas()
    formulas.forEach((row, r) => row.forEach((f, c) => {
      const next = f ? renameTableInFormula(f, from, to) : null
      if (next) {
        sheet.getRange(r, c).setValue(next)
        n++
      }
    }))
  }
  return n
}
