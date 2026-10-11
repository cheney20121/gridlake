/**
 * 选区变化监听 — 不绑定具体工作簿，工作簿重建后仍然有效
 */

import type { IRange } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'

export interface SelectionWatcher {
  dispose: () => void
}

/** 销毁工作簿（保留入口，便于统一处理销毁前的清理） */
export function disposeWorkbook(api: FUniver, unitId: string): boolean {
  return api.disposeUnit(unitId)
}

const SET_SELECTIONS = 'sheet.operation.set-selections'
/** SelectionMoveType：0 开始拖选、1 拖选中，只在拖选结束（2）或直接设置（3）时通知 */
const SETTLED = new Set([2, 3, undefined])

interface SetSelectionsParams {
  unitId?: string
  type?: number
  selections?: { range: IRange }[]
}

/**
 * 监听当前工作簿的选区变化（拖选结束或程序设置选区时）
 *
 * 不用 workbook.onSelectionChange：它订阅的是订阅那一刻的选区模型。打开工作簿、重建汇总表时会先销毁再创建
 * 同 id 的工作簿，订阅可能落在已作废的模型上，之后再也收不到通知（「查看明细」、透视表下钻因此失效）。
 * 也不用全局 Event.SelectionChanged：在 1.0.2 中先启动 Univer 再 createWorkbook 的流程下它不会触发。
 * 这里监听设置选区的操作本身，与工作簿的生命周期无关。
 */
export function watchSelection(api: FUniver, callback: (selections: IRange[]) => void): SelectionWatcher {
  const sub = api.addEvent(api.Event.CommandExecuted, (e) => {
    const { id, params } = e as unknown as { id: string, params?: SetSelectionsParams }
    if (id !== SET_SELECTIONS || !params || !SETTLED.has(params.type))
      return
    if (params.unitId && params.unitId !== api.getActiveWorkbook()?.getId())
      return
    callback((params.selections ?? []).map(s => s.range))
  })
  return { dispose: () => sub.dispose() }
}
