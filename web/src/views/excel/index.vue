<script setup lang="ts">
// 页面装配了多组组合式函数，回调里引用的函数在文件后面才定义（运行到回调时早已定义好），这里按调用顺序组织代码，不按定义顺序
/* eslint-disable ts/no-use-before-define */
import type { FUniver } from '@univerjs/core/facade'
import type { NotificationItem } from '@/api/comment'
import type { AnomalyFinding } from '@/api/dataset'
import type { WorkbookMeta } from '@/api/workbook'
import type { PaletteCommand } from '@/components/excel/command-palette/CommandPalette.vue'
import type { AppRibbon, RibbonTab } from '@/features/excel/ribbon/app-ribbon'
import type { DataCenterCommand, FileCommand } from '@/features/excel/workspace/palette-commands'
import { computed, nextTick, onMounted, onUnmounted, ref, watch, watchEffect } from 'vue'
import { useRouter } from 'vue-router'
import { getWorkbookMetaApi, listWorkbooksApi } from '@/api/workbook'
import AnomalyDialog from '@/components/excel/anomaly/AnomalyDialog.vue'
import ApprovalDrawer from '@/components/excel/ApprovalDrawer.vue'
import ApprovalModal from '@/components/excel/ApprovalModal.vue'
import AuditLogDialog from '@/components/excel/audit/AuditLogDialog.vue'
import DataScopeDialog from '@/components/excel/audit/DataScopeDialog.vue'
import DataTableView from '@/components/excel/big-grid/DataTableView.vue'
import ChartView from '@/components/excel/chart/ChartView.vue'
import CommandPalette from '@/components/excel/command-palette/CommandPalette.vue'
import ConnectorManagerDialog from '@/components/excel/connector/ConnectorManagerDialog.vue'
import KpiView from '@/components/excel/dashboard/KpiView.vue'
import SlicerView from '@/components/excel/dashboard/SlicerView.vue'
import TableCardView from '@/components/excel/dashboard/TableCardView.vue'
import DataPanel from '@/components/excel/data-panel/DataPanel.vue'
import DatasetImportDialog from '@/components/excel/dataset-import/DatasetImportDialog.vue'
import DatasetTrashDialog from '@/components/excel/dataset-import/DatasetTrashDialog.vue'
import DatasetUsageDialog from '@/components/excel/dataset-import/DatasetUsageDialog.vue'
import CalcColumnDialog from '@/components/excel/dataset-join/CalcColumnDialog.vue'
import DatasetJoinDialog from '@/components/excel/dataset-join/DatasetJoinDialog.vue'
import DataTableFloat from '@/components/excel/datatable/DataTableFloat.vue'
import DataTableMenuHost from '@/components/excel/datatable/DataTableMenuHost.vue'
import FormulaDiagnosticsDialog from '@/components/excel/formula-diagnostics/FormulaDiagnosticsDialog.vue'
import BatchImportDialog from '@/components/excel/ledger-admin/BatchImportDialog.vue'
import CoverageDrawer from '@/components/excel/ledger-admin/CoverageDrawer.vue'
import ShopManagerDialog from '@/components/excel/ledger-admin/ShopManagerDialog.vue'
import McpAccessDialog from '@/components/excel/mcp/McpAccessDialog.vue'
import MonthCloseDrawer from '@/components/excel/month-close/MonthCloseDrawer.vue'
import OutlineGutter from '@/components/excel/outline/OutlineGutter.vue'
import ReconDialog from '@/components/excel/recon/ReconDialog.vue'
import ReconWorkbench from '@/components/excel/recon/ReconWorkbench.vue'
import ReportManagerDialog from '@/components/excel/report/ReportManagerDialog.vue'
import SqlWorkbench from '@/components/excel/sql/SqlWorkbench.vue'
import PublishTemplateDialog from '@/components/excel/template/PublishTemplateDialog.vue'
import TemplateGalleryDialog from '@/components/excel/template/TemplateGalleryDialog.vue'
import SaveConflictDialog from '@/components/excel/workbook/SaveConflictDialog.vue'
import WorkbookManagerDialog from '@/components/excel/workbook/WorkbookManagerDialog.vue'
import { useBodyClass } from '@/composables/useBodyClass'
import { useUniver } from '@/composables/useUniver'
import { registerChartComponent, registerFloatView } from '@/features/excel/chart/chart-manager'
import { clearCursorTags, renderCursorTags } from '@/features/excel/collaboration/cursor-tags'
import { collabSession } from '@/features/excel/collaboration/session'
import { collaborationSync } from '@/features/excel/collaboration/sync-engine'
import { createCommentHint } from '@/features/excel/comments/cell-hint'
import { focusComment, reloadComments } from '@/features/excel/comments/state'
import { KPI_KEY, SLICER_KEY, TABLE_KEY } from '@/features/excel/dashboard/types'
import { formulaHelper } from '@/features/excel/data/formula-helper'
import { rowColManager } from '@/features/excel/data/row-col-manager'
import { watchSelection } from '@/features/excel/data/selection'
import { sortFilterEngine } from '@/features/excel/data/sort-filter'
import { styleManager } from '@/features/excel/data/style-manager'
import { registerTableFormulaInput } from '@/features/excel/datatable/formula-input'
import { DATA_TABLE_KEY } from '@/features/excel/datatable/types'
import { registerDatasetFunctions } from '@/features/excel/formula/dataset-functions'
import { registerImportRange } from '@/features/excel/formula/import-range'
import { registerOutline } from '@/features/excel/outline/outline'
import { persistenceManager } from '@/features/excel/persistence/manager'
import { openServerWorkbook } from '@/features/excel/persistence/workbook-session'
import { getPivotSpec, pivotDrillAt, registerPivotInteractions } from '@/features/excel/pivot/pivot-sheet'
import { registerAppRibbon } from '@/features/excel/ribbon/app-ribbon'
import { resolveDrillTarget, watchEntityAliasEdits } from '@/features/excel/shop-ledger/ledger-workbook'
import { useLedgerOperator } from '@/features/excel/shop-ledger/month-close'
import { buildPaletteCommands } from '@/features/excel/workspace/palette-commands'
import { appRibbonTabs as buildRibbonTabs } from '@/features/excel/workspace/ribbon-tabs'
import { useAnalysis } from '@/features/excel/workspace/use-analysis'
import { useDatasets } from '@/features/excel/workspace/use-datasets'
import { useLedger } from '@/features/excel/workspace/use-ledger'
import { usePlatformDialogs } from '@/features/excel/workspace/use-platform-dialogs'
import { useWorkbookSession } from '@/features/excel/workspace/use-workbook-session'
import { useXlsx } from '@/features/excel/workspace/use-xlsx'
import { t } from '@/i18n'
import { useApprovalStore } from '@/stores/excel-approval'
import { useExcelUIStore } from '@/stores/excel-ui'
import { useLedgerUserStore } from '@/stores/ledger-user'
import ExcelSidePanel from './components/ExcelSidePanel.vue'
import ExcelTopbar from './components/ExcelTopbar.vue'

