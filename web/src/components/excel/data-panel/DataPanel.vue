<script setup lang="ts">
import type { DatasetInfo } from '@/api/dataset'
import { computed, ref, watch } from 'vue'
import { t } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'
import { formatRowCount } from '@/utils/format-count'

/**
 * 左侧「数据」面板：工作簿与数据集的切换入口（替代顶部的数据集标签栏，数据集多了也能搜索），
 * 底部是导入数据与「数据中心」（SQL、连接、推送、外部 AI 接入、模板、回收站）。可收起为窄条，状态记在本地
 */
export type DataCenterCommand = 'sql' | 'connector' | 'report' | 'mcp' | 'audit' | 'datascope' | 'template' | 'trash' | 'anomaly' | 'import' | 'shops' | 'close'

const props = defineProps<{
  datasets: DatasetInfo[]
  /** 当前显示：工作簿或某个数据集 */
  view: 'workbook' | 'dataset'
  activeDatasetId: string | null
  workbookName: string
  connectorFailures: number
  trashCount: number
  /** 台账工作簿：台账入口在功能区「台账」页签，这里不再列出 */
  isLedgerWorkbook: boolean
}>()

const emit = defineEmits<{
  (e: 'openWorkbook'): void
  (e: 'openDataset', id: string): void
  (e: 'datasetCommand', dataset: DatasetInfo, command: 'rename' | 'delete' | 'usage'): void
  (e: 'importData'): void
  (e: 'newRecon'): void
  (e: 'dataCenter', command: DataCenterCommand): void
}>()

const ledgerUser = useLedgerUserStore()
/** 改名 / 删除：只有所有者与管理员（内置数据集不可改） */
const canManage = (d: DatasetInfo) => !d.builtin && (!ledgerUser.authRequired || ledgerUser.isAdmin || (!!d.ownerId && d.ownerId === ledgerUser.user?.id))

const STORAGE_KEY = 'excel.dataPanel.collapsed'
const collapsed = ref(localStorage.getItem(STORAGE_KEY) === '1')
watch(collapsed, v => localStorage.setItem(STORAGE_KEY, v ? '1' : '0'))

const keyword = ref('')
const KIND_ORDER: Record<DatasetInfo['kind'], number> = { builtin: 0, table: 1, view: 2, summary: 3, recon: 4 }
const groups = computed(() => {
  const k = keyword.value.trim().toLowerCase()
  const list = k
    ? props.datasets.filter(d => d.name.toLowerCase().includes(k) || d.columns.some(c => c.title.toLowerCase().includes(k)))
    : props.datasets
  const sorted = [...list].sort((a, b) => KIND_ORDER[a.kind] - KIND_ORDER[b.kind])
  return [
    { key: 'table', title: t('数据集'), items: sorted.filter(d => d.kind === 'builtin' || d.kind === 'table') },
    { key: 'view', title: t('关联视图'), items: sorted.filter(d => d.kind === 'view') },
    { key: 'summary', title: t('汇总（透视转成）'), items: sorted.filter(d => d.kind === 'summary') },
    { key: 'recon', title: t('对账'), items: sorted.filter(d => d.kind === 'recon') },
  ].filter(g => g.items.length)
})

const rows = formatRowCount
</script>

