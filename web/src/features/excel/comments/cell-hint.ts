import type { FUniver } from '@univerjs/core/facade'
import { createCellButton } from '@/features/excel/data/drill-hint'
import { t } from '@/i18n'
import { threadsAt } from './state'

/**
 * 选中有评论的格子时，格子旁出现「💬 N」，点一下打开评论面板。
 * （没有给每个有评论的格子画角标：Univer 的单元格没有稳定的自定义标记，评论面板的「本页其他评论」可以看到全部）
 */
export function createCommentHint(api: FUniver, onOpen: (row: number, col: number) => void) {
  let at = { row: 0, col: 0 }
  const button = createCellButton(api, 'app.comment-hint', () => onOpen(at.row, at.col), { direction: 'vertical', offset: [0, 2] })
  return {
    /** 选区变化时调用；panelOpen 为真时面板自己就在显示，不再弹提示 */
    update(sel: { startRow: number, startColumn: number } | undefined, panelOpen: boolean) {
      const sheet = api.getActiveWorkbook()?.getActiveSheet()
      const here = sel && sheet ? threadsAt(sheet.getSheetId(), sel.startRow, sel.startColumn) : []
      if (!sel || !here.length || panelOpen) {
        button.hide()
        return
      }
      at = { row: sel.startRow, col: sel.startColumn }
      const open = here.filter(t => !t.resolved).length
      button.show(sel.startRow, sel.startColumn, `💬 ${here.reduce((n, t) => n + 1 + t.replies.length, 0)}`, open ? t('{0} 条未解决的评论线程，点击查看', [open]) : t('评论已全部解决，点击查看'))
    },
    hide: button.hide,
  }
}