defineOptions({ name: 'ExcelWorkspace' })

useBodyClass('app-excel')
const uiStore = useExcelUIStore()
const approvalStore = useApprovalStore()

const containerRef = ref<HTMLDivElement>()
const { univerAPI, error } = useUniver('univer-container')

const approvalModalVisible = ref(false)

// 引擎（定时器 / 协作 / 自动保存）清理
let engineCleanup: (() => void) | null = null
/** 依赖 Univer 实例的小部件（下钻提示、表名按钮、演示拦截），initEngines 中创建 */
let widgets: ReturnType<typeof attachWidgets> | null = null
/** 选中有评论的格子时格子旁的「💬 N」 */
let commentHint: ReturnType<typeof createCommentHint> | null = null

/** 初始化所有功能引擎 */
function initEngines() {
  if (!univerAPI.value)
    return

  const api = univerAPI.value

  // 初始化各模块
  sortFilterEngine.init(api)
  styleManager.init(api)
  rowColManager.init(api)
  formulaHelper.init(api)

  // 持久化：工作簿保存到服务端（打开工作簿前已初始化，这里只同步保存状态到界面）

  // 实时协作：加入当前工作簿的协作房间（服务端中转），别人的在线状态与光标显示在顶栏、面板和格子旁
  collabSession.attach(api, {
    getVersion: () => persistenceManager.getMeta()?.version ?? 0,
    adoptVersion: v => persistenceManager.adoptVersion(v),
    reload: async () => {
      const meta = persistenceManager.getMeta()
      if (!meta)
        return
      await openServerWorkbook(api, await getWorkbookMetaApi(meta.id))
      await refreshCellTablesAfterOpen()
    },
    track: fn => persistenceManager.withoutTracking(fn),
    isSuspended: () => persistenceManager.isSuspended(),
    save: () => persistenceManager.save(),
    toast: (message, level) => ElMessage({ message, type: level, duration: 5000 }),
    displayName: () => operatorName(),
  })
  persistenceManager.setCollab({
    before: () => collabSession.settle(),
    // 在协作房间里（包括重连中）保存都带上序号，服务端据此截断日志、通知别人新版本，不会让别人重新载入
    seq: () => (collabSession.status.value === 'off' ? undefined : collabSession.appliedSeq()) as number,
  })
  const syncCollabRoom = () => {
    const id = persistenceManager.getMeta()?.id
    if (id)
      collabSession.start(id)
    else
      collabSession.stop()
  }
  const offPersist = persistenceManager.onChange(() => syncCollabRoom())
  syncCollabRoom()
  const refreshPeers = () => {
    const list = collabSession.peers.value
    uiStore.setCollaborators(list.map(p => ({ id: p.id, name: p.name, color: p.color, lastActive: p.lastActive, cursor: p.cursor ? { row: p.cursor.row, col: p.cursor.col, sheetId: p.cursor.sheetId } : undefined })))
    renderCursorTags(api, list)
  }
  const stopPeersWatch = watch(collabSession.peers, refreshPeers)
  const sheetChanged = api.addEvent(api.Event.ActiveSheetChanged, refreshPeers)

  // 监听选区变化以同步光标（快照恢复重建工作簿后自动重新绑定）
  const selectionListener = watchSelection(api, (selections) => {
    // 选中汇总表 / 透视表的单元格时，顶栏「查看明细」按钮切换为对应下钻
    drillTarget.value = resolveDrillTarget(api)
    const active = api.getActiveWorkbook()?.getActiveSheet()
    pivotDrill.value = selections[0] && active && getPivotSpec(active)
      ? pivotDrillAt(active.getSheetId(), selections[0].startRow, selections[0].startColumn)
      : null
    const sel = selections[0]
    const activeSheet = api.getActiveWorkbook()?.getActiveSheet()
    if (sel && activeSheet)
      collabSession.broadcastCursor(activeSheet.getSheetId(), sel.startRow, sel.startColumn)
    widgets?.onSelection(sel)
    commentHint?.update(sel, uiStore.activePanel === 'comments' || presenting.value)
  })
  widgets = attachWidgets(api, openDetail)
  commentHint = createCommentHint(api, (row, col) => {
    const sheetId = api.getActiveWorkbook()?.getActiveSheet()?.getSheetId()
    if (sheetId)
      openCommentsAt(sheetId, row, col)
  })

  // 汇总表第 1 行（主体简称）修改后保存到后端
  const aliasWatcher = watchEntityAliasEdits(api, {
    onSaved: items => ElMessage.success(t('主体简称已保存：{0}', [items.map(i => i.alias ?? '（恢复默认）').join('、')])),
    onError: e => notifyLedgerError(e, '保存主体简称'),
  })

  // initEngines 在 watchEffect 异步回调中执行，不能在这里调用 onUnmounted，改为登记清理函数
  engineCleanup = () => {
    aliasWatcher.dispose()
    selectionListener.dispose()
    widgets?.dispose()
    widgets = null
    commentHint?.hide()
    commentHint = null
    stopPeersWatch()
    sheetChanged.dispose()
    offPersist()
    collabSession.stop()
    persistenceManager.setCollab(null)
    clearCursorTags()
  }
}

