import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import type { App, Component } from 'vue'
import type { ChartSpec } from './types'
import { createElement } from 'react'
import { createApp } from 'vue'
import { i18nPlugin, t } from '@/i18n'
import { CHART_COMPONENT_KEY } from './types'

/**
 * 图表管理：插入 / 修改 / 删除、跨工作表定位、数据变化后刷新
 *
 * 图表是 Univer 的浮动对象（float DOM）：随工作表滚动、可拖动缩放，定义存放在浮动对象的 data 里，
 * 随工作簿快照一起保存，打开工作簿时由 Univer 按 componentKey 重新创建。
 */

// ── 事件通道（图表组件 ↔ 页面） ──

type Listener = (payload: any) => void
const listeners = new Map<string, Set<Listener>>()

export const chartBus = {
  /**
   * select：双击 / 点「编辑」某个浮动对象，payload 为 ID（图表）或 { id, kind }（仪表盘部件）
   * spec：定义被修改；slicer：某工作表的切片器选择变化（payload 为 sheetId）
   * table-menu：数据表点了列头按钮（payload 为 { id, column, rect }，由页面显示筛选菜单）
   */
  on(event: 'select' | 'spec' | 'refresh-range' | 'refresh-dataset' | 'slicer' | 'table-menu' | 'cell-table', fn: Listener) {
    let set = listeners.get(event)
    if (!set) {
      set = new Set()
      listeners.set(event, set)
    }
    set.add(fn)
    return () => set!.delete(fn)
  },
  emit(event: string, payload?: unknown) {
    listeners.get(event)?.forEach(fn => fn(payload))
  },
}

// ── 注册组件 ──

let registered: FUniver | null = null
const refreshTimers = new Map<string, ReturnType<typeof setTimeout>>()

/**
 * 把 Vue 组件注册为浮动对象组件。Univer 的 Vue 适配需要额外插件，这里用一个 React 外壳挂载 Vue 应用。
 *
 * 外壳不使用 React hooks：开发环境里我们引入的 react 与 Univer 预构建包里的 react 可能是两份实例，
 * hooks 会报「Invalid hook call」；不带 hooks 的函数组件只产出普通元素对象，与 React 实例无关。
 * 挂载用回调 ref，并按图表 ID 缓存回调（引用不变，React 重新渲染时不会反复卸载 / 挂载）。
 */
const hostRefs = new Map<string, (el: HTMLDivElement | null) => void>()
const registeredKeys = new Set<string>()

/** 浮动对象定义的公共部分：v = 1，id 在工作簿内唯一 */
export interface FloatSpec {
  v: 1
  id: string
}

function isFloatSpec(d: unknown): d is FloatSpec {
  return !!d && typeof d === 'object' && (d as FloatSpec).v === 1 && typeof (d as FloatSpec).id === 'string'
}

/** 注册一种浮动对象组件（图表、切片器、指标卡、表格卡片共用同一套挂载方式） */
export function registerFloatView(api: FUniver, key: string, view: Component) {
  if (registeredKeys.has(key))
    return
  registeredKeys.add(key)
  function mountRef(spec: FloatSpec) {
    let ref = hostRefs.get(spec.id)
    if (!ref) {
      let app: App | null = null
      ref = (el) => {
        if (el && !app) {
          app = createApp(view, { spec, api })
          // 浮动部件是独立的 Vue 应用，模板里的 $t 需要单独装上
          app.use(i18nPlugin)
          app.mount(el)
        }
        else if (!el && app) {
          app.unmount()
          app = null
          hostRefs.delete(spec.id)
        }
      }
      hostRefs.set(spec.id, ref)
    }
    return ref
  }
  function Host(props: { data?: FloatSpec }) {
    if (!isFloatSpec(props.data))
      return null
    return createElement('div', { ref: mountRef(props.data), style: { width: '100%', height: '100%' } })
  }
  api.registerComponent(key, Host)
}

export function registerChartComponent(api: FUniver, view: Component) {
  if (registered === api)
    return
  registered = api
  registerFloatView(api, CHART_COMPONENT_KEY, view)

  // 单元格变化（含公式重算结果）→ 刷新引用该工作表的区域图表（防抖）
  api.addEvent(api.Event.CommandExecuted, (event) => {
    const cmd = event as unknown as { id: string, params?: { subUnitId?: string } }
    if (cmd.id !== 'sheet.mutation.set-range-values')
      return
    const sheetId = cmd.params?.subUnitId
    if (!sheetId)
      return
    clearTimeout(refreshTimers.get(sheetId))
    refreshTimers.set(sheetId, setTimeout(() => chartBus.emit('refresh-range', sheetId), 300))
  })
}

