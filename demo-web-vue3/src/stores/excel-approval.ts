import type { Approval, ApprovalBox } from '@/api/approval'
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { approvalCountApi, cancelApprovalApi, createApprovalApi, decideApprovalApi, getApprovalApi, listApprovalsApi } from '@/api/approval'

/**
 * 文件审批（服务端存储）：审批单、历史、待办都在表格服务里，刷新页面、换浏览器、换设备都在。
 * 这里只缓存当前列表与角标数量
 */
export const useApprovalStore = defineStore('excel-approval', () => {
  const drawerVisible = ref(false)
  const drawerTab = ref<ApprovalBox>('todo')
  const items = ref<Approval[]>([])
  const loading = ref(false)
  /** 待我审批的数量（顶栏角标） */
  const pendingCount = ref(0)
  /** 我发起的审批中单据：用来判断某个工作簿是否已有审批在进行 */
  const mineOpen = ref<Approval[]>([])

  const pendingFor = computed(() => (workbookId: string | null) => (workbookId ? mineOpen.value.find(a => a.workbookId === workbookId) ?? null : null))

  /** 刷新角标与我发起的审批中单据（轻量，页面加载与定时调用） */
  async function refresh() {
    try {
      const [c, mine] = await Promise.all([approvalCountApi(), listApprovalsApi('started')])
      pendingCount.value = c.todo
      mineOpen.value = mine.filter(a => a.status === 'pending')
    }
    catch {}
  }

  /** 最近一次加载的序号：先发出的请求晚到时不能覆盖后发出的（连续切换页签 / 发起审批后会连发两次加载） */
  let loadSeq = 0

  async function load(tab: ApprovalBox = drawerTab.value) {
    drawerTab.value = tab
    const seq = ++loadSeq
    loading.value = true
    try {
      const list = await listApprovalsApi(tab)
      if (seq === loadSeq)
        items.value = list
    }
    finally {
      if (seq === loadSeq)
        loading.value = false
    }
  }

  async function start(input: Parameters<typeof createApprovalApi>[0]) {
    const a = await createApprovalApi(input)
    await refresh()
    return a
  }

  async function decide(id: string, approve: boolean, comment: string, operator?: string) {
    await decideApprovalApi(id, approve, comment, operator)
    await Promise.all([load(), refresh()])
  }

  async function cancel(id: string, comment: string, operator?: string) {
    await cancelApprovalApi(id, comment, operator)
    await Promise.all([load(), refresh()])
  }

  function detail(id: string) {
    return getApprovalApi(id)
  }

  async function toggleDrawer(force?: boolean) {
    drawerVisible.value = force ?? !drawerVisible.value
    if (drawerVisible.value)
      await load()
  }

  return { drawerVisible, drawerTab, items, loading, pendingCount, mineOpen, pendingFor, refresh, load, start, decide, cancel, detail, toggleDrawer }
})