onUnmounted(() => {
  engineCleanup?.()
  engineCleanup = null
})

const workspaceView = ref<'workbook' | 'dataset'>('workbook')

// 操作人（写入版本历史、回收站记录）
const ledgerOperator = useLedgerOperator()
// 还没有操作人时固定一个（写入本地），保证版本历史里同一个人的名字稳定；可在月结中心修改。接入登录后改为账号名
if (!ledgerOperator.value.trim())
  ledgerOperator.value = collaborationSync.getUserName()
/** 首页整页跳转（离开前有未保存修改时浏览器会提示） */
const homeHref = useRouter().resolve({ name: 'ExcelHome' }).href
const operatorName = () => ledgerOperator.value.trim() || collaborationSync.getUserName()

// ─── 数据集（大数据表：数据留在服务端，canvas 渲染，支撑千万行）───
const {
  datasets,
  activeDatasetId,
  activeDataset,
  datasetDrill,
  datasetTrashCount,
  datasetTrashVisible,
  datasetImportVisible,
  joinVisible,
  joinLeftId,
  joinViewId,
  reconVisible,
  reconEditId,
  reconDetail,
  calcDatasetId,
  calcDataset,
  usageVisible,
  usageDataset,
  refreshDatasets,
  openDataset,
  onDatasetsChanged,
  onDatasetImported,
  onDatasetCommand,
} = useDatasets({ workspaceView, operatorName })

// ─── 店铺资金流水（gridlake）───
const {
  importDialogVisible,
  shopDialogVisible,
  coverageVisible,
  importing,
  importMessage,
  periods,
  currentPeriod,
  monthCloseVisible,
  ledgerLocks,
  currentLock,
  drillTarget,
  pivotDrill,
  detailTitle,
  isLedgerWorkbook,
  refreshLedgerContext,
  openDetail,
  notifyLedgerError,
  loadPeriod,
  onImported,
  onShopsChanged,
  onLedgerCommand,
  initLedger,
} = useLedger({ univerAPI, workspaceView, refreshDatasets, openDataset, datasetDrill })

const { xlsxInputRef, xlsxBusy, onXlsxCommand, onXlsxFile } = useXlsx({ univerAPI, currentPeriod })

// ─── 服务端工作簿 ───
const {
  workbookDialogVisible,
  conflictVisible,
  conflictMeta,
  currentWorkbookId,
  readOnlyWorkbook,
  handleSave,
  openWorkbook,
  onCreateWorkbook,
  onConflictOverwrite,
  onConflictLoadTheirs,
  onConflictSaveCopy,
  openInitialWorkbook,
} = useWorkbookSession({
  univerAPI,
  workspaceView,
  importing,
  importMessage,
  operatorName,
  refreshLedgerContext,
  notifyLedgerError,
  initLedger: () => initLedger(),
  refreshCellTablesAfterOpen: () => refreshCellTablesAfterOpen(),
})

