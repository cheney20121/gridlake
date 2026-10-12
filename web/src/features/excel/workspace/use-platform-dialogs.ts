import type { FUniver } from '@univerjs/core/facade'
import type { ShallowRef } from 'vue'
import type { WorkbookMeta } from '@/api/workbook'
import { useIntervalFn } from '@vueuse/core'
import { ref } from 'vue'
import { listConnectorsApi } from '@/api/connector'
import { listWorkbooksApi } from '@/api/workbook'
import { persistenceManager } from '@/features/excel/persistence/manager'
import { t } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'

/**
 * 「数据中心」里与当前工作簿无关的平台功能的对话框状态与入口：SQL 查询、外部 AI 接入（MCP）、操作审计、数据权限、
 * 模板（从模板新建 / 发布为模板）、定时推送、数据连接（含后台同步结果的轮询）。
 */
export function usePlatformDialogs(options: {
  univerAPI: ShallowRef<FUniver | null>
  workspaceView: { value: 'workbook' | 'dataset' }
  openWorkbook: (meta: WorkbookMeta) => Promise<void>
  /** 数据集有变化（后台同步完成等）：刷新数据集、公式、图表 */
  onDatasetsChanged: (openId?: string) => Promise<void>
}) {
  const { univerAPI, workspaceView, openWorkbook, onDatasetsChanged } = options
  const ledgerUser = useLedgerUserStore()

  // ── SQL 查询 ──
  const sqlVisible = ref(false)
  // ── 外部 AI 接入（MCP）──
  const mcpVisible = ref(false)
  const auditVisible = ref(false)
  const dataScopeVisible = ref(false)
  const anomalyVisible = ref(false)
  // ── 模板 ──
  const templateVisible = ref(false)
  const publishVisible = ref(false)
  const publishWorkbookId = ref<string | null>(null)

  function openPublish() {
    const meta = persistenceManager.getMeta()
    if (!meta)
      return ElMessage.warning(t('工作簿还没有保存到服务端，无法发布为模板'))
    publishWorkbookId.value = meta.id
    publishVisible.value = true
  }

  /** 发布前保存：模板取服务端保存的版本 */
  async function saveForPublish() {
    if (!persistenceManager.isDirty())
      return true
    const r = await persistenceManager.save({ description: t('发布为模板前保存') })
    if (r.state === 'saved' || r.state === 'unchanged')
      return true
    ElMessage.error(t('工作簿保存失败，请先处理保存问题再发布'))
    return false
  }

  async function onTemplateCreated(meta: WorkbookMeta, warnings: string[]) {
    await openWorkbook(meta)
    ElMessage.success(t('已用模板创建「{0}」', [meta.name]))
    if (warnings.length)
      ElNotification.warning({ title: t('部分引用需要手动检查'), message: warnings.join('；'), duration: 0 })
  }

  async function openWorkbookById(id: string) {
    const meta = (await listWorkbooksApi().catch(() => [])).find(w => w.id === id)
    if (!meta)
      return ElMessage.warning(t('工作簿不存在'))
    templateVisible.value = false
    await openWorkbook(meta)
  }

  // ── 定时推送 ──
  const reportVisible = ref(false)
  /** 从仪表盘面板进入：直接新建当前页的推送 */
  const reportCreate = ref(false)
  const reportWorkbookId = ref<string | null>(null)
  const reportSheetId = ref<string | null>(null)

  async function openReports(create = false) {
    // 推送按服务端保存的工作簿计算：有未保存的修改先保存
    if (create && persistenceManager.isDirty())
      await persistenceManager.save({ description: t('设置定时推送前保存') }).catch(() => {})
    reportWorkbookId.value = persistenceManager.getMeta()?.id ?? null
    reportSheetId.value = univerAPI.value?.getActiveWorkbook()?.getActiveSheet().getSheetId() ?? null
    reportCreate.value = create
    reportVisible.value = true
  }

  /** 打开推送任务对应的仪表盘页 */
  async function openReportSheet(workbookId: string, sheetId: string) {
    reportVisible.value = false
    if (persistenceManager.getMeta()?.id !== workbookId) {
      const meta = (await listWorkbooksApi().catch(() => [])).find(w => w.id === workbookId)
      if (!meta)
        return ElMessage.warning(t('工作簿不存在'))
      await openWorkbook(meta)
    }
    activateSheet(sheetId)
  }

  function activateSheet(sheetId: string) {
    const sheet = univerAPI.value?.getActiveWorkbook()?.getSheetBySheetId(sheetId)
    if (sheet) {
      workspaceView.value = 'workbook'
      sheet.activate()
    }
  }

  // ── 数据连接器 ──
  const connectorVisible = ref(false)
  /** 最近一次同步失败的（启用中）连接器数：按钮上显示红点 */
  const connectorFailures = ref(0)
  /** 各连接器最近一次同步的 ID：后台定时同步完成后自动刷新数据集 */
  let lastRunIds: Map<string, string> | null = null

  async function pollConnectors() {
    // 数据连接列表对数据范围受限的用户不开放：不轮询，否则每分钟弹一次「需要完整的数据权限」
    if (ledgerUser.restricted)
      return
    try {
      const { items } = await listConnectorsApi()
      connectorFailures.value = items.filter(c => c.enabled && c.lastRun?.status === 'failed').length
      const now = new Map(items.filter(c => c.lastRun && c.lastRun.status !== 'running').map(c => [c.id, c.lastRun!.id]))
      const changed = lastRunIds && items.some(c => c.lastRun?.status === 'success' && lastRunIds!.get(c.id) !== c.lastRun.id)
      lastRunIds = now
      if (changed && !connectorVisible.value)
        onDatasetsChanged()
    }
    catch {
      // 表格服务不可用时由其他请求提示，这里静默
    }
  }
  useIntervalFn(pollConnectors, 60_000, { immediateCallback: true })

  return {
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
  }
}
