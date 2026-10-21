import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import { t } from '@/i18n'

/**
 * 行列分组（大纲）：Univer 开源版没有，这里按 Excel 的模型实现
 *
 * - 每行 / 每列有一个大纲级别（0 ~ 7）。组合 = 选中范围级别 +1，取消组合 = -1；
 *   同一级别连续的行构成一个分组（与 xlsx 的 outlineLevel 一致，导入导出无损）
 * - 折叠 = 隐藏分组内的行；分组的 −/+ 按钮在分组下方一行（Excel 默认「汇总行在下方」）
 * - 折叠状态按「级别@分组末行」记录：展开外层分组时，内层已折叠的分组保持折叠
 * - 插入 / 删除行列时级别随之移动
 * 定义保存在工作表元数据 `outline` 里
 */

export type Dim = 'rows' | 'cols'

export interface SheetOutline {
  rows: Record<string, number>
  cols: Record<string, number>
  collapsed: { rows: string[], cols: string[] }
}

export interface OutlineGroup {
  level: number
  start: number
  end: number
  collapsed: boolean
}

const META_KEY = 'outline'
export const MAX_LEVEL = 7

export function emptyOutline(): SheetOutline {
  return { rows: {}, cols: {}, collapsed: { rows: [], cols: [] } }
}

export function getOutline(sheet: FWorksheet | null | undefined): SheetOutline {
  const custom = sheet?.getCustomMetadata() as Record<string, unknown> | undefined
  const o = custom?.[META_KEY] as SheetOutline | undefined
  return o ? { rows: { ...o.rows }, cols: { ...o.cols }, collapsed: { rows: [...(o.collapsed?.rows ?? [])], cols: [...(o.collapsed?.cols ?? [])] } } : emptyOutline()
}

export function saveOutline(sheet: FWorksheet, o: SheetOutline) {
  const empty = !Object.keys(o.rows).length && !Object.keys(o.cols).length
  const custom = { ...(sheet.getCustomMetadata() ?? {}) } as Record<string, unknown>
  if (empty)
    delete custom[META_KEY]
  else
    custom[META_KEY] = o
  sheet.setCustomMetadata(custom)
  notify()
}

export function hasOutline(o: SheetOutline) {
  return Object.keys(o.rows).length > 0 || Object.keys(o.cols).length > 0
}

const key = (level: number, end: number) => `${level}@${end}`

/** 由级别推出分组：每个级别 L 上，级别 ≥ L 的连续行为一组 */
export function groupsOf(o: SheetOutline, dim: Dim): OutlineGroup[] {
  const levels = o[dim]
  const idx = Object.keys(levels).map(Number).filter(i => (levels[i] ?? 0) > 0).sort((a, b) => a - b)
  if (!idx.length)
    return []
  const collapsed = new Set(o.collapsed[dim])
  const max = Math.max(...idx.map(i => levels[i]!))
  const out: OutlineGroup[] = []
  for (let level = 1; level <= max; level++) {
    let start = -1
    let prev = -2
    for (const i of idx) {
      const ok = levels[i]! >= level
      if (ok && start >= 0 && i === prev + 1) {
        prev = i
        continue
      }
      if (start >= 0)
        out.push({ level, start, end: prev, collapsed: collapsed.has(key(level, prev)) })
      start = ok ? i : -1
      prev = ok ? i : -2
    }
    if (start >= 0)
      out.push({ level, start, end: prev, collapsed: collapsed.has(key(level, prev)) })
  }
  return out
}

export function maxLevel(o: SheetOutline, dim: Dim) {
  return Object.values(o[dim]).reduce((m, l) => Math.max(m, l), 0)
}

/** 去掉已不存在的分组的折叠记录 */
function pruneCollapsed(o: SheetOutline, dim: Dim) {
  const alive = new Set(groupsOf({ ...o, collapsed: { rows: [], cols: [] } }, dim).map(g => key(g.level, g.end)))
  o.collapsed[dim] = o.collapsed[dim].filter(k => alive.has(k))
}

/** 按折叠状态设置一段范围内各行 / 列的显示（其他原因隐藏的行列不在分组范围内时不受影响） */
function applyVisibility(sheet: FWorksheet, o: SheetOutline, dim: Dim, from: number, to: number) {
  const hidden = new Set<number>()
  for (const g of groupsOf(o, dim)) {
    if (g.collapsed && g.end >= from && g.start <= to) {
      for (let i = Math.max(g.start, from); i <= Math.min(g.end, to); i++)
        hidden.add(i)
    }
  }
  // 连续段批量隐藏 / 显示
  let i = from
  while (i <= to) {
    const h = hidden.has(i)
    let j = i
    while (j + 1 <= to && hidden.has(j + 1) === h)
      j++
    if (dim === 'rows')
      h ? sheet.hideRows(i, j - i + 1) : sheet.showRows(i, j - i + 1)
    else
      h ? sheet.hideColumns(i, j - i + 1) : sheet.showColumns(i, j - i + 1)
    i = j + 1
  }
}