// ─── 数据中心里的平台功能（SQL / MCP / 审计 / 数据权限 / 模板 / 定时推送 / 数据连接）───
const {
  sqlVisible,
  mcpVisible,
  auditVisible,
  dataScopeVisible,
  anomalyVisible,
  templateVisible,
  publishVisible,
  publishWorkbookId,
  openPublish,
  saveForPublish,
  onTemplateCreated,
  openWorkbookById,
  reportVisible,
  reportCreate,
  reportWorkbookId,
  reportSheetId,
  openReports,
  openReportSheet,
  activateSheet,
  connectorVisible,
  connectorFailures,
} = usePlatformDialogs({ univerAPI, workspaceView, openWorkbook, onDatasetsChanged })

// ─── 图表 / 仪表盘 / 演示 / 数据表 / 透视 ───
const {
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
} = useAnalysis({ univerAPI, workspaceView, datasets, refreshDatasets, detailTitle, notifyLedgerError })

const ledgerUser = useLedgerUserStore()

// ─── 评论 ───
watch(currentWorkbookId, id => reloadComments(id), { immediate: true })

/** 打开评论面板并选中某个格子 */
function openCommentsAt(sheetId: string, row: number, col: number) {
  uiStore.openPanel('comments')
  focusComment(sheetId, row, col)
}

/** 点铃铛里的通知：同一个工作簿直接定位；别的工作簿整页跳转（带上位置，打开后自动定位） */
function onOpenNotification(n: NotificationItem) {
  // 审批：待我审批 / 我发起的（结果）/ 已处理（被撤回的）；月结：打开月结中心
  if (n.kind.startsWith('approval_')) {
    approvalStore.drawerTab = n.kind === 'approval_request' ? 'todo' : n.kind === 'approval_cancelled' ? 'done' : 'started'
    approvalStore.toggleDrawer(true)
    return
  }
  if (n.kind.startsWith('close_')) {
    monthCloseVisible.value = true
    return
  }
  if (n.workbookId === currentWorkbookId.value) {
    reloadComments(n.workbookId)
    openCommentsAt(n.sheetId, n.row, n.col)
    return
  }
  const u = new URL(window.location.href)
  u.search = ''
  u.searchParams.set('wb', n.workbookId)
  u.searchParams.set('sheet', n.sheetId)
  u.searchParams.set('cell', `${n.row},${n.col}`)
  u.searchParams.set('comments', '1')
  window.location.assign(u.toString())
}

// ─── 顶栏菜单 ───
/** 功能区里的平台页签（「分析」、台账工作簿才显示的「台账」） */
let appRibbon: AppRibbon | null = null
watch(isLedgerWorkbook, v => appRibbon?.setTabVisible('ribbon.ledger', v))
/** 功能区：Excel 用户习惯的「插入 → 数据透视表 / 图表」，平台功能集中在「分析」页签，台账功能在上下文页签 */
/** 公式诊断 */
const diagnoseVisible = ref(false)

/** 平台功能：功能区与命令面板共用 */
function appRibbonTabs(): RibbonTab[] {
  return buildRibbonTabs({
    openPanel: name => uiStore.openPanel(name),
    openChart: openChartPanel,
    openDashboard: openDashboardPanel,
    present: enterPresent,
    importDataset: () => { datasetImportVisible.value = true },
    openSql: () => { sqlVisible.value = true },
    diagnose: () => { diagnoseVisible.value = true },
    ledger: onLedgerCommand,
  })
}

function setupRibbon(api: FUniver): AppRibbon {
  return registerAppRibbon(api, appRibbonTabs())
}

// ─── 命令面板（Alt+Q） ───
/** 公式诊断：点地址定位到单元格 */
function locateCell(sheetId: string, row: number, col: number) {
  diagnoseVisible.value = false
  activateSheet(sheetId)
  univerAPI.value?.getActiveWorkbook()?.getSheetBySheetId(sheetId)?.getRange(row, col).activate()
}

/** AI 对话链接（?ai=）指定的对话：AI 面板首次打开时选中 */
const aiLinkThread = ref<string | null>(null)
const paletteVisible = ref(false)
const paletteWorkbooks = ref<WorkbookMeta[]>([])
const paletteSheets = ref<{ id: string, name: string }[]>([])

async function openPalette() {
  const wb = univerAPI.value?.getActiveWorkbook()
  paletteSheets.value = wb?.getSheets().filter(s => !s.isSheetHidden()).map(s => ({ id: s.getSheetId(), name: s.getSheetName() })) ?? []
  paletteVisible.value = true
  paletteWorkbooks.value = await listWorkbooksApi().catch(() => paletteWorkbooks.value)
}

