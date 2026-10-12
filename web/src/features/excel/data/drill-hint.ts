/**
 * 贴在格子旁边的小按钮（挂在 Univer 的单元格弹层上，随滚动、缩放跟着格子走）：
 * - 「查看明细 ↗」：选中汇总表 / 透视表里能下钻的格子时出现，点一下打开明细；顶栏的「查看明细」按钮仍保留
 * - 单元格模式数据表：选中表里的格子时，在表头右侧显示表名与行数，点一下编辑
 */

import type { FUniver } from '@univerjs/core/facade'
import { createElement } from 'react'

/** 供其他浮层小部件（如协作光标标签）创建 React 元素用 */
export const reactElement = createElement

const STYLE = {
  display: 'flex',
  alignItems: 'center',
  gap: '2px',
  height: '20px',
  padding: '0 6px',
  border: '1px solid #107C41',
  borderRadius: '10px',
  background: '#fff',
  color: '#0E5A31',
  fontSize: '12px',
  lineHeight: '18px',
  whiteSpace: 'nowrap',
  cursor: 'pointer',
  boxShadow: '0 1px 4px rgba(0, 0, 0, 0.12)',
} as const

export interface CellButton {
  show: (row: number, col: number, label: string, title: string) => void
  hide: () => void
}

/** placement：弹层相对格子的方向与偏移（默认在格子右侧）；同一格子上有多个按钮时错开 */
export function createCellButton(api: FUniver, key: string, onClick: () => void, placement: { direction?: string, offset?: [number, number] } = {}): CellButton {
  let label = ''
  let title = ''
  api.registerComponent(key, () => createElement('button', {
    'type': 'button',
    'style': STYLE,
    title,
    'data-cell-button': key,
    // 不让按下事件落到画布上（否则会被当成重新选区）
    'onPointerDown': (e: { stopPropagation: () => void }) => e.stopPropagation(),
    onClick,
  }, label))

  let popup: { dispose: () => void } | null = null
  const hide = () => {
    popup?.dispose()
    popup = null
  }
  return {
    show(row, col, l, t) {
      hide()
      label = l
      title = t
      const range = api.getActiveWorkbook()?.getActiveSheet()?.getRange(row, col) as unknown as {
        attachPopup?: (p: { componentKey: string, direction?: string, offset?: [number, number] }) => { dispose: () => void } | null
      } | undefined
      popup = range?.attachPopup?.({ componentKey: key, direction: placement.direction ?? 'horizontal', offset: placement.offset ?? [4, 0] }) ?? null
    },
    hide,
  }
}

export interface DrillHint {
  show: (row: number, col: number, title: string) => void
  hide: () => void
}

export function createDrillHint(api: FUniver, onOpen: () => void): DrillHint {
  const button = createCellButton(api, 'app.drill-hint', onOpen)
  return {
    show: (row, col, title) => button.show(row, col, '查看明细 ↗', `查看「${title}」的明细`),
    hide: button.hide,
  }
}
