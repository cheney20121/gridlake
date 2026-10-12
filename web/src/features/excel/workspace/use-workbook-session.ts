import type { FUniver } from '@univerjs/core/facade'
import type { ShallowRef } from 'vue'
import type { WorkbookMeta } from '@/api/workbook'
import type { LocalDraft } from '@/features/excel/persistence/manager'
import { computed, ref } from 'vue'
import { loadDemoWorkbook } from '@/features/excel/data/demo-workbook'
import { persistenceManager } from '@/features/excel/persistence/manager'
import { blankWorkbookJson, createServerWorkbook, loadWorkbookJson, openServerWorkbook, resolveInitialWorkbook } from '@/features/excel/persistence/workbook-session'
import { t } from '@/i18n'
import { useExcelUIStore } from '@/stores/excel-ui'
import { useLedgerUserStore } from '@/stores/ledger-user'

/**
 * 服务端工作簿的会话：当前打开的是哪个工作簿、对登录用户是否只读、保存（含冲突、离线草稿）、打开 / 新建 / 另存为，
 * 以及页面启动时选择并打开初始工作簿。保存与版本由 persistenceManager 负责，这里管界面上的流程。
 */
export function useWorkbookSession(options: {
  univerAPI: ShallowRef<FUniver | null>
  workspaceView: { value: 'workbook' | 'dataset' }
  /** 打开 / 加载中的遮罩 */
  importing: { value: boolean }
  importMessage: { value: string }
  operatorName: () => string
  refreshLedgerContext: () => void
  notifyLedgerError: (e: unknown, action: string) => void
  /** 打开工作簿后：按最新数据刷新台账汇总 / 单元格模式数据表（页面里后面才创建的功能，用函数延迟取用） */
  initLedger: () => Promise<void>
  refreshCellTablesAfterOpen: () => Promise<void>
}) {
  const { univerAPI, workspaceView, importing, importMessage, operatorName, refreshLedgerContext, notifyLedgerError } = options
  const uiStore = useExcelUIStore()
  const ledgerUser = useLedgerUserStore()
  const initLedger = () => options.initLedger()

  const workbookDialogVisible = ref(false)
  const conflictVisible = ref(false)
  const conflictMeta = ref<WorkbookMeta | null>(null)
  const currentWorkbookId = ref<string | null>(null)

  /** 当前工作簿对登录用户是只读的（别人共享给你、没有开放编辑）：修改不会被保存 */
  const currentMeta = ref<WorkbookMeta | null>(null)
  const readOnlyWorkbook = computed(() => !!currentMeta.value && ledgerUser.authRequired && ledgerUser.accessLevel(currentMeta.value) < 2)

  persistenceManager.onChange((state, meta) => {
    uiStore.setSaveStatus(state)
    if (meta) {
      if (meta.id !== currentWorkbookId.value && ledgerUser.authRequired && ledgerUser.accessLevel(meta) < 2)
        ElMessage.warning({ message: t('「{0}」是 {1} 共享给你的只读工作簿：可以查看和试算，但修改不会被保存。需要修改请另存为副本，或请所有者开放编辑', [meta.name, meta.ownerName || '其他人']), duration: 8000 })
      currentMeta.value = meta
      currentWorkbookId.value = meta.id
      uiStore.fileName = meta.name
    }
    refreshLedgerContext()
    if (state === 'conflict') {
      conflictMeta.value = persistenceManager.getConflict()
      conflictVisible.value = true
    }
  })

  /** 手动保存（Ctrl / ⌘ + S） */
  async function handleSave() {
    if (!persistenceManager.getMeta()) {
      ElMessage.warning(t('未连接服务，当前工作簿无法保存到服务端'))
      return
    }
    const r = await persistenceManager.save({ description: t('手动保存') })
    if (r.state === 'saved')
      ElMessage.success({ message: t('已保存'), duration: 1200 })
    else if (r.state === 'error')
      ElMessage.error(uiStore.saveStatus === 'offline' ? '无法连接服务，修改已暂存为本地草稿' : '保存失败')
  }

  /** 有未保存修改时先保存；冲突则中止（交给冲突对话框） */
  async function saveBeforeLeave(): Promise<boolean> {
    if (!persistenceManager.isDirty())
      return true
    const r = await persistenceManager.save({ auto: false })
    if (r.state === 'conflict')
      return false
    if (r.state === 'error') {
      return ElMessageBox.confirm(t('当前修改保存失败，继续将丢失这些修改（已存本地草稿的仍可在重新打开时恢复）。'), t('保存失败'), {
        type: 'warning',
        confirmButtonText: t('仍然继续'),
      }).then(() => true, () => false)
    }
    return true
  }

  /** 本地有未同步的草稿：询问是否恢复 */
  async function offerDraft(draft: LocalDraft | null) {
    if (!draft || !univerAPI.value)
      return
    const time = new Date(draft.savedAt).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
    const ok = await ElMessageBox.confirm(t('发现 {0} 未能保存到服务端的修改（离线草稿），是否恢复？', [time]), t('恢复未保存的修改'), {
      confirmButtonText: t('恢复'),
      cancelButtonText: t('丢弃'),
      type: 'info',
    }).then(() => true, () => false)
    if (ok) {
      await loadWorkbookJson(univerAPI.value, draft.json)
      persistenceManager.markDirty()
    }
    else {
      await persistenceManager.clearDraft(draft.workbookId)
    }
  }

  /** 打开服务端工作簿（含汇总表时按最新数据刷新） */
  async function openWorkbook(meta: WorkbookMeta) {
    const api = univerAPI.value
    if (!api || !(await saveBeforeLeave()))
      return
    importing.value = true
    importMessage.value = `正在打开「${meta.name}」…`
    try {
      const draft = await openServerWorkbook(api, meta)
      workspaceView.value = 'workbook'
      await offerDraft(draft)
    }
    catch (e) {
      notifyLedgerError(e, '打开工作簿')
    }
    finally {
      importing.value = false
    }
    await initLedger()
    await options.refreshCellTablesAfterOpen()
  }

  async function onCreateWorkbook(name: string, mode: 'blank' | 'copy', folderId: string | null = null) {
    const api = univerAPI.value
    if (!api)
      return
    try {
      if (mode === 'blank') {
        if (!(await saveBeforeLeave()))
          return
        await loadWorkbookJson(api, blankWorkbookJson(name))
        workspaceView.value = 'workbook'
      }
      const meta = await createServerWorkbook(name, operatorName(), folderId)
      ElMessage.success(mode === 'blank' ? `已新建「${meta.name}」` : `已另存为「${meta.name}」，后续修改保存到新工作簿`)
    }
    catch (e) {
      notifyLedgerError(e, '新建工作簿')
    }
  }

  async function onConflictOverwrite() {
    const r = await persistenceManager.save({ force: true, description: t('覆盖保存（处理冲突）') })
    if (r.state === 'saved') {
      conflictVisible.value = false
      ElMessage.success(t('已覆盖保存，对方的版本保留在版本历史中'))
    }
  }

  async function onConflictLoadTheirs() {
    const meta = conflictMeta.value ?? persistenceManager.getConflict()
    const api = univerAPI.value
    if (!meta || !api)
      return
    await openServerWorkbook(api, meta)
    conflictVisible.value = false
    ElMessage.success(t('已加载第 {0} 版', [meta.version]))
    await initLedger()
  }

  async function onConflictSaveCopy() {
    const name = `${uiStore.fileName}（我的副本）`
    const meta = await createServerWorkbook(name, operatorName())
    conflictVisible.value = false
    ElMessage.success(t('已另存为「{0}」', [meta.name]))
  }

  /** 启动：打开服务端工作簿；服务端没有工作簿时用默认模板创建一个；服务不可用时以本地模式运行 */
  async function openInitialWorkbook(): Promise<boolean> {
    const api = univerAPI.value!
    try {
      const meta = await resolveInitialWorkbook()
      if (meta) {
        const draft = await openServerWorkbook(api, meta)
        await offerDraft(draft)
        return false
      }
      await persistenceManager.withoutTracking(() => loadDemoWorkbook(api, uiStore.fileName))
      await createServerWorkbook(uiStore.fileName.replace(/\.xlsx$/i, ''), operatorName())
      return true
    }
    catch (e) {
      console.warn('[workbook] 服务端工作簿不可用，以本地模式运行:', e)
      if (!api.getActiveWorkbook())
        loadDemoWorkbook(api, uiStore.fileName)
      uiStore.setSaveStatus('offline')
      ElMessage.warning({ message: t('无法连接服务，工作簿未保存到服务端'), duration: 5000 })
      return false
    }
  }

  return {
    workbookDialogVisible,
    conflictVisible,
    conflictMeta,
    currentWorkbookId,
    readOnlyWorkbook,
    handleSave,
    saveBeforeLeave,
    openWorkbook,
    onCreateWorkbook,
    onConflictOverwrite,
    onConflictLoadTheirs,
    onConflictSaveCopy,
    openInitialWorkbook,
  }
}