const paletteCommands = computed<PaletteCommand[]>(() => buildPaletteCommands({
  tabs: appRibbonTabs(),
  isLedgerWorkbook: isLedgerWorkbook.value,
  isAdmin: ledgerUser.isAdmin,
  authRequired: ledgerUser.authRequired,
  connectorFailures: connectorFailures.value,
  homeHref,
  sheets: paletteSheets.value,
  activeSheetId: univerAPI.value?.getActiveWorkbook()?.getActiveSheet()?.getSheetId(),
  workspaceView: workspaceView.value,
  datasets: datasets.value,
  currentWorkbookId: persistenceManager.getMeta()?.id,
  workbooks: paletteWorkbooks.value,
  save: handleSave,
  onFile: onFileCommand,
  onDataCenter: onDataCenterCommand,
  onLedger: onLedgerCommand,
  onApproval: onApprovalCommand,
  activateSheet,
  openDataset: (id) => {
    datasetDrill.value = null
    openDataset(id)
  },
  openWorkbook,
}))

/** 数据中心：与当前工作簿无关的平台功能 */
function onDataCenterCommand(command: DataCenterCommand) {
  if (command === 'sql')
    sqlVisible.value = true
  else if (command === 'anomaly')
    anomalyVisible.value = true
  else if (command === 'connector')
    connectorVisible.value = true
  else if (command === 'report')
    openReports()
  else if (command === 'mcp')
    mcpVisible.value = true
  else if (command === 'audit')
    auditVisible.value = true
  else if (command === 'datascope')
    dataScopeVisible.value = true
  else if (command === 'template')
    templateVisible.value = true
  else if (command === 'trash')
    datasetTrashVisible.value = true
  else
    onLedgerCommand(command)
}

/** 异常发现：打开对应数据集并带上筛选（与汇总表下钻同一套） */
function onAnomalyDrill(f: AnomalyFinding) {
  anomalyVisible.value = false
  datasetDrill.value = { title: f.drill.title, filters: f.drill.filters }
  openDataset(f.datasetId)
}

/** 异常发现：把这条结论带给 AI 助手，让它核实原因（预填问题，用户确认后发送） */
function onAnomalyAsk(f: AnomalyFinding) {
  anomalyVisible.value = false
  uiStore.askAi(`数据集「${f.datasetName}」里发现：${f.title}（${f.detail}）。请核实这个情况，并分析可能的原因，必要时用 compare_periods 拆解到店铺 / 渠道等维度。`)
}

/** 审批 */
function onApprovalCommand(command: 'start' | 'center') {
  if (command === 'start')
    handleStartApproval()
  else
    approvalStore.toggleDrawer()
}

/** 文件菜单（点工作簿名） */
function onFileCommand(command: FileCommand) {
  if (command === 'manage')
    workbookDialogVisible.value = true
  else if (command === 'template')
    templateVisible.value = true
  else if (command === 'publish')
    openPublish()
  else if (command === 'import' || command === 'export')
    onXlsxCommand(command)
  else if (command === 'snapshots')
    uiStore.openPanel('snapshots')
  else
    handleSave()
}

function onKeydown(e: KeyboardEvent) {
  // 演示模式：Esc 退出（没能进入浏览器全屏时，Esc 不会触发 fullscreenchange）
  if (presenting.value && e.key === 'Escape') {
    exitPresent()
    return
  }
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 's') {
    e.preventDefault()
    handleSave()
  }
  // Alt+Q：命令面板（与 Excel「告诉我」相同；Mac 上 Option+Q 输入的是 œ，按物理键判断）
  else if (e.altKey && !e.metaKey && !e.ctrlKey && e.code === 'KeyQ' && !presenting.value) {
    e.preventDefault()
    e.stopPropagation()
    openPalette()
  }
}

function onBeforeUnload(e: BeforeUnloadEvent) {
  if (persistenceManager.isDirty()) {
    persistenceManager.save({ auto: true }).catch(() => {})
    e.preventDefault()
    e.returnValue = ''
  }
}

/** 审批中心角标：进入页面时与每分钟刷新一次 */
let approvalTimer: ReturnType<typeof setInterval> | undefined
onMounted(() => {
  window.addEventListener('keydown', onKeydown)
  window.addEventListener('beforeunload', onBeforeUnload)
  approvalStore.refresh()
  approvalTimer = setInterval(() => approvalStore.refresh(), 60_000)
})
onUnmounted(() => {
  clearInterval(approvalTimer)
  window.removeEventListener('keydown', onKeydown)
  window.removeEventListener('beforeunload', onBeforeUnload)
  persistenceManager.dispose()
})

/** 当前工作簿正在进行的审批（有则不能再发起） */
const openApproval = computed(() => approvalStore.pendingFor(currentWorkbookId.value))

function handleStartApproval() {
  if (openApproval.value) {
    ElMessage.info(t('这个工作簿已有一张审批中的单据（审批人：{0}），请在审批中心查看或撤回', [openApproval.value.approverName]))
    return
  }
  approvalModalVisible.value = true
}

