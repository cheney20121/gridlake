<script setup lang="ts">
import type { LedgerCloseLog, LedgerPeriod, LedgerPeriodLock } from '@/api/ledger'
import { computed, ref, watch } from 'vue'
import { getLedgerCloseLogsApi, getLedgerLocksApi } from '@/api/ledger'
import { CLOSE_ACTION_LABEL, useLedgerOperator } from '@/features/excel/shop-ledger/month-close'
import { useLedgerUserStore } from '@/stores/ledger-user'
import BalancePanel from './BalancePanel.vue'
import CloseOverviewPanel from './CloseOverviewPanel.vue'
import WithdrawPanel from './WithdrawPanel.vue'

const props = defineProps<{
  visible: boolean
  /** 打开时默认的月份（当前汇总表月份） */
  period: string
  periods: LedgerPeriod[]
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 锁账状态变化，父组件刷新工具栏标识 */
  (e: 'locksChanged', locks: LedgerPeriodLock[]): void
}>()

type Tab = 'overview' | 'balance' | 'withdraw' | 'logs'

const operator = useLedgerOperator()
const ledgerUser = useLedgerUserStore()
const period = ref('')
const tab = ref<Tab>('overview')
const shopFilter = ref('')
const locks = ref<LedgerPeriodLock[]>([])
const logs = ref<LedgerCloseLog[]>([])

const overviewRef = ref<InstanceType<typeof CloseOverviewPanel>>()
const balanceRef = ref<InstanceType<typeof BalancePanel>>()
const withdrawRef = ref<InstanceType<typeof WithdrawPanel>>()

const frozenShops = computed(() => new Set(locks.value.filter(l => l.period === period.value).map(l => l.shop)))
const shopsOfPeriod = computed(() => props.periods.find(p => p.period === period.value)?.shops ?? [])

async function refreshLocks() {
  locks.value = await getLedgerLocksApi()
  emit('locksChanged', locks.value)
}

async function refreshLogs() {
  logs.value = await getLedgerCloseLogsApi(period.value)
}

watch(() => props.visible, (v) => {
  if (!v)
    return
  period.value = props.period || props.periods[0]?.period || ''
  tab.value = 'overview'
  shopFilter.value = ''
  refreshLocks()
})

watch([period, tab], () => {
  if (tab.value === 'logs' && period.value)
    refreshLogs()
})

/** 余额 / 配对变化后体检结果随之变化 */
function onDataChanged() {
  overviewRef.value?.load()
}

async function onCloseChanged() {
  await refreshLocks()
  balanceRef.value?.load()
  withdrawRef.value?.load()
}

function goto(target: 'balance' | 'withdraw', shop: string) {
  shopFilter.value = shop
  tab.value = target
}
</script>

<template>
  <el-drawer
    :model-value="visible"
    size="88%"
    class="month-close-drawer"
    @update:model-value="emit('update:visible', $event)"
  >
    <template #header>
      <div class="flex items-center gap-3">
        <span class="text-base font-semibold text-gray-800">月结中心</span>
        <el-select v-model="period" size="small" class="!w-28">
          <el-option v-for="p in periods" :key="p.period" :label="p.period" :value="p.period" />
        </el-select>
        <el-select v-if="tab === 'balance' || tab === 'withdraw'" v-model="shopFilter" size="small" clearable placeholder="全部店铺" class="!w-48">
          <el-option v-for="s in shopsOfPeriod" :key="s" :label="s" :value="s" />
        </el-select>
        <div class="ml-auto flex items-center gap-2 text-xs text-gray-500">
          操作人
          <span v-if="ledgerUser.authRequired" class="text-gray-800" title="已登录账号，操作记入日志">{{ operator }}</span>
          <el-input v-else v-model="operator" size="small" class="!w-32" placeholder="必填，记入日志" :class="{ 'need-operator': !operator.trim() }" />
        </div>
      </div>
    </template>

    <div class="h-full flex flex-col">
      <el-tabs v-model="tab">
        <el-tab-pane label="月结总览" name="overview" />
        <el-tab-pane label="余额勾稽" name="balance" />
        <el-tab-pane label="提现对账" name="withdraw" />
        <el-tab-pane label="操作日志" name="logs" />
      </el-tabs>
      <div class="min-h-0 flex-1 overflow-auto">
        <!-- v-show 保留各页签的编辑状态 -->
        <CloseOverviewPanel
          v-show="tab === 'overview'"
          ref="overviewRef"
          :period="period"
          :operator="operator"
          @changed="onCloseChanged"
          @goto="goto"
        />
        <BalancePanel
          v-show="tab === 'balance'"
          ref="balanceRef"
          :period="period"
          :operator="operator"
          :frozen-shops="frozenShops"
          :shop="shopFilter"
          @saved="onDataChanged"
        />
        <WithdrawPanel
          v-show="tab === 'withdraw'"
          ref="withdrawRef"
          :period="period"
          :operator="operator"
          :frozen-shops="frozenShops"
          :shop="shopFilter"
          @changed="onDataChanged"
        />
        <el-table v-if="tab === 'logs'" :data="logs" size="small" empty-text="暂无月结操作">
          <el-table-column label="时间" prop="at" width="170" />
          <el-table-column label="店铺" prop="shop" min-width="140" />
          <el-table-column label="操作" width="100">
            <template #default="{ row }">
              {{ CLOSE_ACTION_LABEL[row.action as keyof typeof CLOSE_ACTION_LABEL] ?? row.action }}
            </template>
          </el-table-column>
          <el-table-column label="操作人" prop="operator" width="120" />
          <el-table-column label="说明" prop="comment" min-width="300" show-overflow-tooltip />
        </el-table>
      </div>
    </div>
  </el-drawer>
</template>

<style scoped lang="scss">
.need-operator :deep(.#{$namespace}-input__wrapper) {
  box-shadow: 0 0 0 1px #f56c6c inset;
}
</style>
