import type { IRange } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'
import type { Ref, ShallowRef } from 'vue'
import type { DatasetFilter, DatasetInfo, DatasetQuery } from '@/api/dataset'
import type { ChartSpec } from '@/features/excel/chart/types'
import type { CellButton, DrillHint } from '@/features/excel/data/drill-hint'
import type { CellTableSpec } from '@/features/excel/datatable/types'
import { ref, watch } from 'vue'
import { chartBus, listFloats } from '@/features/excel/chart/chart-manager'
import { CHART_COMPONENT_KEY } from '@/features/excel/chart/types'
import { restoreCrossFilters, setCrossFilterPersistGuard } from '@/features/excel/dashboard/cross-filter'
import { guardPresenting } from '@/features/excel/dashboard/present-guard'
import { hideGridlinesForPresent, hideHeadersForPresent } from '@/features/excel/dashboard/present-view'
import { createCellButton, createDrillHint } from '@/features/excel/data/drill-hint'
import { CellTableBlockedError, refreshCellTables, watchCellTables } from '@/features/excel/datatable/cell-table'
import { cellTableArea, cellTableAt, insertDataTable, listCellTables } from '@/features/excel/datatable/types'
import { persistenceManager } from '@/features/excel/persistence/manager'
import { createPivotSheet, getPivotSpec, hasLayout, refreshPivot } from '@/features/excel/pivot/pivot-sheet'
import { t } from '@/i18n'
import { useExcelUIStore } from '@/stores/excel-ui'

/**
 * 分析功能在编辑器里的入口与状态：图表面板、仪表盘面板、演示模式（全屏只读）、从大数据表放入工作表的数据表、
 * 单元格模式数据表（表名按钮与打开后的刷新）、透视表（从数据集新建、打开后重算）。
 */