onMounted(() => {
  watchEffect(async () => {
    if (univerAPI.value) {
      await nextTick()
      if (!dataLoaded) {
        dataLoaded = true
        const api = univerAPI.value
        // 先注册数据集公式（DSUMIFS 等）与变更监听，再打开工作簿
        registerDatasetFunctions(api)
        registerImportRange(api)
        registerTableFormulaInput(api, (n, ex) => {
          ElMessage.success({ message: n > 1 ? `已把 ${n} 个数据表公式转为服务端计算（如 ${ex.to}）` : `已转为服务端计算：${ex.to}（按数据表当前的筛选）`, duration: 5000 })
        })
        // 透视表页签内点 ▾ / ▸ 折叠、改标签文字设别名
        registerPivotInteractions(api, e => notifyLedgerError(e, '更新透视表'))
        // 行列分组：右键菜单、Shift+Alt+→ / ←，插入删除行列时同步
        registerOutline(api, { onDirty: () => persistenceManager.markDirty(), onMessage: m => ElMessage.warning(m) })
        appRibbon = setupRibbon(api)
        // 图表组件须在打开工作簿前注册：工作簿快照里的图表按组件名重建
        registerChartComponent(api, ChartView)
        registerFloatView(api, SLICER_KEY, SlicerView)
        registerFloatView(api, KPI_KEY, KpiView)
        registerFloatView(api, TABLE_KEY, TableCardView)
        registerFloatView(api, DATA_TABLE_KEY, DataTableFloat)
        persistenceManager.init(api, operatorName)
        const firstRun = await openInitialWorkbook()
        // 推送消息里的看板链接：?wb=…&sheet=… 打开指定工作表
        const linkSheet = new URL(window.location.href).searchParams.get('sheet')
        if (linkSheet) {
          activateSheet(linkSheet)
          const u = new URL(window.location.href)
          u.searchParams.delete('sheet')
          window.history.replaceState(window.history.state, '', u)
        }
        // 评论通知的链接：?wb=…&sheet=…&cell=行,列&comments=1 打开评论面板并选中那个格子
        const linkComments = new URL(window.location.href).searchParams
        if (linkComments.get('comments')) {
          const [r, c] = (linkComments.get('cell') ?? '').split(',').map(Number)
          const sheetId = linkSheet || api.getActiveWorkbook()?.getActiveSheet()?.getSheetId()
          if (sheetId && Number.isInteger(r) && Number.isInteger(c))
            openCommentsAt(sheetId, r!, c!)
          else
            uiStore.openPanel('comments')
          const u = new URL(window.location.href)
          for (const k of ['comments', 'cell'])
            u.searchParams.delete(k)
          window.history.replaceState(window.history.state, '', u)
        }
        // AI 对话链接：?wb=…&ai=… 打开 AI 助手并选中该对话
        const linkThread = new URL(window.location.href).searchParams.get('ai')
        if (linkThread) {
          aiLinkThread.value = linkThread
          uiStore.openPanel('ai')
          const u = new URL(window.location.href)
          u.searchParams.delete('ai')
          window.history.replaceState(window.history.state, '', u)
        }
        // 首页「数据中心」打开数据集：?ds=…
        const linkDataset = new URL(window.location.href).searchParams.get('ds')
        if (linkDataset) {
          openDataset(linkDataset)
          const u = new URL(window.location.href)
          u.searchParams.delete('ds')
          window.history.replaceState(window.history.state, '', u)
        }
        initEngines()
        await initLedger(firstRun)
        refreshCellTablesAfterOpen()
        api.addEvent(api.Event.ActiveSheetChanged, () => {
          ensurePivotFresh()
          refreshLedgerContext()
        })
        // 增删、改名页签或换工作簿后，判断是否为台账工作簿
        api.addEvent(api.Event.CommandExecuted, refreshLedgerContext)
        refreshLedgerContext()
        ensurePivotFresh()
      }
    }
  })
})

/** 防止重复初始化 */
let dataLoaded = false
</script>