<template>
  <aside v-if="collapsed" class="data-panel w-8 flex shrink-0 flex-col items-center gap-2 border-r border-[#e6e6e6] border-r-solid bg-[#FAFAFA] py-2">
    <button type="button" class="rail-btn" :title="$t('展开数据面板')" @click="collapsed = false">
      »
    </button>
    <button type="button" class="rail-btn [writing-mode:vertical-rl]" :class="{ active: view === 'workbook' }" :title="$t('工作簿')" @click="emit('openWorkbook')">
      {{ $t('工作簿') }}
    </button>
    <button type="button" class="rail-btn [writing-mode:vertical-rl]" :class="{ active: view === 'dataset' }" :title="$t('数据集 {0} 个', [datasets.length])" @click="collapsed = false">
      {{ $t('数据 {0}', [datasets.length]) }}
    </button>
  </aside>

  <aside v-else class="data-panel w-56 flex shrink-0 flex-col border-r border-[#e6e6e6] border-r-solid bg-[#FAFAFA] text-[13px]">
    <div class="flex items-center justify-between px-3 pb-1 pt-2">
      <span class="text-xs text-gray-500 font-semibold">{{ $t('数据') }}</span>
      <button type="button" class="rail-btn" :title="$t('收起')" @click="collapsed = true">
        «
      </button>
    </div>

    <button type="button" class="item mx-2" :class="{ active: view === 'workbook' }" :title="workbookName" @click="emit('openWorkbook')">
      <svg viewBox="0 0 16 16" width="14" height="14" class="shrink-0"><rect x="2" y="2" width="12" height="12" rx="2" fill="#107C41" /><path d="M2 6h12M2 10h12M6 6v8" stroke="#fff" stroke-width="1" /></svg>
      <span class="min-w-0 flex-1 truncate text-left">{{ workbookName || $t('工作簿') }}</span>
    </button>

    <div v-if="datasets.length > 6" class="px-2 pt-2">
      <el-input v-model="keyword" size="small" clearable :placeholder="$t('搜索数据集 / 列名')" />
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
      <template v-for="g in groups" :key="g.key">
        <div class="px-2 pb-1 pt-3 text-[11px] text-gray-400">
          {{ g.title }} {{ g.items.length }}
        </div>
        <div
          v-for="d in g.items"
          :key="d.id"
          class="item group"
          :class="{ active: view === 'dataset' && activeDatasetId === d.id }"
          :title="$t('{0}：{1} 行，{2} 列', [d.name, d.rowCount.toLocaleString(), d.columns.length])"
          @click="emit('openDataset', d.id)"
        >
          <span v-if="d.kind === 'view'" class="w-3.5 shrink-0 text-center text-xs text-[#3b7dd8]">⟕</span>
          <span v-else-if="d.kind === 'recon'" class="w-3.5 shrink-0 text-center text-xs text-[#107C41]">⇄</span>
          <span v-else-if="d.kind === 'summary'" class="w-3.5 shrink-0 text-center text-xs text-[#7c3aed]" :title="$t('由透视表转成的汇总数据集，随源数据实时变化')">Σ</span>
          <svg v-else viewBox="0 0 16 16" width="14" height="14" class="shrink-0" :class="d.kind === 'builtin' ? 'text-[#107C41]' : 'text-gray-500'" fill="none" stroke="currentColor" stroke-width="1.3"><ellipse cx="8" cy="4" rx="5" ry="2" /><path d="M3 4v8c0 1.1 2.2 2 5 2s5-.9 5-2V4M3 8c0 1.1 2.2 2 5 2s5-.9 5-2" /></svg>
          <span class="min-w-0 flex-1 truncate">{{ d.name }}</span>
          <span class="shrink-0 text-[11px] text-gray-400 tabular-nums" :class="{ 'group-hover:hidden': !ledgerUser.restricted }">{{ rows(d.rowCount) }}</span>
          <el-dropdown v-if="!ledgerUser.restricted" trigger="click" @command="(c: 'rename' | 'delete' | 'usage') => emit('datasetCommand', d, c)" @click.stop>
            <span class="more hidden group-hover:inline-block" @click.stop>⋯</span>
            <template #dropdown>
              <el-dropdown-menu>
                <el-dropdown-item v-if="!ledgerUser.restricted" command="usage">
                  {{ $t('使用情况') }}
                </el-dropdown-item>
                <el-dropdown-item v-if="canManage(d)" command="rename">
                  {{ $t('重命名') }}
                </el-dropdown-item>
                <el-dropdown-item v-if="canManage(d)" command="delete" divided>
                  <span class="text-[#B91C1C]">{{ $t('删除数据集') }}</span>
                </el-dropdown-item>
              </el-dropdown-menu>
            </template>
          </el-dropdown>
        </div>
      </template>
      <div v-if="keyword && !groups.length" class="px-2 py-4 text-center text-xs text-gray-400">
        {{ $t('没有匹配的数据集') }}
      </div>
    </div>

    <div class="flex flex-col gap-0.5 border-t border-[#e6e6e6] border-t-solid p-2">
      <button v-if="!ledgerUser.restricted" type="button" class="item text-[#217346]" :title="$t('导入任意 CSV / XLSX 为数据集（支持千万行）')" @click="emit('importData')">
        <span class="w-3.5 shrink-0 text-center">+</span>
        {{ $t('导入数据') }}
      </button>
      <button v-if="!ledgerUser.restricted" type="button" class="item text-[#217346]" :title="$t('两个数据集逐笔勾对（银行流水与企业账、往来对账等），得到未达账项')" @click="emit('newRecon')">
        <span class="w-3.5 shrink-0 text-center">⇄</span>
        {{ $t('新建对账') }}
      </button>
      <el-dropdown trigger="click" placement="top-start" class="w-full" @command="(c: DataCenterCommand) => emit('dataCenter', c)">
        <button type="button" class="item relative w-full" :title="$t('SQL 查询、数据连接、定时推送、外部 AI 接入、模板')">
          <span class="w-3.5 shrink-0 text-center text-gray-500">⊞</span>
          <span class="flex-1 text-left">{{ $t('数据中心') }}</span>
          <span v-if="connectorFailures" class="h-1.5 w-1.5 rounded-full bg-[#DC2626]" :title="$t('{0} 个连接最近一次同步失败', [connectorFailures])" />
          <span class="text-xs text-gray-400">▴</span>
        </button>
        <template #dropdown>
          <el-dropdown-menu>
            <el-dropdown-item command="sql">
              {{ $t('SQL 查询') }}
            </el-dropdown-item>
            <el-dropdown-item command="anomaly">
              {{ $t('异常发现') }}
            </el-dropdown-item>
            <el-dropdown-item v-if="!ledgerUser.restricted" command="connector">
              {{ $t('数据连接') }}<span v-if="connectorFailures" class="ml-1 text-xs text-[#DC2626]">{{ $t('{0} 个同步失败', [connectorFailures]) }}</span>
            </el-dropdown-item>
            <el-dropdown-item v-if="!ledgerUser.restricted" command="report">
              {{ $t('定时推送') }}
            </el-dropdown-item>
            <el-dropdown-item v-if="ledgerUser.isAdmin" command="mcp">
              {{ $t('外部 AI 接入（MCP）') }}
            </el-dropdown-item>
            <el-dropdown-item v-if="ledgerUser.authRequired && ledgerUser.isAdmin" command="audit">
              {{ $t('操作审计') }}
            </el-dropdown-item>
            <el-dropdown-item v-if="ledgerUser.authRequired && ledgerUser.isAdmin" command="datascope">
              {{ $t('数据权限（行级）') }}
            </el-dropdown-item>
            <el-dropdown-item command="template">
              {{ $t('模板库') }}
            </el-dropdown-item>
            <el-dropdown-item v-if="trashCount && !ledgerUser.restricted" command="trash">
              <span class="text-gray-500">{{ $t('数据集回收站（{0}）', [trashCount]) }}</span>
            </el-dropdown-item>
            <template v-if="ledgerUser.ecommerce && !isLedgerWorkbook">
              <li class="menu-group">
                {{ $t('店铺资金台账') }}
              </li>
              <el-dropdown-item v-if="!ledgerUser.restricted" command="import">
                {{ $t('导入店铺明细…') }}
              </el-dropdown-item>
              <el-dropdown-item v-if="!ledgerUser.restricted" command="shops">
                {{ $t('店铺档案') }}
              </el-dropdown-item>
              <el-dropdown-item command="close">
                {{ $t('月结中心') }}
              </el-dropdown-item>
            </template>
          </el-dropdown-menu>
        </template>
      </el-dropdown>
    </div>
  </aside>
</template>

<style scoped lang="scss">
.item {
  @apply flex cursor-pointer items-center gap-2 rounded border-none bg-transparent px-2 py-1.5 text-[13px] text-gray-700 hover:bg-[#EEF2EF];

  &.active {
    @apply bg-[#E7F4EC] font-medium text-[#0E5A31];
  }
}

.more {
  @apply rounded px-1 text-gray-400 hover:bg-gray-200 hover:text-gray-700;
}

.rail-btn {
  @apply cursor-pointer rounded border-none bg-transparent px-1 py-1 text-xs text-gray-500 hover:bg-[#EEF2EF] hover:text-gray-800;

  &.active {
    @apply bg-[#E7F4EC] text-[#0E5A31];
  }
}

.menu-group {
  @apply list-none border-t border-[#ebeef5] border-t-solid px-4 pb-1 pt-2 text-xs text-gray-400;
}
</style>
