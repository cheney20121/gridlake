<script setup lang="ts">
import type { Approval, ApprovalBox, ApprovalStatus } from '@/api/approval'
import { computed, reactive, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useLedgerOperator } from '@/features/excel/shop-ledger/month-close'
import { t } from '@/i18n'
import { useApprovalStore } from '@/stores/excel-approval'
import { useLedgerUserStore } from '@/stores/ledger-user'

defineOptions({ name: 'ApprovalDrawer' })

/** 审批中心：待我审批 / 我发起的 / 已处理 /（管理员）全部。数据在服务端，换浏览器也在 */
const store = useApprovalStore()
const ledgerUser = useLedgerUserStore()
const operator = useLedgerOperator()
const router = useRouter()

const drawerVisible = computed({
  get: () => store.drawerVisible,
  set: (val) => { store.toggleDrawer(val) },
})

const activeTab = computed({
  get: () => store.drawerTab,
  set: (val: ApprovalBox) => { store.load(val) },
})

const STATUS: Record<ApprovalStatus, { label: string, type: 'warning' | 'success' | 'danger' | 'info' }> = {
  pending: { label: t('审批中'), type: 'warning' },
  approved: { label: t('已通过'), type: 'success' },
  rejected: { label: t('已驳回'), type: 'danger' },
  cancelled: { label: t('已撤回'), type: 'info' },
}

const ACTION_LABEL: Record<string, string> = { submit: '发起审批', approve: '审批通过', reject: '审批驳回', cancel: '撤回' }

const formatCurrency = (n: number) => `¥ ${n.toLocaleString('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`

/** 每张单据各自的意见输入与展开的历史 */
const comments = reactive<Record<string, string>>({})
const expanded = ref<string | null>(null)
const detail = ref<Approval | null>(null)
const busy = ref<string | null>(null)

async function toggleDetail(a: Approval) {
  if (expanded.value === a.id) {
    expanded.value = null
    return
  }
  expanded.value = a.id
  detail.value = await store.detail(a.id)
}

async function act(a: Approval, kind: 'approve' | 'reject' | 'cancel') {
  const comment = (comments[a.id] ?? '').trim()
  if (kind === 'reject' && !comment) {
    ElMessage.warning(t('驳回时请填写原因'))
    return
  }
  busy.value = a.id
  try {
    if (kind === 'cancel')
      await store.cancel(a.id, comment, operator.value)
    else
      await store.decide(a.id, kind === 'approve', comment, operator.value)
    ElMessage.success(kind === 'approve' ? '已通过' : kind === 'reject' ? '已驳回' : '已撤回')
    expanded.value = null
  }
  finally {
    busy.value = null
  }
}

function openWorkbook(a: Approval) {
  window.open(router.resolve({ name: 'ExcelWorkspace', query: { wb: a.workbookId } }).href, '_blank')
}

const TABS = computed<{ name: ApprovalBox, label: string, empty: string }[]>(() => [
  { name: 'todo', label: t('待我审批'), empty: '暂无待审批事项' },
  { name: 'started', label: t('我发起的'), empty: '你还没有发起过审批' },
  { name: 'done', label: t('已处理'), empty: '暂无已处理的审批' },
  ...(ledgerUser.authRequired && ledgerUser.isAdmin ? [{ name: 'all' as const, label: t('全部'), empty: '暂无审批单' }] : []),
])
</script>

