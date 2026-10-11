/**
 * 协作同步里的变更（Univer mutation）处理：哪些要转发、同一个格子的先后冲突怎么处理、换成本地工作簿的 unitId（纯函数）
 */

export interface Op {
  id: string
  params: Record<string, unknown>
}

/** 不转发的变更：权限点、筛选的脏标记、空操作（这些由各端自己的状态得出） */
const SKIP = new Set([
  'sheet.mutation.set-worksheet-permission-points',
  'sheet.mutation.mark-dirty-filter-change',
  'sheet.mutation.empty',
])

/** 一个本地执行的命令事件是否要转发给别人：只转发工作表的变更；公式计算结果、只在本地生效的、从协作收到的不转发 */
export function shouldRelay(id: string, options: { onlyLocal?: boolean, fromFormula?: boolean, fromCollab?: boolean } | undefined): boolean {
  if (!id.startsWith('sheet.mutation.') || SKIP.has(id))
    return false
  return !(options?.onlyLocal || options?.fromFormula || options?.fromCollab)
}

type CellMatrix = Record<string, Record<string, unknown>>

/** 一批变更动到的格子（`工作表|行|列`），用于冲突判断；只认设置单元格的变更 */
export function cellKeys(op: Op): string[] {
  if (op.id !== 'sheet.mutation.set-range-values')
    return []
  const sheet = String(op.params.subUnitId ?? '')
  const matrix = op.params.cellValue as CellMatrix | undefined
  if (!matrix)
    return []
  const out: string[] = []
  for (const [r, cols] of Object.entries(matrix)) {
    for (const c of Object.keys(cols ?? {}))
      out.push(`${sheet}|${r}|${c}`)
  }
  return out
}

/**
 * 别人的设置单元格变更到来时，去掉我这边「已经发出、还没收到回显」的格子：
 * 那些格子在服务端的顺序里我的修改更晚，最终应该是我的值。没有可去的返回原变更；全部去掉返回 null
 */
export function withoutPending(op: Op, pending: ReadonlySet<string>): Op | null {
  if (!pending.size || op.id !== 'sheet.mutation.set-range-values')
    return op
  const sheet = String(op.params.subUnitId ?? '')
  const matrix = op.params.cellValue as CellMatrix | undefined
  if (!matrix)
    return op
  let removed = 0
  let total = 0
  const next: CellMatrix = {}
  for (const [r, cols] of Object.entries(matrix)) {
    for (const [c, cell] of Object.entries(cols ?? {})) {
      total++
      if (pending.has(`${sheet}|${r}|${c}`)) {
        removed++
        continue
      }
      ;(next[r] ??= {})[c] = cell
    }
  }
  if (!removed)
    return op
  return removed === total ? null : { ...op, params: { ...op.params, cellValue: next } }
}

/** 别人的变更里的 unitId 换成本地工作簿的（各端打开同一个工作簿时 id 通常一样，但不依赖这一点） */
export function retarget(op: Op, unitId: string): Op {
  return 'unitId' in op.params ? { ...op, params: { ...op.params, unitId } } : op
}
