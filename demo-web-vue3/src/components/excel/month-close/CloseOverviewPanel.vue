<script setup lang="ts">
import type { LedgerUserItem } from '@/api/approval'
import type { LedgerCloseAction, LedgerCloseShop } from '@/api/ledger'
import { computed, reactive, ref, watch } from 'vue'
import { listLedgerUsersApi } from '@/api/approval'
import { getLedgerCloseApi, ledgerCloseActionApi } from '@/api/ledger'
import { formatMoney } from '@/components/excel/ledger-detail/columns'
import { CHECK_LEVEL, CLOSE_STATUS } from '@/features/excel/shop-ledger/month-close'
import { useLedgerUserStore } from '@/stores/ledger-user'

const props = defineProps<{
  period: string
  operator: string
}>()

const emit = defineEmits<{
  /** 月结状态变化（锁账 / 解锁） */
  (e: 'changed'): void
  /** 跳转到余额 / 提现页签处理问题 */
  (e: 'goto', tab: 'balance' | 'withdraw', shop: string): void
}>()

const ledgerUser = useLedgerUserStore()
const shops = ref<LedgerCloseShop[]>([])
const loading = ref(false)

async function load() {
  if (!props.period)
    return
  loading.value = true
  try {
    shops.value = (await getLedgerCloseApi(props.period)).shops
  }
  finally {
    loading.value = false
  }
}

watch(() => props.period, load, { immediate: true })
defineExpose({ load })

const counts = computed(() => {
  const r = { locked: 0, pending: 0, open: 0 }
  for (const s of shops.value)
    r[s.status === 'locked' ? 'locked' : s.status === 'pending' ? 'pending' : 'open']++
  return r
})

const failCount = (s: LedgerCloseShop) => s.report.checks.filter(c => c.level === 'fail').length

// ── 操作对话框 ──
const dialog = reactive({
  visible: false,
  action: 'submit' as LedgerCloseAction,
  shop: null as LedgerCloseShop | null,
  /** 审批人：开启登录校验后从登录过的用户里选（id），否则填名字 */
  approverId: '',
  approverName: '',
  comment: '',
  acceptRisk: false,
  submitting: false,
})

const DIALOG_TITLE: Record<LedgerCloseAction, string> = {
  submit: '提交月结审批',
  approve: '审批通过并锁账',
  reject: '驳回月结',
  cancel: '撤回审批',
  reopen: '反结账（解除锁定）',
}

const candidates = ref<LedgerUserItem[]>([])
const searching = ref(false)

async function searchUsers(keyword = '') {
  searching.value = true
  try {
    candidates.value = (await listLedgerUsersApi(keyword)).filter(u => u.id !== ledgerUser.user?.id)
  }
  finally {
    searching.value = false
  }
}

function open(action: LedgerCloseAction, shop: LedgerCloseShop) {
  Object.assign(dialog, { visible: true, action, shop, comment: '', acceptRisk: false, approverId: '', approverName: '' })
  if (action === 'submit' && ledgerUser.authRequired)
    searchUsers()
}

/** 操作人就是登录用户（服务端也按登录账号记录） */
const actingUser = computed(() => props.operator)

const commentRequired = computed(() =>
  dialog.action === 'reject' || dialog.action === 'reopen' || (dialog.action === 'submit' && !!dialog.shop && failCount(dialog.shop) > 0),
)

const canConfirm = computed(() => {
  if (!actingUser.value.trim())
    return false
  if (dialog.action === 'submit' && (ledgerUser.authRequired ? !dialog.approverId : !dialog.approverName.trim()))
    return false
  if (commentRequired.value && !dialog.comment.trim())
    return false
  if (dialog.action === 'submit' && dialog.shop && failCount(dialog.shop) > 0 && !dialog.acceptRisk)
    return false
  return true
})

async function confirm() {
  const shop = dialog.shop
  if (!shop)
    return
  dialog.submitting = true
  try {
    await ledgerCloseActionApi({
      shop: shop.shop,
      period: props.period,
      action: dialog.action,
      operator: actingUser.value,
      approverId: dialog.action === 'submit' && dialog.approverId ? dialog.approverId : undefined,
      approver: dialog.action === 'submit' && dialog.approverName.trim() ? dialog.approverName.trim() : undefined,
      comment: dialog.comment,
      acceptRisk: dialog.acceptRisk,
    })
    ElMessage.success(`${shop.shop} ${props.period}：${DIALOG_TITLE[dialog.action]}成功`)
    dialog.visible = false
    await load()
    emit('changed')
  }
  finally {
    dialog.submitting = false
  }
}