<template>
  <el-drawer v-model="drawerVisible" :title="$t('审批中心')" direction="rtl" size="420px" :with-header="false">
    <div class="h-full flex flex-col">
      <div class="flex items-center justify-between border-b border-[#ebeef5] border-b-solid pb-2">
        <span class="text-base font-semibold">{{ $t('审批中心') }}</span>
        <el-tag size="small" round>
          {{ $t('{0} 待我处理', [store.pendingCount]) }}
        </el-tag>
      </div>

      <el-tabs v-model="activeTab" class="approval-tabs min-h-0 flex-1">
        <el-tab-pane v-for="t in TABS" :key="t.name" :name="t.name">
          <template #label>
            {{ t.label }}
            <el-badge v-if="t.name === 'todo'" :value="store.pendingCount" :hidden="store.pendingCount === 0" type="warning" />
          </template>

          <div v-loading="store.loading" class="py-2">
            <el-empty v-if="!store.loading && !store.items.length" :description="t.empty" :image-size="80" />
            <div v-for="a in store.items" :key="a.id" class="mb-2 border border-[#ebeef5] rounded-md border-solid bg-white p-3">
              <div class="mb-2 flex items-center justify-between">
                <button type="button" class="truncate border-0 bg-transparent p-0 text-left text-[13px] font-semibold cursor-pointer hover:text-[#0E5A31]" :title="$t('查看审批历史')" @click="toggleDetail(a)">
                  {{ a.workbookName }}
                </button>
                <el-tag size="small" :type="STATUS[a.status].type">
                  {{ STATUS[a.status].label }}
                </el-tag>
              </div>
              <div class="text-xs text-gray-500 leading-[1.8]">
                <div class="row">
                  <span>{{ $t('审批版本') }}</span>
                  <strong>
                    {{ $t('第 {0} 版', [a.workbookVersion]) }}
                    <span v-if="a.currentVersion && a.currentVersion > a.workbookVersion" class="text-[#B45309] font-normal" :title="$t('审批之后工作簿又被修改过')">{{ $t('（现在已到第 {0} 版）', [a.currentVersion]) }}</span>
                  </strong>
                </div>
                <template v-if="a.summary?.dataRows !== undefined">
                  <div class="row">
                    <span>{{ $t('数据行数') }}</span><strong>{{ $t('{0} 行', [a.summary.dataRows]) }}</strong>
                  </div>
                  <div v-if="a.summary.totalAmount" class="row">
                    <span>{{ $t('总金额') }}</span><strong>{{ formatCurrency(a.summary.totalAmount) }}</strong>
                  </div>
                </template>
                <div class="row">
                  <span>{{ $t('发起人') }}</span><strong>{{ a.initiatorName }}</strong>
                </div>
                <div class="row">
                  <span>{{ $t('审批人') }}</span><strong>{{ a.approverName }}</strong>
                </div>
                <div class="row">
                  <span>{{ $t('发起时间') }}</span><strong>{{ a.createdAt }}</strong>
                </div>
                <div class="row">
                  <span>{{ $t('说明') }}</span><strong class="max-w-[220px] truncate" :title="a.comment">{{ a.comment }}</strong>
                </div>
                <div v-if="a.decidedAt" class="row">
                  <span>{{ STATUS[a.status].label }}</span>
                  <strong class="max-w-[240px] truncate" :title="a.decisionComment || ''">{{ a.decidedBy }} · {{ a.decidedAt.slice(5, 16) }}<template v-if="a.decisionComment">「{{ a.decisionComment }}」</template></strong>
                </div>
              </div>

              <template v-if="a.status === 'pending' && (a.canDecide || a.canCancel)">
                <el-input v-model="comments[a.id]" :placeholder="$t('填写意见（驳回必填）')" size="small" class="my-2" />
                <div class="flex justify-end gap-2">
                  <el-button v-if="a.canCancel" size="small" :loading="busy === a.id" @click="act(a, 'cancel')">
                    {{ $t('撤回') }}
                  </el-button>
                  <el-button v-if="a.canDecide" size="small" type="danger" :loading="busy === a.id" @click="act(a, 'reject')">
                    {{ $t('驳回') }}
                  </el-button>
                  <el-button v-if="a.canDecide" size="small" type="success" :loading="busy === a.id" @click="act(a, 'approve')">
                    {{ $t('通过') }}
                  </el-button>
                </div>
              </template>

              <div class="mt-2 flex items-center justify-between">
                <el-button link size="small" @click="openWorkbook(a)">
                  {{ $t('打开工作簿') }}
                </el-button>
                <el-button link size="small" @click="toggleDetail(a)">
                  {{ expanded === a.id ? $t('收起历史') : $t('审批历史') }}
                </el-button>
              </div>
              <el-timeline v-if="expanded === a.id && detail?.id === a.id" class="mt-2 !pl-1">
                <el-timeline-item v-for="(e, i) in detail.events" :key="i" :timestamp="e.at.slice(5, 16)" placement="top" size="normal">
                  <div class="text-xs">
                    <strong>{{ e.userName }}</strong> {{ ACTION_LABEL[e.action] ?? e.action }}
                    <span v-if="e.comment" class="text-gray-500">：{{ e.comment }}</span>
                  </div>
                </el-timeline-item>
              </el-timeline>
            </div>
          </div>
        </el-tab-pane>
      </el-tabs>
    </div>
  </el-drawer>
</template>

<style lang="scss" scoped>
.row {
  @apply flex justify-between;

  strong {
    @apply font-medium text-gray-800;
  }
}

.approval-tabs {
  :deep(.#{$namespace}-tabs__content) {
    @apply overflow-y-auto;

    height: calc(100% - 40px);
  }
}
</style>
