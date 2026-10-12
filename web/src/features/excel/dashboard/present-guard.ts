/**
 * 演示模式下拦截用户的编辑操作：输入 / 双击编辑单元格、删除内容、粘贴剪切、填充柄、拖动单元格与行列宽高、
 * 拖动 / 缩放浮动部件、右键菜单。切片器、图表联动（同样会更新浮动对象数据）与程序内部的刷新不受影响。
 *
 * 不用 workbook.setEditable(false)：它同时挡住切片器保存选中项，且被挡时弹「该范围已被保护」提示
 */

import type { FUniver } from '@univerjs/core/facade'
import { findUniverServiceById } from '@/features/excel/univer-internal'

const BLOCKED = new Set([
  'sheet.command.clear-selection-all',
  'sheet.command.clear-selection-content',
  'sheet.command.clear-selection-format',
  'sheet.command.auto-fill',
  'sheet.command.move-range',
  'sheet.command.move-rows',
  'sheet.command.move-cols',
  'sheet.command.delta-row-height',
  'sheet.command.delta-column-width',
  'univer.command.cut',
  'univer.command.paste',
])

const EDIT_VISIBLE = 'sheet.operation.set-cell-edit-visible'
const SET_FLOAT = 'sheet.command.set-sheet-image'

interface Box { left: number, top: number, width: number, height: number }
/** 拖动时工作表 ID 在每个对象上，切片器保存时在外层 */
interface SetFloatParams {
  unitId?: string
  subUnitId?: string
  drawings?: { unitId?: string, subUnitId?: string, drawingId: string, drawingType?: number, transform?: Box }[]
}

interface DrawingManagerLike {
  refreshTransform: (params: { unitId: string, subUnitId: string, drawingId: string, drawingType?: number, transform: Box }[]) => void
}

/** 拖动被拦下后数据没变，但画面上的部件已跟着鼠标移动：按进入演示时的位置重新摆回去（只刷新显示，不写数据） */
function restoreFloats(api: FUniver, origin: Map<string, Box>, params: SetFloatParams) {
  // 画面层的浮动对象服务（工作表的绘图数据服务也有同名方法，但不刷新画面）
  const manager = findUniverServiceById<DrawingManagerLike>(api, 'univer.drawing-manager.service')
  if (!manager)
    return
  const items = (params.drawings ?? []).flatMap((d) => {
    const unitId = d.unitId ?? params.unitId
    const subUnitId = d.subUnitId ?? params.subUnitId
    const transform = origin.get(d.drawingId)
    return unitId && subUnitId && transform ? [{ unitId, subUnitId, drawingId: d.drawingId, drawingType: d.drawingType, transform: { ...transform } }] : []
  })
  manager.refreshTransform(items)
}

interface RenderManagerLike {
  getRenderUnitById: (unitId: string) => { scene?: { getTransformer?: () => { clearSelectedObjects?: () => void } | null } } | null
}

/** 去掉浮动部件的选中框与缩放把手（拖动被拦下后不再显示可编辑的样子） */
function clearFloatSelection(api: FUniver) {
  const unitId = api.getActiveWorkbook()?.getId()
  const render = findUniverServiceById<RenderManagerLike>(api, 'engine-render.render-manager.service')
  if (unitId)
    render?.getRenderUnitById(unitId)?.scene?.getTransformer?.()?.clearSelectedObjects?.()
}

const SIZE_KEYS = ['left', 'top', 'width', 'height'] as const

/** 各浮动对象进入演示时的位置（拖动过程中画面上的位置已经变了，不能拿当前位置比较） */
function snapshotFloats(api: FUniver) {
  const map = new Map<string, Box>()
  for (const sheet of api.getActiveWorkbook()?.getSheets() ?? []) {
    const doms = (sheet as unknown as { getAllFloatDoms?: () => { id: string, position: Box }[] }).getAllFloatDoms?.() ?? []
    for (const d of doms)
      map.set(d.id, { ...d.position })
  }
  return map
}

/** 浮动对象的位置或大小有变化（拖动、缩放）。切片器保存选中项走同一个命令，但位置不变 */
function movesFloat(origin: Map<string, Box>, params: SetFloatParams) {
  return (params.drawings ?? []).some((d) => {
    const before = origin.get(d.drawingId)
    const next = d.transform
    return !!before && !!next && SIZE_KEYS.some(k => Math.abs(before[k] - next[k]) > 0.5)
  })
}

function isBlocked(origin: Map<string, Box>, id: string, params: unknown) {
  if (BLOCKED.has(id) || id.startsWith('sheet.command.paste'))
    return true
  if (id === SET_FLOAT)
    return movesFloat(origin, (params ?? {}) as SetFloatParams)
  // 打开单元格编辑器（键入、双击、F2）；关闭编辑器照常放行
  return id.startsWith(EDIT_VISIBLE) && !!(params as { visible?: boolean } | undefined)?.visible
}

export function guardPresenting(api: FUniver, container: () => HTMLElement | null, isPresenting: () => boolean) {
  let origin = new Map<string, Box>()
  const sub = api.addEvent(api.Event.BeforeCommandExecute, (e) => {
    const { id, params } = e as unknown as { id: string, params?: unknown }
    if (!isPresenting() || !isBlocked(origin, id, params))
      return
    (e as unknown as { cancel: boolean }).cancel = true
    // 拖动结束时还会继续更新画面，等它收尾后再摆回去
    if (id === SET_FLOAT) {
      for (const delay of [0, 100]) {
        setTimeout(() => {
          restoreFloats(api, origin, params as SetFloatParams)
          clearFloatSelection(api)
        }, delay)
      }
    }
  })
  // 右键菜单由画布的右键按下打开：在捕获阶段先拦下
  const onPointerDown = (ev: PointerEvent) => {
    if (ev.button === 2 && isPresenting() && container()?.contains(ev.target as Node))
      ev.stopPropagation()
  }
  const onContextMenu = (ev: MouseEvent) => {
    if (isPresenting() && container()?.contains(ev.target as Node))
      ev.preventDefault()
  }
  window.addEventListener('pointerdown', onPointerDown, true)
  window.addEventListener('contextmenu', onContextMenu, true)
  return {
    /** 进入演示时调用：记下各浮动对象的位置 */
    start() {
      origin = snapshotFloats(api)
    },
    dispose() {
      sub.dispose()
      window.removeEventListener('pointerdown', onPointerDown, true)
      window.removeEventListener('contextmenu', onContextMenu, true)
    },
  }
}