<template>
  <div class="excel-workspace" :class="{ presenting }">
    <div v-if="presenting" class="exit-present">
      <span class="text-gray-400">{{ $t('演示中 · 只读') }}</span>
      <button type="button" class="exit-btn" @click="exitPresent">
        {{ $t('退出演示') }} <kbd>Esc</kbd>
      </button>
    </div>
    <ExcelTopbar
      :home-href="homeHref"
      :read-only="readOnlyWorkbook"
      :xlsx-busy="xlsxBusy"
      :is-ledger-workbook="isLedgerWorkbook"
      :periods="periods"
      :current-period="currentPeriod"
      :current-lock="currentLock"
      :detail-title="detailTitle"
      :open-approval="openApproval"
      @file-command="onFileCommand"
      @open-palette="openPalette"
      @save="handleSave"
      @load-period="loadPeriod"
      @open-month-close="monthCloseVisible = true"
      @open-detail="openDetail"
      @approval-command="onApprovalCommand"
      @open-notification="onOpenNotification"
    />
    <input ref="xlsxInputRef" type="file" accept=".xlsx,.xlsm" class="hidden" @change="onXlsxFile">

    <!-- Main Content -->
    <div class="excel-body">
      <!-- 左侧数据面板：工作簿 / 数据集切换，导入数据与数据中心 -->
      <DataPanel
        :datasets="datasets"
        :view="workspaceView"
        :active-dataset-id="activeDatasetId"
        :workbook-name="uiStore.fileName"
        :connector-failures="connectorFailures"
        :trash-count="datasetTrashCount"
        :is-ledger-workbook="isLedgerWorkbook"
        @open-workbook="workspaceView = 'workbook'"
        @open-dataset="openDataset"
        @dataset-command="onDatasetCommand"
        @import-data="datasetImportVisible = true"
        @new-recon="reconEditId = ''; reconVisible = true"
        @data-center="onDataCenterCommand"
      />
      <div class="relative min-w-0 flex flex-1">
        <!-- 大数据表覆盖在工作簿之上（不隐藏 Univer 容器，避免其画布尺寸失效） -->
        <!-- 对账结果数据集默认显示对账工作台，「结果明细」切到表格 -->
        <ReconWorkbench
          v-if="activeDataset?.kind === 'recon' && !reconDetail"
          v-show="workspaceView === 'dataset'"
          :key="activeDataset.id"
          class="absolute inset-0 z-20"
          :dataset="activeDataset"
          @edit="(id: string) => { reconEditId = id; reconVisible = true }"
          @detail="reconDetail = true"
          @changed="onDatasetsChanged()"
        />
        <DataTableView
          v-if="activeDataset && !(activeDataset.kind === 'recon' && !reconDetail)"
          v-show="workspaceView === 'dataset'"
          class="absolute inset-0 z-20"
          :dataset="activeDataset"
          :drill="datasetDrill"
          @exit-drill="datasetDrill = null"
          @pivot="createPivotFromDataset"
          @chart="createChartFromDataset"
          @embed="embedDataTable"
          @calc="(id: string) => calcDatasetId = id"
          @join="(id: string) => { joinLeftId = id; joinViewId = ''; joinVisible = true }"
          @edit-join="(id: string) => { joinViewId = id; joinVisible = true }"
          @back-to-recon="reconDetail = false"
        />
        <!-- Univer Container -->
        <div id="univer-container" ref="containerRef" class="univer-container">
          <!-- Univer 将在此渲染 -->
        </div>
      </div>

      <ExcelSidePanel
        v-model:dashboard-edit-id="dashboardEditId"
        v-model:chart-edit-id="chartEditId"
        :univer-api="univerAPI"
        :datasets="datasets"
        :workbook-id="currentWorkbookId"
        :operator="operatorName()"
        :ai-link-thread="aiLinkThread"
        :chart-initial-dataset="chartInitialDataset"
        @datasets-changed="onDatasetsChanged()"
        @add-chart="(id: string) => createChartFromDataset(id, [])"
        @edit-chart="(id: string) => { chartInitialDataset = null; chartEditId = id; uiStore.openPanel('chart') }"
        @present="enterPresent"
        @schedule="openReports(true)"
        @pivot-converted="async (id: string) => { await onDatasetsChanged(); insertSummaryTable(id) }"
      />
    </div>

    <!-- 导入进度遮罩：导入十几万行明细时阻止误操作 -->
    <div v-if="importing" class="absolute inset-0 z-200 flex flex-col items-center justify-center gap-3 bg-white/70 text-sm text-gray-600">
      <div class="i-svg-spinners-90-ring-with-bg text-3xl text-[#107C41]" />
      <span>{{ importMessage }}</span>
    </div>

    <!-- Error -->
    <div v-if="error" class="error-banner">
      <el-alert :title="$t('Univer 初始化失败: {0}', [error])" type="error" :closable="false" />
    </div>

    <!-- Approval Modal -->
    <ApprovalModal v-model="approvalModalVisible" :univer-api="univerAPI" />

    <!-- 店铺资金台账：批量导入 / 店铺档案 / 月结完整性 -->
    <BatchImportDialog v-model:visible="importDialogVisible" @imported="onImported" />
    <ShopManagerDialog v-model:visible="shopDialogVisible" @changed="onShopsChanged" />
    <CoverageDrawer v-model:visible="coverageVisible" @open-period="loadPeriod" />
    <WorkbookManagerDialog
      v-model:visible="workbookDialogVisible"
      :current-id="currentWorkbookId"
      :operator="operatorName()"
      @open="openWorkbook"
      @create="onCreateWorkbook"
      @renamed="(m: WorkbookMeta) => uiStore.fileName = m.name"
      @template="templateVisible = true"
      @publish="openPublish"
    />
    <McpAccessDialog v-model:visible="mcpVisible" :datasets="datasets" :operator="operatorName()" />
    <AuditLogDialog v-model:visible="auditVisible" />
    <DataScopeDialog v-model:visible="dataScopeVisible" />
    <AnomalyDialog v-model:visible="anomalyVisible" :datasets="datasets" @drill="onAnomalyDrill" @ask="onAnomalyAsk" />
    <SqlWorkbench
      v-model:visible="sqlVisible"
      :datasets="datasets"
      :operator="operatorName()"
      @datasets-changed="onDatasetsChanged()"
      @open-dataset="(id: string) => { sqlVisible = false; onDatasetsChanged(id) }"
    />
    <TemplateGalleryDialog
      v-model:visible="templateVisible"
      :datasets="datasets"
      :operator="operatorName()"
      @created="onTemplateCreated"
      @import-data="datasetImportVisible = true"
      @publish="() => { templateVisible = false; openPublish() }"
      @open-workbook="openWorkbookById"
    />
    <PublishTemplateDialog
      v-model:visible="publishVisible"
      :workbook-id="publishWorkbookId"
      :workbook-name="uiStore.fileName"
      :operator="operatorName()"
      :before-publish="saveForPublish"
      @open-gallery="templateVisible = true"
    />
    <SaveConflictDialog
      v-model:visible="conflictVisible"
      :conflict="conflictMeta"
      @overwrite="onConflictOverwrite"
      @load-theirs="onConflictLoadTheirs"
      @save-copy="onConflictSaveCopy"
    />
    <ReconDialog v-model:visible="reconVisible" :datasets="datasets" :recon-id="reconEditId || undefined" @saved="(id: string) => { reconDetail = false; onDatasetsChanged(id) }" />
    <DatasetJoinDialog v-model:visible="joinVisible" :datasets="datasets" :left-id="joinLeftId" :view-id="joinViewId" @saved="(id: string) => onDatasetsChanged(id)" />
    <CalcColumnDialog
      v-if="calcDataset"
      :visible="!!calcDataset"
      :dataset="calcDataset"
      @update:visible="(v: boolean) => { if (!v) calcDatasetId = null }"
      @changed="onDatasetsChanged()"
    />
    <DatasetImportDialog v-model:visible="datasetImportVisible" :datasets="datasets" @imported="onDatasetImported" />
    <DatasetUsageDialog v-model:visible="usageVisible" :dataset="usageDataset" @open-workbook="(id: string) => { usageVisible = false; openWorkbookById(id) }" />
    <ReportManagerDialog
      v-model:visible="reportVisible"
      :workbook-id="reportWorkbookId"
      :sheet-id="reportSheetId"
      :create-on-open="reportCreate"
      @open="openReportSheet"
    />
    <ConnectorManagerDialog
      v-model:visible="connectorVisible"
      :datasets="datasets"
      @changed="onDatasetsChanged()"
      @failures="(n: number) => connectorFailures = n"
      @open="(id: string) => { connectorVisible = false; onDatasetsChanged(id) }"
    />
    <MonthCloseDrawer
      v-model:visible="monthCloseVisible"
      :period="currentPeriod"
      :periods="periods"
      @locks-changed="ledgerLocks = $event"
    />

    <DatasetTrashDialog v-model:visible="datasetTrashVisible" @changed="onDatasetsChanged()" />
    <CommandPalette v-model:visible="paletteVisible" :commands="paletteCommands" />
    <FormulaDiagnosticsDialog v-model:visible="diagnoseVisible" :univer-api="univerAPI" @locate="locateCell" />
    <DataTableMenuHost :univer-api="univerAPI" @changed="persistenceManager.markDirty()" />
    <OutlineGutter :univer-api="univerAPI" :visible="workspaceView === 'workbook'" />

    <!-- Approval Drawer -->
    <ApprovalDrawer />
  </div>
