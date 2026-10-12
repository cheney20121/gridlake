import type { FUniver } from '@univerjs/core/facade'
import type { ShallowRef } from 'vue'
import type { LedgerPeriod, LedgerPeriodLock } from '@/api/ledger'
import type { DatasetDrill } from '@/components/excel/big-grid/drill'
import type { pivotDrillAt } from '@/features/excel/pivot/pivot-sheet'
import type { DrillTarget } from '@/features/excel/shop-ledger/ledger-workbook'
import { isAxiosError } from 'axios'
import { computed, ref } from 'vue'
import { getLedgerLocksApi, getLedgerPeriodsApi } from '@/api/ledger'
import { drillFromTarget } from '@/components/excel/big-grid/drill'
import { persistenceManager } from '@/features/excel/persistence/manager'
import { loadLedgerPeriod } from '@/features/excel/shop-ledger/ledger-workbook'
import { SUMMARY_SHEET_NAME } from '@/features/excel/shop-ledger/summary-template'

export type LedgerCommand = 'import' | 'detail' | 'shops' | 'coverage' | 'close'

/**
 * 店铺资金台账（gridlake）：月份与锁账、汇总表的加载与重建、台账功能入口（导入明细 / 店铺档案 / 月结完整性 / 月结中心）、
 * 选中汇总表 / 透视表格子时的「查看明细」下钻，以及「当前工作簿是不是台账」的判断。
 */
