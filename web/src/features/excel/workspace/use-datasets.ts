import type { Ref } from 'vue'
import type { DatasetInfo } from '@/api/dataset'
import type { DatasetDrill } from '@/components/excel/big-grid/drill'
import { computed, ref, watch } from 'vue'
import { deleteDatasetApi, getDatasetsApi, getDatasetUsageApi, listDatasetTrashApi, renameDatasetApi } from '@/api/dataset'
import { refreshDatasetCharts } from '@/features/excel/chart/chart-manager'
import { setDatasetCatalog } from '@/features/excel/dashboard/slicer'
import { impactSummary, partsText } from '@/features/excel/dataset/usage-text'
import { invalidateDatasetFormulas } from '@/features/excel/formula/dataset-functions'
import { t } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'
import { escapeHtml } from './html'

/**
 * 工作区里的数据集：列表与当前打开的数据集、回收站数量，导入 / 关联视图 / 计算列 / 使用情况对话框的开关，
 * 以及改名、删除前提示影响范围（哪些工作簿、推送、视图、SQL 在用它）。
 */
export function useDatasets(options: {
  /** 当前显示：工作簿还是大数据表 */
  workspaceView: Ref<'workbook' | 'dataset'>
  /** 当前操作人（写入回收站记录） */
  operatorName: () => string
}) {
  const { workspaceView, operatorName } = options
  const ledgerUser = useLedgerUserStore()

  const datasets = ref<DatasetInfo[]>([])
  // 仪表盘部件据此判断数据集有没有切片器的字段
  // 同步更新：取数（单元格模式数据表、公式）可能紧接着加载数据集之后发生
  watch(datasets, list => setDatasetCatalog(list), { immediate: true, flush: 'sync' })
  const activeDatasetId = ref('ledger_detail')
  const activeDataset = computed(() => datasets.value.find(d => d.id === activeDatasetId.value) ?? null)
  const datasetDrill = ref<DatasetDrill | null>(null)
  /** 对账结果数据集：true 看明细表，false 看对账工作台 */
  const reconDetail = ref(false)

  /** 数据集回收站里的个数（大于 0 时数据集栏末尾显示「回收站」） */
  const datasetTrashCount = ref(0)
  const datasetTrashVisible = ref(false)

  async function refreshDatasets() {
    // 回收站对数据范围受限的用户不开放（请求会被拒绝并弹出提示）
    if (!ledgerUser.restricted)
      listDatasetTrashApi().then(r => datasetTrashCount.value = r.items.length).catch(() => {})
    try {
      datasets.value = await getDatasetsApi()
    }
    catch (e) {
      console.warn('[dataset] 数据集加载失败:', e)
    }
  }

  function openDataset(id: string) {
    if (id !== activeDatasetId.value)
      reconDetail.value = false
    activeDatasetId.value = id
    workspaceView.value = 'dataset'
  }

  const datasetImportVisible = ref(false)
  const joinVisible = ref(false)
  const joinLeftId = ref('')
  /** 正在修改的关联视图（为空时新建） */
  const joinViewId = ref('')
  /** 新建 / 修改对账的对话框（reconEditId 为空时新建） */
  const reconVisible = ref(false)
  const reconEditId = ref('')
  const calcDatasetId = ref<string | null>(null)
  const calcDataset = computed(() => datasets.value.find(d => d.id === calcDatasetId.value) ?? null)

  async function onDatasetsChanged(openId?: string) {
    await refreshDatasets()
    invalidateDatasetFormulas()
    refreshDatasetCharts()
    if (openId) {
      datasetDrill.value = null
      openDataset(openId)
    }
  }

  /** 导入完成：刷新数据集列表；公式结果缓存作废（DSUMIFS 引用的数据变了） */
  async function onDatasetImported(id: string, open: boolean) {
    await refreshDatasets()
    invalidateDatasetFormulas()
    if (open) {
      datasetDrill.value = null
      openDataset(id)
    }
  }

  const usageVisible = ref(false)
  const usageDataset = ref<DatasetInfo | null>(null)

  /** 删除 / 改名前查引用（查不到时不阻止操作） */
  async function datasetUsage(id: string) {
    return getDatasetUsageApi(id).catch(() => null)
  }

  async function onDatasetCommand(d: DatasetInfo, command: 'rename' | 'delete' | 'usage') {
    if (command === 'usage') {
      usageDataset.value = d
      usageVisible.value = true
      return
    }
    if (command === 'rename') {
      const u = await datasetUsage(d.id)
      const formulaBooks = (u?.workbooks ?? []).filter(w => w.parts.formula)
      const hint = formulaBooks.length
        ? `以下工作簿里的公式按名称引用它，改名后需同步修改：${formulaBooks.slice(0, 5).map(w => `「${w.name}」（${w.parts.formula} 个）`).join('、')}${formulaBooks.length > 5 ? ` 等 ${formulaBooks.length} 个` : ''}。透视表、图表、仪表盘、数据表按 ID 引用，不受影响`
        : t('没有公式按名称引用它；透视表、图表、仪表盘、数据表按 ID 引用，改名不受影响')
      const name = await ElMessageBox.prompt(hint, t('重命名数据集'), {
        inputValue: d.name,
        confirmButtonText: t('保存'),
      }).then(r => r.value, () => null)
      if (!name || name.trim() === d.name)
        return
      await renameDatasetApi(d.id, name.trim())
      ElMessage.success(t('已重命名'))
    }
    else {
      const u = await datasetUsage(d.id)
      const impact = u ? impactSummary(u) : ''
      const list = (u?.workbooks ?? []).slice(0, 5).map(w => `<li>工作簿「${escapeHtml(w.name)}」：${escapeHtml(partsText(w.parts))}</li>`).concat((u?.reports ?? []).slice(0, 3).map(r => `<li>定时推送「${escapeHtml(r.name)}」</li>`)).concat((u?.connectors ?? []).map(c => `<li>数据连接「${escapeHtml(c.name)}」（${escapeHtml(c.role)}）</li>`)).concat((u?.views ?? []).map(v => `<li>关联视图「${escapeHtml(v.name)}」</li>`)).concat((u?.sqlQueries ?? []).slice(0, 3).map(q => `<li>SQL 查询「${escapeHtml(q.name)}」（${escapeHtml(q.role)}）</li>`))
      const access = u?.access.total ? `<p style="margin-top:6px;color:#6B7280">近 30 天被查询 ${u.access.total.toLocaleString()} 次${u.access.lastDay ? `，最近一次 ${u.access.lastDay}` : ''}</p>` : ''
      const ok = await ElMessageBox.confirm(
        `<p>把数据集「${escapeHtml(d.name)}」（${d.rowCount.toLocaleString()} 行）移到回收站？数据会保留，可在回收站恢复。</p>${
          impact
            ? `<p style="margin-top:8px;color:#B91C1C">正在使用它的有：${impact}。在回收站期间这些地方会显示错误或暂停：</p><ul style="margin:4px 0 0 18px;list-style:disc">${list.join('')}</ul>`
            : '<p style="margin-top:8px;color:#0B5A2E">没有工作簿、推送、视图、SQL 查询或数据连接在用它。</p>'
        }${access}`,
        t('删除数据集'),
        { type: 'warning', dangerouslyUseHTMLString: true, confirmButtonText: t('移到回收站'), confirmButtonClass: 'ec-button--danger' },
      ).then(() => true, () => false)
      if (!ok)
        return
      await deleteDatasetApi(d.id, operatorName())
      ElMessage.success(t('已移到回收站，可在数据集栏末尾的「回收站」恢复'))
      if (activeDatasetId.value === d.id)
        workspaceView.value = 'workbook'
    }
    await refreshDatasets()
    invalidateDatasetFormulas()
    refreshDatasetCharts()
  }

  return {
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
  }
}