</template>

<style lang="scss" scoped>
.excel-workspace {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
  background: #fafbfc;
}

/* Main Content */
.excel-body {
  flex: 1 1 0;
  min-height: 0;
  display: flex;
  overflow: hidden;
  position: relative;
}

.univer-container {
  flex: 1 1 0;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  position: relative;
  width: 100%;
  height: 100%;

  /* 让 Univer 内部最外层容器自适应父容器 */
  :deep(> div) {
    height: 100%;
  }

  :deep(.univer-container) {
    width: 100%;
    height: 100%;
  }
}

/* 演示模式隐藏行号列标：画布整体向左上挪出标题的宽高，由外层裁掉（浮动部件跟着一起挪，不会错位），见 present-view.ts */
.univer-container.hide-headers :deep(> div) {
  margin-left: calc(-1 * var(--present-hx, 0px));
  margin-top: calc(-1 * var(--present-hy, 0px));
  width: calc(100% + var(--present-hx, 0px));
  height: calc(100% + var(--present-hy, 0px));
}

/* Error */
.error-banner {
  position: absolute;
  top: 60px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 100;
}

/* 演示模式：只保留工作表画布 */
.excel-workspace.presenting {
  :deep(.data-panel),
  > header,
  :deep(.side-panel) {
    display: none !important;
  }
}

.exit-present {
  @apply fixed right-4 top-3 z-300 flex items-center gap-2 rounded border border-[#e1e0d9] border-solid bg-white/90 py-0.5 pl-3 pr-0.5 text-xs shadow-sm opacity-40 transition-opacity hover:opacity-100;

  .exit-btn {
    @apply cursor-pointer rounded border-none bg-transparent px-2 py-1 text-xs text-gray-600 hover:bg-[#EEF2EF] hover:text-[#0E5A31];
  }

  kbd {
    @apply ml-1 rounded border border-[#e5e7eb] border-solid bg-white px-1 text-[10px] text-gray-400 font-sans;
  }
}
</style>