function gotoFor(key: string): 'balance' | 'withdraw' | null {
  if (key === 'balance' || key === 'continuity')
    return 'balance'
  if (key === 'withdraw')
    return 'withdraw'
  return null
}
</script>

<template>
  <div v-loading="loading" class="flex flex-col gap-3">
    <div class="flex items-center gap-3 text-[13px]">
      <el-tag type="success" effect="plain">
        已锁账 {{ counts.locked }}
      </el-tag>
      <el-tag type="warning" effect="plain">
        审批中 {{ counts.pending }}
      </el-tag>
      <el-tag type="info" effect="plain">
        未结账 {{ counts.open }}
      </el-tag>
      <span class="text-xs text-gray-400">
        体检全部通过才能直接提交；有不通过项时需填写差异说明后「带差异提交」。提交后数据冻结（禁止导入、删除、改余额、改配对），审批通过即锁账
      </span>
    </div>

    <el-empty v-if="!loading && !shops.length" description="该月没有需要结账的店铺" :image-size="80" />

    <div v-for="s in shops" :key="s.shop" class="shop-card">
      <div class="flex items-center gap-2">
        <span class="text-[15px] font-semibold text-gray-800">{{ s.shop }}</span>
        <el-tag :type="CLOSE_STATUS[s.status].type" size="small">
          {{ CLOSE_STATUS[s.status].label }}
        </el-tag>
        <el-tag v-if="s.shopStatus === 'closed'" size="small" type="info">
          已关店
        </el-tag>
        <span class="text-xs text-gray-400">
          {{ s.report.figures.rowCount.toLocaleString() }} 条流水 · 入账 {{ formatMoney(s.report.figures.inflow) }} · 出账 {{ formatMoney(s.report.figures.outflow) }}
          · 提现 {{ s.report.figures.withdrawCount }} 笔 {{ formatMoney(s.report.figures.withdrawAmount) }}
        </span>
        <div class="ml-auto flex gap-2">
          <template v-if="s.status === 'open' || s.status === 'rejected'">
            <el-button size="small" :type="failCount(s) ? 'warning' : 'primary'" @click="open('submit', s)">
              {{ failCount(s) ? '带差异提交' : '提交月结' }}
            </el-button>
          </template>
          <template v-else-if="s.status === 'pending'">
            <el-button v-if="s.can.cancel" size="small" @click="open('cancel', s)">
              撤回
            </el-button>
            <template v-if="s.can.decide">
              <el-button size="small" type="danger" plain @click="open('reject', s)">
                驳回
              </el-button>
              <el-button size="small" type="success" @click="open('approve', s)">
                审批通过
              </el-button>
            </template>
            <span v-else-if="s.can.decideBlocked" class="self-center text-xs text-gray-400">{{ s.can.decideBlocked }}</span>
          </template>
          <el-button v-else-if="s.can.reopen" size="small" type="danger" plain @click="open('reopen', s)">
            反结账
          </el-button>
          <span v-else class="self-center text-xs text-gray-400">反结账需要审批人或管理员</span>
        </div>
      </div>

      <div class="mt-2 grid grid-cols-[1fr_auto] gap-x-6 gap-y-1.5">
        <template v-for="c in s.report.checks" :key="c.key">
          <div class="flex items-start gap-2 text-[13px] leading-5">
            <span class="lv" :class="CHECK_LEVEL[c.level].cls">{{ CHECK_LEVEL[c.level].icon }}</span>
            <span class="w-20 shrink-0 text-gray-500">{{ c.label }}</span>
            <span :class="{ 'text-[#B91C1C]': c.level === 'fail', 'text-[#92400E]': c.level === 'warn', 'text-gray-700': c.level === 'pass' }">
              {{ c.message }}
            </span>
          </div>
          <div>
            <el-button
              v-if="c.level !== 'pass' && gotoFor(c.key)"
              link
              type="primary"
              size="small"
              @click="emit('goto', gotoFor(c.key)!, s.shop)"
            >
              去处理 →
            </el-button>
          </div>
        </template>
      </div>

      <!-- 审批单 -->
      <div v-if="s.record && s.status !== 'open'" class="record">
        <div>
          <span class="text-gray-500">提交：</span>{{ s.record.submittedBy }} · {{ s.record.submittedAt }}
          <span class="ml-3 text-gray-500">审批人：</span>{{ s.record.approver }}
        </div>
        <div v-if="s.record.comment">
          <span class="text-gray-500">说明：</span>{{ s.record.comment }}
        </div>
        <div v-if="s.record.decidedAt">
          <span class="text-gray-500">{{ s.status === 'locked' ? '通过' : '驳回' }}：</span>{{ s.record.decidedBy }} · {{ s.record.decidedAt }}
          <span v-if="s.record.decisionComment">「{{ s.record.decisionComment }}」</span>
        </div>
      </div>
    </div>

    <el-dialog v-model="dialog.visible" :title="DIALOG_TITLE[dialog.action]" width="520px" append-to-body>
      <div v-if="dialog.shop" class="flex flex-col gap-3 text-[13px]">
        <div class="font-medium text-gray-800">
          {{ dialog.shop.shop }} · {{ period }}
        </div>

        <template v-if="dialog.action === 'submit'">
          <div class="flex flex-col gap-1">
            <div v-for="c in dialog.shop.report.checks" :key="c.key" class="flex gap-2">
              <span class="lv" :class="CHECK_LEVEL[c.level].cls">{{ CHECK_LEVEL[c.level].icon }}</span>
              <span>{{ c.label }}：{{ c.message }}</span>
            </div>
          </div>
          <el-form label-width="72px" size="small">
            <el-form-item label="审批人">
              <el-select
                v-if="ledgerUser.authRequired"
                v-model="dialog.approverId"
                filterable
                remote
                :remote-method="searchUsers"
                :loading="searching"
                placeholder="输入姓名搜索（需登录过本系统）"
                class="w-full"
                no-data-text="没有找到：对方需要先登录过本系统"
              >
                <el-option v-for="u in candidates" :key="u.id" :label="u.name" :value="u.id" />
              </el-select>
              <el-input v-else v-model="dialog.approverName" placeholder="审批人姓名" />
            </el-form-item>
          </el-form>
          <el-checkbox v-if="failCount(dialog.shop)" v-model="dialog.acceptRisk">
            体检有 {{ failCount(dialog.shop) }} 项不通过，我已核实原因，带差异提交
          </el-checkbox>
        </template>

        <div v-if="dialog.action === 'approve' || dialog.action === 'reject'" class="text-xs text-gray-500">
          以「{{ actingUser }}」的身份处理，记入月结日志。通过后该店铺 {{ period }} 锁账，不能再导入或修改。
        </div>
        <div v-if="dialog.action === 'reopen'" class="text-xs text-[#B91C1C]">
          反结账会解除锁定，允许重新导入和修改数据，之后需要重新提交审批。操作将记录在日志中。
        </div>

        <el-input
          v-model="dialog.comment"
          type="textarea"
          :rows="3"
          :placeholder="commentRequired ? '必填：请说明原因 / 差异情况' : '备注（可选）'"
        />
        <div v-if="!actingUser.trim()" class="text-xs text-[#B91C1C]">
          {{ ledgerUser.authRequired ? '未取得登录账号，请刷新页面' : '请先在右上角填写操作人' }}
        </div>
      </div>
      <template #footer>
        <el-button @click="dialog.visible = false">
          取消
        </el-button>
        <el-button
          :type="dialog.action === 'reject' || dialog.action === 'reopen' ? 'danger' : 'primary'"
          :disabled="!canConfirm"
          :loading="dialog.submitting"
          @click="confirm"
        >
          确定
        </el-button>
      </template>
    </el-dialog>
  </div>
</template>

<style scoped lang="scss">
.shop-card {
  @apply rounded border border-solid border-[#e6e6e6] p-3;
}

.record {
  @apply mt-2 rounded bg-[#F7F8FA] px-3 py-2 text-xs leading-5 text-gray-700;
}

.lv {
  @apply h-5 w-5 shrink-0 inline-flex items-center justify-center rounded-full text-[11px] font-bold;
}

.lv-pass {
  @apply bg-[#E7F4EC] text-[#0B5A2E];
}

.lv-warn {
  @apply bg-[#FEF3C7] text-[#92400E];
}

.lv-fail {
  @apply bg-[#FEE2E2] text-[#B91C1C];
}
</style>