/** 组合：返回错误说明，成功返回 null */
export function group(sheet: FWorksheet, dim: Dim, start: number, end: number): string | null {
  const o = getOutline(sheet)
  for (let i = start; i <= end; i++) {
    if ((o[dim][i] ?? 0) >= MAX_LEVEL)
      return t('最多 {0} 级分组', [MAX_LEVEL])
  }
  for (let i = start; i <= end; i++)
    o[dim][i] = (o[dim][i] ?? 0) + 1
  pruneCollapsed(o, dim)
  saveOutline(sheet, o)
  return null
}

/** 取消组合：范围内级别 -1；原本被折叠隐藏、现在不再属于折叠分组的行列重新显示 */
export function ungroup(sheet: FWorksheet, dim: Dim, start: number, end: number): string | null {
  const o = getOutline(sheet)
  let changed = false
  for (let i = start; i <= end; i++) {
    const l = o[dim][i] ?? 0
    if (l <= 0)
      continue
    changed = true
    if (l === 1)
      delete o[dim][i]
    else
      o[dim][i] = l - 1
  }
  if (!changed)
    return dim === 'rows' ? t('选中的行没有分组') : t('选中的列没有分组')
  // 记录在被取消的范围内结束的分组：级别变化后键会失效
  pruneCollapsed(o, dim)
  saveOutline(sheet, o)
  applyVisibility(sheet, o, dim, start, end)
  return null
}

/** 折叠 / 展开一个分组 */
export function toggleGroup(sheet: FWorksheet, dim: Dim, g: OutlineGroup, collapse = !g.collapsed) {
  const o = getOutline(sheet)
  const k = key(g.level, g.end)
  o.collapsed[dim] = o.collapsed[dim].filter(x => x !== k)
  if (collapse)
    o.collapsed[dim].push(k)
  saveOutline(sheet, o)
  applyVisibility(sheet, o, dim, g.start, g.end)
}

/** 显示到第 n 级（Excel 左上角的 1 2 3 按钮）：级别 ≥ n 的分组折叠，其余展开 */
export function showLevel(sheet: FWorksheet, dim: Dim, n: number) {
  const o = getOutline(sheet)
  const groups = groupsOf(o, dim)
  if (!groups.length)
    return
  o.collapsed[dim] = groups.filter(g => g.level >= n).map(g => key(g.level, g.end))
  saveOutline(sheet, o)
  applyVisibility(sheet, o, dim, Math.min(...groups.map(g => g.start)), Math.max(...groups.map(g => g.end)))
}

/** 最近一次删除的行列的级别：紧接着在同一位置插回同样多的行列（撤销删除）时恢复 */
const removed = new Map<string, { at: number, count: number, levels: Record<number, number>, collapsed: string[] }>()

/** 插入 / 删除行列时移动级别（新插入的行列：两侧同属一个分组时继承级别） */
export function shiftOutline(sheet: FWorksheet, dim: Dim, at: number, count: number) {
  const o = getOutline(sheet)
  const stashKey = `${sheet.getSheetId()}|${dim}`
  const stash = removed.get(stashKey)
  removed.delete(stashKey)
  if (count < 0) {
    const levels: Record<number, number> = {}
    for (let i = at; i < at - count; i++) {
      if (o[dim][i])
        levels[i - at] = o[dim][i]!
    }
    if (Object.keys(levels).length)
      removed.set(stashKey, { at, count: -count, levels, collapsed: [...o.collapsed[dim]] })
  }
  if (!Object.keys(o[dim]).length && !(count > 0 && stash))
    return
  const next: Record<string, number> = {}
  const shiftKey = (k: string) => {
    const [l, e] = k.split('@').map(Number) as [number, number]
    if (count < 0 && e >= at && e < at - count)
      return null
    return key(l, e >= at ? e + count : e)
  }
  const restore = count > 0 && stash && stash.at === at && stash.count === count ? stash : null
  if (count > 0) {
    const inherit = Math.min(o[dim][at - 1] ?? 0, o[dim][at] ?? 0)
    for (const [i, l] of Object.entries(o[dim]))
      next[Number(i) >= at ? Number(i) + count : Number(i)] = l
    for (let i = at; i < at + count; i++) {
      const l = restore ? restore.levels[i - at] ?? 0 : inherit
      if (l > 0)
        next[i] = l
    }
  }
  else {
    for (const [i, l] of Object.entries(o[dim])) {
      const n = Number(i)
      if (n >= at && n < at - count)
        continue
      next[n >= at - count ? n + count : n] = l
    }
  }
  o[dim] = next
  // 撤销删除：折叠记录恢复为删除前的
  o.collapsed[dim] = restore ? restore.collapsed : o.collapsed[dim].map(shiftKey).filter((k): k is string => !!k)
  pruneCollapsed(o, dim)
  saveOutline(sheet, o)
}

