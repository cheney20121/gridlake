import type { FUniver } from '@univerjs/core/facade'
import type { Peer } from './session'
import { reactElement as createElement } from '@/features/excel/data/drill-hint'

/**
 * 别人的光标：在他选中的格子旁贴一个带他颜色与名字的小标签（挂在 Univer 的单元格弹层上，随滚动跟着格子走）。
 * 只显示当前工作表上的；每个人一个标签，位置变了就重新挂
 */

const registered = new Set<string>()
const popups = new Map<string, { dispose: () => void }>()
const shown = new Map<string, string>()

const keyOf = (id: string) => `collab.cursor.${id}`

function ensureComponent(api: FUniver, peer: Peer) {
  const key = keyOf(peer.id)
  if (registered.has(key))
    return key
  registered.add(key)
  // 名字与颜色在注册时固定（同一个连接里不会变）
  api.registerComponent(key, () => createElement('div', {
    style: {
      pointerEvents: 'none',
      padding: '0 5px',
      height: '16px',
      lineHeight: '16px',
      borderRadius: '3px',
      background: peer.color,
      color: '#fff',
      fontSize: '11px',
      whiteSpace: 'nowrap',
    },
  }, peer.name))
  return key
}

/** 按当前的在线者与活动工作表刷新所有光标标签 */
export function renderCursorTags(api: FUniver, peers: Peer[]) {
  const sheet = api.getActiveWorkbook()?.getActiveSheet()
  const sheetId = sheet?.getSheetId()
  const alive = new Set<string>()
  for (const p of peers) {
    const c = p.cursor
    if (!sheet || !c || c.sheetId !== sheetId)
      continue
    alive.add(p.id)
    const at = `${sheetId}|${c.row}|${c.col}`
    if (shown.get(p.id) === at)
      continue
    popups.get(p.id)?.dispose()
    const range = sheet.getRange(c.row, c.col) as unknown as { attachPopup?: (o: { componentKey: string, direction?: string, offset?: [number, number] }) => { dispose: () => void } | null }
    const popup = range.attachPopup?.({ componentKey: ensureComponent(api, p), direction: 'top-right' as never, offset: [0, -2] })
    if (popup) {
      popups.set(p.id, popup)
      shown.set(p.id, at)
    }
  }
  for (const id of [...popups.keys()]) {
    if (!alive.has(id)) {
      popups.get(id)?.dispose()
      popups.delete(id)
      shown.delete(id)
    }
  }
}

export function clearCursorTags() {
  for (const p of popups.values())
    p.dispose()
  popups.clear()
  shown.clear()
}