export function useLedger(options: {
  univerAPI: ShallowRef<FUniver | null>
  /** 当前显示：工作簿还是大数据表 */
  workspaceView: { value: 'workbook' | 'dataset' }
  refreshDatasets: () => Promise<void>
  openDataset: (id: string) => void
  datasetDrill: { value: DatasetDrill | null }
}) {
  const { univerAPI, refreshDatasets, openDataset, datasetDrill } = options

  const importDialogVisible = ref(false)
  const shopDialogVisible = ref(false)
  const coverageVisible = ref(false)
  const importing = ref(false)
  const importMessage = ref('')
  const periods = ref<LedgerPeriod[]>([])
  const currentPeriod = ref('')
  const monthCloseVisible = ref(false)
  /** 已冻结（审批中 / 已锁账）的店铺月份 */
  const ledgerLocks = ref<LedgerPeriodLock[]>([])
  const currentLock = computed(() => {
    const shops = periods.value.find(p => p.period === currentPeriod.value)?.shops ?? []
    const locked = ledgerLocks.value.filter(l => l.period === currentPeriod.value && l.status === 'locked').length
    const pending = ledgerLocks.value.filter(l => l.period === currentPeriod.value && l.status === 'pending').length
    return { total: shops.length, locked, pending }
  })

  /** 当前选中单元格对应的下钻目标；打开明细时固化为大数据表的下钻上下文 */
  const drillTarget = ref<DrillTarget | null>(null)
  /** 当前选中的透视表格子对应的下钻 */
  const pivotDrill = ref<ReturnType<typeof pivotDrillAt>>(null)
  const detailTitle = computed(() => pivotDrill.value?.title ?? drillTarget.value?.title ?? null)

  /**
   * 当前工作簿是资金台账（含「汇总表」页签）：顶栏才显示月份、锁账与「台账」菜单，
   * 其他工作簿从「数据中心」进入台账功能
   */
  const isLedgerWorkbook = ref(false)
  let ledgerCheckTimer: ReturnType<typeof setTimeout> | null = null

  function refreshLedgerContext() {
    if (ledgerCheckTimer)
      return
    // 不用 requestAnimationFrame：页面在后台标签页时不执行
    ledgerCheckTimer = setTimeout(() => {
      ledgerCheckTimer = null
      isLedgerWorkbook.value = !!univerAPI.value?.getActiveWorkbook()?.getSheetByName(SUMMARY_SHEET_NAME)
    })
  }

  /** 查看明细：带着汇总表 / 透视表选中单元格的上下文打开大数据表（自动核对） */
  function openDetail() {
    if (pivotDrill.value) {
      const { datasetId, ...drill } = pivotDrill.value
      datasetDrill.value = drill
      openDataset(datasetId)
      return
    }
    datasetDrill.value = drillFromTarget(drillTarget.value, currentPeriod.value)
    openDataset('ledger_detail')
  }

  /** request.ts 已对后端返回的业务错误弹出提示，这里只补充「服务连不上」的情况 */
  function notifyLedgerError(e: unknown, action: string) {
    console.error(`[ledger] ${action}失败:`, e)
    if (isAxiosError(e) && !e.response)
      ElMessage.error(`${action}失败：无法连接资金流水服务，请确认 gridlake 已启动`)
    else if (!isAxiosError(e))
      ElMessage.error(`${action}失败：${e instanceof Error ? e.message : String(e)}`)
  }

  async function refreshPeriods() {
    ;[periods.value, ledgerLocks.value] = await Promise.all([getLedgerPeriodsApi(), getLedgerLocksApi()])
  }

  /** 加载某个月份的聚合结果并重建汇总表 */
  async function loadPeriod(period: string, userInitiated = true) {
    if (!univerAPI.value || !period)
      return
    importing.value = true
    importMessage.value = `正在加载 ${period} 汇总…`
    try {
      const api = univerAPI.value
      // 重建汇总表不算用户编辑；用户主动切换月份 / 导入后才需要保存
      await persistenceManager.withoutTracking(() => loadLedgerPeriod(api, period))
      currentPeriod.value = period
      drillTarget.value = null
      if (userInitiated)
        persistenceManager.markDirty()
    }
    catch (e) {
      notifyLedgerError(e, '加载汇总')
    }
    finally {
      importing.value = false
    }
  }

  /** 批量导入完成：刷新月份并加载本次涉及的最新月份 */
  async function onImported(importedPeriods: string[]) {
    refreshDatasets()
    try {
      await refreshPeriods()
      await loadPeriod(importedPeriods[importedPeriods.length - 1] ?? currentPeriod.value)
    }
    catch (e) {
      notifyLedgerError(e, '刷新汇总')
    }
  }

  /** 店铺改名/关店后重建当前月份汇总（列名随店铺名变化） */
  async function onShopsChanged() {
    try {
      await refreshPeriods()
      if (currentPeriod.value)
        await loadPeriod(currentPeriod.value)
    }
    catch (e) {
      notifyLedgerError(e, '刷新汇总')
    }
  }

  function onLedgerCommand(command: LedgerCommand) {
    if (command === 'import') {
      importDialogVisible.value = true
    }
    else if (command === 'detail') {
      // 全部流水明细（当前月份）
      if (!currentPeriod.value) {
        ElMessage.warning('还没有导入店铺资金流水')
        return
      }
      datasetDrill.value = drillFromTarget(null, currentPeriod.value)
      openDataset('ledger_detail')
    }
    else if (command === 'shops') {
      shopDialogVisible.value = true
    }
    else if (command === 'coverage') {
      coverageVisible.value = true
    }
    else {
      monthCloseVisible.value = true
    }
  }

  /** 进入页面时自动加载最新月份（数据保存在后端，刷新页面不丢失） */
  /**
   * 资金流水：刷新月份列表；当前工作簿含「汇总表」时按其 A1 月份刷新（数据可能已更新），
   * 首次创建的默认工作簿生成最新月份的汇总表；其他工作簿不自动插入汇总表
   */
  async function initLedger(firstRun = false) {
    refreshDatasets()
    try {
      await refreshPeriods()
      const summary = univerAPI.value?.getActiveWorkbook()?.getSheetByName(SUMMARY_SHEET_NAME)
      const a1 = summary?.getRange(0, 0).getRawValue()
      const saved = typeof a1 === 'string' && periods.value.some(p => p.period === a1) ? a1 : null
      const target = saved ?? periods.value[0]?.period
      if (target && (summary || firstRun))
        await loadPeriod(target, firstRun)
    }
    catch (e) {
      // 服务未启动时不打扰用户，导入时再提示
      console.warn('[ledger] 资金流水服务不可用:', e)
    }
  }

  return {
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
    refreshPeriods,
    loadPeriod,
    onImported,
    onShopsChanged,
    onLedgerCommand,
    initLedger,
  }
}