// ── 变化通知（大纲区重画；工作表元数据的修改不走命令，需另行通知工作簿保存） ──
const listeners = new Set<() => void>()
let onDirty: (() => void) | null = null
export function onOutlineChanged(fn: () => void) {
  listeners.add(fn)
  return () => listeners.delete(fn)
}
function notify() {
  listeners.forEach(fn => fn())
  onDirty?.()
}

/** 当前选区的行 / 列范围 */
export function selectionSpan(sheet: FWorksheet, dim: Dim): [number, number] | null {
  const r = sheet.getSelection()?.getActiveRange()
  if (!r)
    return null
  // getHeight / getWidth 是选区的行数 / 列数（Univer 的 FRange 没有 getNumRows / getNumColumns）
  return dim === 'rows' ? [r.getRow(), r.getRow() + r.getHeight() - 1] : [r.getColumn(), r.getColumn() + r.getWidth() - 1]
}

/** 选区是整列（列标被选中）时按列组合，否则按行 */
export function defaultDim(sheet: FWorksheet): Dim {
  const r = sheet.getSelection()?.getActiveRange()
  return r && r.getHeight() >= sheet.getMaxRows() ? 'cols' : 'rows'
}

let registered: FUniver | null = null

/**
 * 注册分组：插入 / 删除行列时同步移动级别；右键菜单「组合 / 取消组合」；
 * 快捷键 Shift+Alt+→ 组合、Shift+Alt+← 取消组合（与 Excel 相同）
 */
export function registerOutline(api: FUniver, options: { onDirty: () => void, onMessage: (msg: string) => void }) {
  onDirty = options.onDirty
  if (registered === api)
    return
  registered = api
  const run = (action: 'group' | 'ungroup', dim?: Dim) => {
    const sheet = api.getActiveWorkbook()?.getActiveSheet()
    if (!sheet)
      return
    const d = dim ?? defaultDim(sheet)
    const span = selectionSpan(sheet, d)
    if (!span)
      return
    const err = action === 'group' ? group(sheet, d, span[0], span[1]) : ungroup(sheet, d, span[0], span[1])
    if (err)
      options.onMessage(err)
  }
  const menus: [string, string, 'group' | 'ungroup', Dim | undefined, string[]][] = [
    ['gridlake.outline.group-rows', '组合行', 'group', 'rows', ['contextMenu.rowHeader', 'contextMenu.others']],
    ['gridlake.outline.ungroup-rows', '取消组合行', 'ungroup', 'rows', ['contextMenu.rowHeader', 'contextMenu.others']],
    ['gridlake.outline.group-cols', '组合列', 'group', 'cols', ['contextMenu.colHeader', 'contextMenu.others']],
    ['gridlake.outline.ungroup-cols', '取消组合列', 'ungroup', 'cols', ['contextMenu.colHeader', 'contextMenu.others']],
    ['gridlake.outline.group', '组合所选行（Shift+Alt+→）', 'group', 'rows', ['contextMenu.mainArea', 'contextMenu.others']],
    ['gridlake.outline.ungroup', '取消组合所选行（Shift+Alt+←）', 'ungroup', 'rows', ['contextMenu.mainArea', 'contextMenu.others']],
  ]
  const ui = api as unknown as { createMenu?: (item: { id: string, title: string, action: () => void }) => { appendTo: (path: string[]) => void } }
  for (const [id, title, action, dim, path] of menus) {
    try {
      ui.createMenu?.({ id, title, action: () => run(action, dim) }).appendTo(path)
    }
    catch (e) {
      console.warn('[outline] 菜单注册失败', id, e)
    }
  }
  window.addEventListener('keydown', (e) => {
    if (!e.shiftKey || !e.altKey || (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft'))
      return
    const container = document.getElementById('univer-container')
    if (!container || !container.contains(document.activeElement))
      return
    e.preventDefault()
    e.stopPropagation()
    run(e.key === 'ArrowRight' ? 'group' : 'ungroup')
  }, true)
  api.addEvent(api.Event.CommandExecuted, (event) => {
    const cmd = event as unknown as { id: string, params?: { subUnitId?: string, range?: { startRow: number, endRow: number, startColumn: number, endColumn: number } } }
    const dims: Record<string, [Dim, 1 | -1]> = {
      'sheet.mutation.insert-row': ['rows', 1],
      'sheet.mutation.remove-rows': ['rows', -1],
      'sheet.mutation.insert-col': ['cols', 1],
      'sheet.mutation.remove-col': ['cols', -1],
    }
    const hit = dims[cmd.id]
    const range = cmd.params?.range
    if (!hit || !range || !cmd.params?.subUnitId)
      return
    const sheet = api.getActiveWorkbook()?.getSheetBySheetId(cmd.params.subUnitId)
    if (!sheet)
      return
    const [dim, sign] = hit
    const at = dim === 'rows' ? range.startRow : range.startColumn
    const n = (dim === 'rows' ? range.endRow - range.startRow : range.endColumn - range.startColumn) + 1
    shiftOutline(sheet, dim, at, sign * n)
  })
}
