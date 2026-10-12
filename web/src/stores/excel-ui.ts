import type { Collaborator } from '@/features/excel/collaboration/sync-engine'
import type { SaveState } from '@/features/excel/persistence/manager'
import type { WorkbookSnapshot } from '@/features/excel/persistence/types'
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

export const useExcelUIStore = defineStore('excel-ui', () => {
  const activeRibbonTab = ref('home')
  const zoom = ref(100)
  const fileName = ref('订单审批跟踪表.xlsx')

  // ─── 侧边面板状态 ─────────────────────────────────────
  const activePanel = ref<'none' | 'sort-filter' | 'style' | 'formula' | 'collaboration' | 'snapshots' | 'pivot' | 'chart' | 'dashboard' | 'ai' | 'writeback' | 'comments'>('none')

  /** 其他功能（如异常发现）请 AI 助手分析时预填的问题；面板取走后清空 */
  const pendingAiQuestion = ref<string | null>(null)

  // ─── 协作状态 ─────────────────────────────────────────
  const collaborators = ref<Collaborator[]>([])
  const onlineCount = computed(() => collaborators.value.length + 1)

  // ─── 保存状态 ─────────────────────────────────────────
  /** 与服务端保存状态同步（见 persistence/manager.ts） */
  const saveStatus = ref<SaveState>('idle')
  const lastSavedAt = ref<number | null>(null)

  // ─── 快照列表 ─────────────────────────────────────────
  const snapshots = ref<WorkbookSnapshot[]>([])

  // ─── 排序/筛选状态 ────────────────────────────────────
  const sortConfig = ref<{
    column: number
    order: 'asc' | 'desc'
  } | null>(null)

  const filterConfig = ref<{
    column: number
    values: unknown[]
  } | null>(null)

  function setRibbonTab(tab: string) {
    activeRibbonTab.value = tab
  }

  function setZoom(value: number) {
    zoom.value = Math.max(25, Math.min(400, value))
  }

  function togglePanel(panel: 'sort-filter' | 'style' | 'formula' | 'collaboration' | 'snapshots' | 'pivot' | 'chart' | 'dashboard' | 'ai' | 'writeback' | 'comments') {
    activePanel.value = activePanel.value === panel ? 'none' : panel
  }

  function openPanel(panel: 'sort-filter' | 'style' | 'formula' | 'collaboration' | 'snapshots' | 'pivot' | 'chart' | 'dashboard' | 'ai' | 'writeback' | 'comments') {
    activePanel.value = panel
  }

  /** 打开 AI 助手并把问题预填到输入框（不自动发送，由用户确认） */
  function askAi(question: string) {
    pendingAiQuestion.value = question
    activePanel.value = 'ai'
  }

  function closePanel() {
    activePanel.value = 'none'
  }

  function setCollaborators(list: Collaborator[]) {
    collaborators.value = list
  }

  function setSaveStatus(status: SaveState) {
    saveStatus.value = status
    if (status === 'saved')
      lastSavedAt.value = Date.now()
  }

  function setSnapshots(list: WorkbookSnapshot[]) {
    snapshots.value = list
  }

  function setSortConfig(config: { column: number, order: 'asc' | 'desc' } | null) {
    sortConfig.value = config
  }

  function setFilterConfig(config: { column: number, values: unknown[] } | null) {
    filterConfig.value = config
  }

  return {
    // state
    activeRibbonTab,
    zoom,
    fileName,
    activePanel,
    collaborators,
    saveStatus,
    lastSavedAt,
    snapshots,
    sortConfig,
    filterConfig,
    pendingAiQuestion,
    // getters
    onlineCount,
    // actions
    setRibbonTab,
    setZoom,
    togglePanel,
    openPanel,
    askAi,
    closePanel,
    setCollaborators,
    setSaveStatus,
    setSnapshots,
    setSortConfig,
    setFilterConfig,
  }
})