/** 数据集有变化（导入、同步、改计算列）：刷新所有数据集图表 */
export function refreshDatasetCharts(datasetId?: string) {
  chartBus.emit('refresh-dataset', datasetId)
}

// ── 定位 ──

export interface FloatLocation<T extends FloatSpec = FloatSpec> {
  sheet: FWorksheet
  floatId: string
  key: string
  spec: T
  position: { left: number, top: number, width: number, height: number }
}

export type ChartLocation = FloatLocation<ChartSpec>

/** 工作簿内某类（key 为空时所有类）浮动对象；sheetId 指定时只看该工作表 */
export function listFloats<T extends FloatSpec = FloatSpec>(api: FUniver, key?: string, sheetId?: string): FloatLocation<T>[] {
  const wb = api.getActiveWorkbook()
  if (!wb)
    return []
  const out: FloatLocation<T>[] = []
  for (const sheet of wb.getSheets()) {
    if (sheetId && sheet.getSheetId() !== sheetId)
      continue
    const doms = (sheet as unknown as { getAllFloatDoms?: () => { id: string, componentKey: string, data?: unknown, position: { left: number, top: number, width: number, height: number } }[] }).getAllFloatDoms?.() ?? []
    for (const d of doms) {
      if ((!key || d.componentKey === key) && registeredKeys.has(d.componentKey) && isFloatSpec(d.data))
        out.push({ sheet, floatId: d.id, key: d.componentKey, spec: d.data as T, position: d.position })
    }
  }
  return out
}

export function findFloat<T extends FloatSpec = FloatSpec>(api: FUniver, id: string): FloatLocation<T> | null {
  return (listFloats<T>(api).find(f => f.spec.id === id)) ?? null
}

/** 浮动对象所在的工作表 ID */
export function sheetIdOf(api: FUniver, id: string) {
  return findFloat(api, id)?.sheet.getSheetId() ?? null
}

/** 工作簿内所有图表（所有工作表） */
export function listCharts(api: FUniver): ChartLocation[] {
  return listFloats<ChartSpec>(api, CHART_COMPONENT_KEY)
}

export function findChart(api: FUniver, chartId: string) {
  return listCharts(api).find(c => c.spec.id === chartId) ?? null
}

// ── 插入 / 修改 / 删除 ──

const DEFAULT_SIZE = { width: 520, height: 320 }

/** 行号列宽度、列标行高度（隐藏时为 0） */
export function sheetHeaderSize(sheet: FWorksheet) {
  const cfg = (sheet as unknown as { getSheet?: () => { getConfig?: () => { rowHeader?: { width?: number, hidden?: number | boolean }, columnHeader?: { height?: number, hidden?: number | boolean } } } }).getSheet?.()?.getConfig?.()
  return {
    width: cfg?.rowHeader?.hidden ? 0 : (cfg?.rowHeader?.width ?? 46),
    height: cfg?.columnHeader?.hidden ? 0 : (cfg?.columnHeader?.height ?? 20),
  }
}

/** 数据区域（区域图表）或选区右侧的像素位置：插入图表时不挡住数据 */
function besideData(sheet: FWorksheet, spec: ChartSpec): { x: number, y: number } {
  let area = sheet.getSelection()?.getActiveRange() ?? null
  if (spec.source.kind === 'range' && spec.source.sheetId === sheet.getSheetId()) {
    try {
      area = sheet.getRange(spec.source.range)
    }
    catch {
      // 区域写错时退回选区
    }
  }
  const s = sheet as unknown as { getColumnWidth: (c: number) => number, getRowHeight: (r: number) => number }
  const endCol = area ? area.getLastColumn() : 0
  const startRow = area ? area.getRow() : 0
  // 浮动对象的坐标原点是画布左上角（含行号列、列标行），不是 A1 单元格
  const header = sheetHeaderSize(sheet)
  let x = header.width
  for (let c = 0; c <= endCol; c++)
    x += s.getColumnWidth(c)
  let y = header.height
  for (let r = 0; r < startRow; r++)
    y += s.getRowHeight(r)
  return { x: x + 24, y: y + 8 }
}

export function newChartId() {
  return `ch_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`
}

