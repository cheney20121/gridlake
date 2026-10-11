<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { LedgerUserItem } from '@/api/approval'
import { computed, ref, watch } from 'vue'
import { listLedgerUsersApi } from '@/api/approval'
import { persistenceManager } from '@/features/excel/persistence/manager'
import { useApprovalStore } from '@/stores/excel-approval'
import { useExcelUIStore } from '@/stores/excel-ui'
import { useLedgerUserStore } from '@/stores/ledger-user'

/**
 * 发起文件审批：审批的是工作簿**已保存的版本**，发起前先保存当前修改；
 * 审批人从登录过的用户里选（未开启登录校验时填名字）
 */
const props = defineProps<{
  modelValue: boolean
  univerApi: FUniver | null
}>()

const emit = defineEmits<{
  'update:modelValue': [val: boolean]
}>()

const approvalStore = useApprovalStore()
const ledgerUser = useLedgerUserStore()
const uiStore = useExcelUIStore()

const visible = computed({
  get: () => props.modelValue,
  set: (val) => { emit('update:modelValue', val) },
})

const approverId = ref('')
const approverName = ref('')
const comment = ref('')
const submitting = ref(false)
const candidates = ref<LedgerUserItem[]>([])
const searching = ref(false)

async function searchUsers(keyword = '') {
  searching.value = true
  try {
    // 自己不能审批自己的单据
    candidates.value = (await listLedgerUsersApi(keyword)).filter(u => u.id !== ledgerUser.user?.id)
  }
  finally {
    searching.value = false
  }
}

/**
 * 读取活动工作表的数据区（不含表头），公式单元格返回计算结果
 * 用 getRawValues 而非 getValues：后者返回数字格式化后的显示文本（如 "95,285.50"），无法参与求和
 */
function readDataRows(): (string | number | boolean | null)[][] {
  const sheet = props.univerApi?.getActiveWorkbook()?.getActiveSheet()
  if (!sheet)
    return []

  const lastRow = sheet.getLastRow()
  const lastCol = sheet.getLastColumn()
  if (lastRow < 1 || lastCol < 0)
    return []

  const values = sheet.getRange(1, 0, lastRow, lastCol + 1).getRawValues()
  return values.map(row => row.map(v => v ?? null))
}

interface ApprovalPreview {
  fileName: string
  dataRows: number
  orderCount: number
  totalAmount: number
}

// 审批概况：每次打开弹窗时重新统计，保证反映最新编辑（仅供展示，不参与校验）
const preview = ref<ApprovalPreview | null>(null)

function buildPreview(): ApprovalPreview | null {
  if (!props.univerApi?.getActiveWorkbook())
    return null

  const rows = readDataRows()
  let totalAmount = 0
  const orderSet = new Set<string>()

  for (const row of rows) {
    const [orderNo, , , , , amount] = row
    if (orderNo)
      orderSet.add(String(orderNo))
    if (typeof amount === 'number')
      totalAmount += amount
  }

  return {
    fileName: uiStore.fileName,
    dataRows: rows.length,
    orderCount: orderSet.size,
    totalAmount,
  }
}

watch(visible, (val) => {
  if (val) {
    comment.value = ''
    approverId.value = ''
    approverName.value = ''
    preview.value = buildPreview()
    if (ledgerUser.authRequired)
      searchUsers()
  }
}, { immediate: true })

const canSubmit = computed(() => (ledgerUser.authRequired ? !!approverId.value : !!approverName.value.trim()))

async function handleSubmit() {
  const meta = persistenceManager.getMeta()
  if (!meta) {
    ElMessage.warning('当前工作簿还没有保存到服务端，不能发起审批')
    return
  }
  submitting.value = true
  try {
    // 审批的是已保存的版本：有未保存的修改先保存，保存不成功就不发起
    if (persistenceManager.isDirty()) {
      const r = await persistenceManager.save({ description: '发起审批前保存' })
      if (r.state !== 'saved') {
        ElMessage.error('保存当前修改失败，没有发起审批')
        return
      }
    }
    const v = persistenceManager.getMeta()?.version
    const a = await approvalStore.start({
      workbookId: meta.id,
      approverId: approverId.value || undefined,
      approverName: approverName.value.trim() || undefined,
      comment: comment.value.trim(),
      summary: preview.value ?? undefined,
    })
    ElMessage.success(`已发起审批：${a.approverName} 将审批「${a.workbookName}」的第 ${v ?? a.workbookVersion} 版`)
    visible.value = false
    approvalStore.toggleDrawer(true)
    approvalStore.drawerTab = 'started'
    approvalStore.load('started')
  }
  finally {
    submitting.value = false
  }
}
</script>

<template>
  <el-dialog v-model="visible" title="发起文件审批" width="480px" :close-on-click-modal="false">
    <div v-if="preview" class="flex flex-col gap-4">
      <div>
        <div class="mb-2 text-[13px] text-gray-800 font-semibold">
          文件信息预览
        </div>
        <div class="rounded-md bg-[#f5f7fa] p-3">
          <div class="preview-row">
            <span>文件名</span><strong>{{ preview.fileName }}</strong>
          </div>
          <div class="preview-row">
            <span>数据行数</span><strong>{{ preview.dataRows }} 行</strong>
          </div>
          <div class="preview-row">
            <span>订单总数</span><strong>{{ preview.orderCount }}</strong>
          </div>
          <div class="preview-row">
            <span>总金额</span>
            <strong>¥ {{ preview.totalAmount.toLocaleString('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 }) }}</strong>
          </div>
        </div>
        <div class="mt-1 text-xs text-gray-400">
          审批的是已保存的版本；发起时会先保存当前修改，之后再改动不影响审批人看到的内容
        </div>
      </div>

      <div>
        <div class="mb-2 text-[13px] text-gray-800 font-semibold">
          选择审批人
        </div>
        <el-select
          v-if="ledgerUser.authRequired"
          v-model="approverId"
          filterable
          remote
          :remote-method="searchUsers"
          :loading="searching"
          placeholder="输入姓名搜索（审批人需要登录过本系统）"
          class="w-full"
          no-data-text="没有找到：对方需要先登录过本系统"
        >
          <el-option v-for="u in candidates" :key="u.id" :label="u.name" :value="u.id" />
        </el-select>
        <el-input v-else v-model="approverName" placeholder="审批人姓名" />
      </div>

      <div>
        <div class="mb-2 text-[13px] text-gray-800 font-semibold">
          审批意见
        </div>
        <el-input v-model="comment" type="textarea" :rows="3" placeholder="请填写审批发起说明..." />
      </div>
    </div>

    <template #footer>
      <el-button @click="visible = false">
        取消
      </el-button>
      <el-button type="primary" :disabled="!canSubmit" :loading="submitting" @click="handleSubmit">
        确认发起
      </el-button>
    </template>
  </el-dialog>
</template>

<style lang="scss" scoped>
.preview-row {
  @apply flex items-center justify-between py-1 text-[13px] text-gray-500;

  strong {
    @apply font-semibold text-gray-800;
  }
}
</style>