export function useAnalysis(options: {
  univerAPI: ShallowRef<FUniver | null>
  workspaceView: Ref<'workbook' | 'dataset'>
  datasets: Ref<DatasetInfo[]>
  refreshDatasets: () => Promise<void>
  /** 选中汇总表 / 透视表格子时的下钻标题；没有则不显示格子旁的「查看明细」 */
  detailTitle: Readonly<Ref<string | null>>
  notifyLedgerError: (e: unknown, action: string) => void
}) {
  const { univerAPI, workspaceView, datasets, refreshDatasets, detailTitle, notifyLedgerError } = options
  const uiStore = useExcelUIStore()

  // 由 attachWidgets 创建（需要 Univer 实例），随页面销毁
  /** 格子旁的「查看明细」提示 */
  let drillHint: DrillHint | null = null
  /** 选中单元格模式数据表里的格子时，旁边显示的表名按钮 */
  let tableChip: CellButton | null = null
  /** 表名按钮当前对应的单元格模式数据表 */
  let chipTableId: string | null = null
  /** 演示模式的编辑拦截 */
  let presentGuard: ReturnType<typeof guardPresenting> | null = null

  // ─── 图表 ───
  /** 正在编辑的图表（为空时面板为新建） */
  const chartEditId = ref<string | null>(null)
  /** 从数据表「图表分析」带入的数据集与筛选 */
  const chartInitialDataset = ref<{ datasetId: string, filters: DatasetFilter[] } | null>(null)

  function openChartPanel() {
    if (uiStore.activePanel === 'chart' && !chartEditId.value) {
      uiStore.closePanel()
      return
    }
    chartEditId.value = null
    chartInitialDataset.value = null
    workspaceView.value = 'workbook'
    uiStore.openPanel('chart')
  }

  function createChartFromDataset(datasetId: string, filters: DatasetFilter[]) {
    chartEditId.value = null
    chartInitialDataset.value = { datasetId, filters }
    workspaceView.value = 'workbook'
    uiStore.openPanel('chart')
  }

  const dashboardEditId = ref<string | null>(null)

  // 双击图表 / 点图表上的「编辑」：打开面板编辑该图表
  // 仪表盘部件（切片器 / 指标卡 / 表格）payload 为 { id, kind }，图表为 ID
  chartBus.on('select', (payload: string | { id: string, kind: string }) => {
    if (typeof payload === 'object') {
      dashboardEditId.value = payload.id
      uiStore.openPanel('dashboard')
      return
    }
    chartInitialDataset.value = null
    chartEditId.value = payload
    uiStore.openPanel('chart')
  })

  // ─── 仪表盘 ───
  const presenting = ref(false)
  function isPresenting() {
    return presenting.value
  }
  // 演示是只读的：演示里点图表联动只在本次演示有效，不写进工作簿（不产生修改）
  setCrossFilterPersistGuard(() => !presenting.value)

  // ─── 数据表（嵌入工作表） ───
  /** 从大数据表「放入工作表」：带着当前筛选 / 排序 / 搜索，插入到工作簿当前工作表的选中单元格处 */
  function embedDataTable(datasetId: string, query: DatasetQuery) {
    const api = univerAPI.value
    const ds = datasets.value.find(d => d.id === datasetId)
    if (!api || !ds)
      return
    workspaceView.value = 'workbook'
    try {
      const spec = insertDataTable(api, { datasetId, datasetName: ds.name, filters: query.filters, sort: query.sort, keyword: query.keyword })
      persistenceManager.markDirty()
      dashboardEditId.value = spec.id
      uiStore.openPanel('dashboard')
      ElMessage.success(t('已在选中的单元格处插入数据表「{0}」，公式可用 =TSUM("{1}[列名]") 引用', [spec.name, spec.name]))
    }
    catch (e) {
      ElMessage.error(e instanceof Error ? e.message : String(e))
    }
  }

  /** 透视表转为数据表之后：在新工作表里放入汇总数据集的数据表（数据集列表需已刷新） */
  function insertSummaryTable(datasetId: string) {
    const api = univerAPI.value
    const ds = datasets.value.find(d => d.id === datasetId)
    const wb = api?.getActiveWorkbook()
    if (!api || !wb || !ds)
      return
    const taken = new Set(wb.getSheets().map(s => s.getSheetName()))
    let name = ds.name.slice(0, 28)
    for (let i = 2; taken.has(name); i++)
      name = `${ds.name.slice(0, 26)}${i}`
    const sheet = wb.insertSheet(name)
    wb.setActiveSheet(sheet)
    try {
      const spec = insertDataTable(api, { datasetId, datasetName: ds.name })
      persistenceManager.markDirty()
      ElMessage.success(t('已转为数据表「{0}」：表头可按值筛选、排序；在数据面板里可给「{1}」加计算列', [spec.name, ds.name]))
    }
    catch (e) {
      ElMessage.error(e instanceof Error ? e.message : String(e))
    }
  }

  function openDashboardPanel() {
    if (uiStore.activePanel === 'dashboard') {
      uiStore.closePanel()
      return
    }
    workspaceView.value = 'workbook'
    dashboardEditId.value = null
    uiStore.openPanel('dashboard')
  }

  function setUniverChrome(visible: boolean) {
    const api = univerAPI.value as unknown as { setUIVisible?: (k: string, v: boolean) => unknown } | null
    for (const part of ['header', 'toolbar', 'footer'])
      api?.setUIVisible?.(part, visible)
  }

  let restoreGridlines: (() => Promise<void>) | null = null
  async function restoreNow() {
    const restore = restoreGridlines
    restoreGridlines = null
    if (restore)
      await persistenceManager.withoutTracking(restore)
  }

  /**
   * 演示模式：全屏，隐藏顶部栏、工具栏、页签栏与侧边面板（Esc 或「退出演示」恢复）。
   * 演示期间不能编辑（present-guard 拦截编辑操作）：误触不会改动单元格、拖动部件；切片器、图表联动照常可用
   */
  async function enterPresent() {
    if (presenting.value)
      return
    workspaceView.value = 'workbook'
    presenting.value = true
    presentGuard?.start()
    uiStore.closePanel()
    setUniverChrome(false)
    // 全屏请求依赖点击带来的用户激活，先发出再做耗时的事
    const fullscreen = document.querySelector('.excel-workspace')?.requestFullscreen?.().catch(() => {})
    if (univerAPI.value) {
      const api = univerAPI.value
      // 网格线是工作表数据：演示期间临时隐藏，退出恢复；两次改动都不算用户修改。行号列标用 CSS 挪出可见区域，不改数据
      const restoreGrid = await persistenceManager.withoutTracking(() => hideGridlinesForPresent(api))
      const restoreHeaders = hideHeadersForPresent(api, document.getElementById('univer-container'))
      restoreGridlines = async () => {
        restoreHeaders()
        await restoreGrid()
      }
      // 等待期间已退出演示（Esc）：立刻恢复
      if (!presenting.value)
        await restoreNow()
    }
    await fullscreen
  }

  function exitPresent() {
    if (!presenting.value)
      return
    presenting.value = false
    setUniverChrome(true)
    restoreNow()
    if (document.fullscreenElement)
      document.exitFullscreen().catch(() => {})
  }

  document.addEventListener('fullscreenchange', () => {
    if (!document.fullscreenElement)
      exitPresent()
  })

  // ─── 数据透视表 ───
  /** 格子旁的「查看明细」：能下钻的格子才显示 */
  function updateDrillHint(sel: IRange | undefined) {
    const title = detailTitle.value
    if (title && sel && !presenting.value && workspaceView.value === 'workbook')
      drillHint?.show(sel.startRow, sel.startColumn, title)
    else
      drillHint?.hide()
  }
  watch([detailTitle, workspaceView, presenting], ([t, v, p]) => {
    if (!t || v !== 'workbook' || p)
      drillHint?.hide()
    if (v !== 'workbook' || p) {
      tableChip?.hide()
      chipTableId = null
    }
  })

  // ─── 单元格模式数据表 ───
  function updateTableChip(sel: IRange | undefined) {
    const sheet = univerAPI.value?.getActiveWorkbook()?.getActiveSheet()
    const table = sel && sheet && workspaceView.value === 'workbook' && !presenting.value ? cellTableAt(sheet, sel.startRow, sel.startColumn) : null
    if (!sel || !table) {
      chipTableId = null
      tableChip?.hide()
      return
    }
    chipTableId = table.id
    const rows = Math.max(cellTableArea(table).rows - 1, 0)
    // 贴在选中的格子旁（表头可能已滚出视野，挂在表头上会看不到）
    tableChip?.show(sel.startRow, sel.startColumn, `${table.name} · ${rows.toLocaleString()} 行 ⚙`, '单元格模式的数据表：随数据集、切片器刷新。点击修改列、筛选、排序或切换显示方式')
  }

  function editChipTable() {
    if (!chipTableId)
      return
    workspaceView.value = 'workbook'
    dashboardEditId.value = chipTableId
    uiStore.openPanel('dashboard')
  }

  function onCellTableError(e: unknown, spec: CellTableSpec) {
    if (e instanceof CellTableBlockedError)
      ElMessage.warning({ message: e.message, duration: 6000 })
    else
      notifyLedgerError(e, t('刷新数据表「{0}」', [spec.name]))
  }

  /** 打开工作簿后按最新数据刷新单元格模式数据表（不算用户修改） */
  async function refreshCellTablesAfterOpen() {
    const api = univerAPI.value
    if (api) {
      // 恢复图表联动（随工作簿保存）：发起联动的图表还在才恢复
      restoreCrossFilters(api, (sheetId, chartId) => listFloats<ChartSpec>(api, CHART_COMPONENT_KEY).some(c => c.spec.id === chartId && c.sheet.getSheetId() === sheetId), fn => void persistenceManager.withoutTracking(fn))
    }
    if (!api || !listCellTables(api).length)
      return
    if (!datasets.value.length)
      await refreshDatasets()
    await refreshCellTables(api, () => true, { wrap: fn => persistenceManager.withoutTracking(fn), onError: onCellTableError })
  }

  /** 打开工作簿后首次切到透视表页签：按最新数据重算（不算用户编辑），并打开字段面板 */
  async function ensurePivotFresh() {
    const sheet = univerAPI.value?.getActiveWorkbook()?.getActiveSheet()
    const spec = getPivotSpec(sheet)
    if (!sheet || !spec)
      return
    if (uiStore.activePanel === 'none')
      uiStore.openPanel('pivot')
    if (hasLayout(sheet.getSheetId()) || !spec.request.values.length)
      return
    await persistenceManager.withoutTracking(() => refreshPivot(sheet, spec)).catch(e => notifyLedgerError(e, t('刷新透视表')))
  }

  /** 从数据表「透视分析」：以当前筛选新建透视表页签 */
  async function createPivotFromDataset(datasetId: string, filters: DatasetFilter[]) {
    const api = univerAPI.value
    const ds = datasets.value.find(d => d.id === datasetId)
    if (!api || !ds)
      return
    workspaceView.value = 'workbook'
    const sheet = await createPivotSheet(api, { version: 1, datasetId: ds.id, datasetName: ds.name, request: { rows: [], columns: [], values: [], filters } })
    sheet.getRange(0, 0).setValue(`数据透视表 · ${ds.name}：从右侧字段列表把字段拖入「行」「列」「值」`)
    uiStore.openPanel('pivot')
    persistenceManager.markDirty()
  }

  /**
   * 创建依赖 Univer 实例的小部件：格子旁的「查看明细」、单元格数据表的表名按钮、单元格数据表的刷新监听、演示模式的编辑拦截。
   * 选区变化时调用返回的 onSelection；页面销毁时调用 dispose
   */
  function attachWidgets(api: FUniver, openDetail: () => void) {
    drillHint = createDrillHint(api, openDetail)
    tableChip = createCellButton(api, 'app.cell-table', editChipTable)
    const stopCellTables = watchCellTables(api, onCellTableError)
    presentGuard = guardPresenting(api, () => document.getElementById('univer-container'), isPresenting)
    return {
      onSelection(sel: IRange | undefined) {
        updateDrillHint(sel)
        updateTableChip(sel)
      },
      dispose() {
        drillHint?.hide()
        drillHint = null
        presentGuard?.dispose()
        presentGuard = null
        stopCellTables()
        tableChip?.hide()
        tableChip = null
      },
    }
  }

  return {
    chartEditId,
    chartInitialDataset,
    dashboardEditId,
    presenting,
    openChartPanel,
    createChartFromDataset,
    openDashboardPanel,
    embedDataTable,
    enterPresent,
    exitPresent,
    createPivotFromDataset,
    insertSummaryTable,
    ensurePivotFresh,
    refreshCellTablesAfterOpen,
    attachWidgets,
  }
}