export function insertChart(api: FUniver, spec: ChartSpec, sheet?: FWorksheet) {
  const target = sheet ?? api.getActiveWorkbook()?.getActiveSheet()
  if (!target)
    throw new Error(t('没有打开的工作表'))
  // 区域图表放在数据右侧；数据集图表（仪表盘上）放在空位
  const { x, y } = spec.source.kind === 'range' ? besideData(target, spec) : freeSpot(api, target, DEFAULT_SIZE.width, DEFAULT_SIZE.height)
  const r = (target as unknown as { addFloatDomToPosition: (layer: unknown, id?: string) => { id: string } | null }).addFloatDomToPosition({
    componentKey: CHART_COMPONENT_KEY,
    initPosition: { startX: x, endX: x + DEFAULT_SIZE.width, startY: y, endY: y + DEFAULT_SIZE.height },
    data: spec,
    allowTransform: true,
  })
  if (!r)
    throw new Error(t('插入图表失败'))
  return r.id
}

/** 在工作表上找一块不与已有浮动对象重叠的位置（从左上角开始逐行扫描） */
export function freeSpot(api: FUniver, sheet: FWorksheet, width: number, height: number) {
  const header = sheetHeaderSize(sheet)
  const taken = listFloats(api, undefined, sheet.getSheetId()).map(f => f.position)
  // 有单元格数据的区域也视为占用（如在透视表页签上插图，放到数据右侧而不是盖住数据）
  const lastRow = sheet.getLastRow()
  const lastCol = sheet.getLastColumn()
  if (lastRow >= 0 && lastCol >= 0) {
    const s = sheet as unknown as { getColumnWidth: (c: number) => number, getRowHeight: (r: number) => number }
    let w = 0
    for (let c = 0; c <= lastCol; c++)
      w += s.getColumnWidth(c)
    let h = 0
    for (let r = 0; r <= Math.min(lastRow, 400); r++)
      h += s.getRowHeight(r)
    taken.push({ left: header.width, top: header.height, width: w + 12, height: lastRow > 400 ? 1e6 : h })
  }
  const gap = 12
  const maxX = header.width + Math.max(1400, (taken[taken.length - 1]?.width ?? 0) + width + 48)
  for (let y = header.height + gap; y < header.height + 6000; y += 16) {
    for (let x = header.width + gap; x + width <= maxX; x += 16) {
      const hit = taken.some(p => x < p.left + p.width + gap && x + width + gap > p.left && y < p.top + p.height + gap && y + height + gap > p.top)
      if (!hit)
        return { x, y }
    }
  }
  return { x: header.width + gap, y: header.height + gap }
}

/** 插入浮动对象；不指定位置时放在空位 */
export function insertFloat(api: FUniver, key: string, spec: FloatSpec, size: { width: number, height: number }, sheet?: FWorksheet, at?: { x: number, y: number }) {
  const target = sheet ?? api.getActiveWorkbook()?.getActiveSheet()
  if (!target)
    throw new Error(t('没有打开的工作表'))
  const { x, y } = at ?? freeSpot(api, target, size.width, size.height)
  const r = (target as unknown as { addFloatDomToPosition: (layer: unknown, id?: string) => { id: string } | null }).addFloatDomToPosition({
    componentKey: key,
    initPosition: { startX: x, endX: x + size.width, startY: y, endY: y + size.height },
    data: spec,
    allowTransform: true,
  })
  if (!r)
    throw new Error(t('插入失败'))
  return r.id
}

/** 修改浮动对象的定义（写回工作簿快照）并通知组件重画 */
export function updateFloat(api: FUniver, spec: FloatSpec) {
  const loc = findFloat(api, spec.id)
  if (!loc)
    throw new Error(t('对象已被删除'))
  const sheet = loc.sheet as unknown as { updateFloatDom: (id: string, cfg: unknown) => void }
  sheet.updateFloatDom(loc.floatId, { data: spec })
  chartBus.emit('spec', spec)
}

export function removeFloat(api: FUniver, id: string) {
  const loc = findFloat(api, id)
  if (loc)
    (loc.sheet as unknown as { removeFloatDom: (id: string) => void }).removeFloatDom(loc.floatId)
}

export function updateChart(api: FUniver, spec: ChartSpec) {
  const loc = findChart(api, spec.id)
  if (!loc)
    throw new Error(t('图表已被删除'))
  const sheet = loc.sheet as unknown as { updateFloatDom: (id: string, cfg: unknown) => void }
  sheet.updateFloatDom(loc.floatId, { data: spec })
  chartBus.emit('spec', spec)
}

export function removeChart(api: FUniver, chartId: string) {
  const loc = findChart(api, chartId)
  if (loc)
    (loc.sheet as unknown as { removeFloatDom: (id: string) => void }).removeFloatDom(loc.floatId)
}
