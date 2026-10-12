<script setup lang="ts">
import type { Approval } from '@/api/approval'
import type { NotificationItem } from '@/api/comment'
import type { LedgerPeriod } from '@/api/ledger'
import type { WorkbookFolder } from '@/api/workbook'
import { computed, onBeforeUnmount, ref } from 'vue'
import { listFoldersApi } from '@/api/workbook'
import UserMenu from '@/components/excel/account/UserMenu.vue'
import { collabSession } from '@/features/excel/collaboration/session'
import { collaborationSync } from '@/features/excel/collaboration/sync-engine'
import { persistenceManager } from '@/features/excel/persistence/manager'
import { useApprovalStore } from '@/stores/excel-approval'
import { useExcelUIStore } from '@/stores/excel-ui'
import { useLedgerUserStore } from '@/stores/ledger-user'
import NotificationBell from './NotificationBell.vue'

/**
 * 编辑器顶栏：首页入口、文件菜单（工作簿名）、命令面板入口、登录用户与数据范围、保存状态、协作头像、
 * 资金台账的月份与锁账、查看明细、AI 助手、审批。只负责展示与发出事件，业务由页面处理
 */
export type FileCommand = 'manage' | 'template' | 'publish' | 'import' | 'export' | 'snapshots' | 'save'
export type ApprovalCommand = 'start' | 'center'

defineProps<{
  homeHref: string
  /** 当前工作簿对登录用户是只读的 */
  readOnly: boolean
  /** Excel 导入 / 导出进行中 */
  xlsxBusy: boolean
  /** 当前工作簿是资金台账（含「汇总表」页签）：显示月份与锁账 */
  isLedgerWorkbook: boolean
  periods: LedgerPeriod[]
  currentPeriod: string
  currentLock: { total: number, locked: number, pending: number }
  /** 选中汇总表 / 透视表的格子时的下钻标题；没有则不显示「查看明细」 */
  detailTitle: string | null
  /** 当前工作簿正在进行的审批（有则不能再发起） */
  openApproval: Approval | null
}>()

const emit = defineEmits<{
  (e: 'fileCommand', command: FileCommand): void
  (e: 'openPalette'): void
  (e: 'save'): void
  (e: 'loadPeriod', period: string): void
  (e: 'openMonthClose'): void
  (e: 'openDetail'): void
  (e: 'approvalCommand', command: ApprovalCommand): void
  (e: 'openNotification', n: NotificationItem): void
}>()

const uiStore = useExcelUIStore()
const approvalStore = useApprovalStore()
const ledgerUser = useLedgerUserStore()

// 面包屑：首页 › 所在文件夹（逐级）› 工作簿名。位置来自工作簿真实所在的文件夹，没有保存到服务端或在根目录时不显示文件夹
const folderId = ref<string | null>(persistenceManager.getMeta()?.folderId ?? null)
const folders = ref<WorkbookFolder[]>([])
let foldersLoaded = false

async function loadFolders() {
  try {
    folders.value = await listFoldersApi()
    foldersLoaded = true
  }
  catch {
    folders.value = []
  }
}

const offMeta = persistenceManager.onChange(async (_state, meta) => {
  const id = meta?.folderId ?? null
  // 换了工作簿（或在管理对话框里移动了位置）：文件夹可能是新建的，重新取一次
  if (id !== folderId.value || (id && !foldersLoaded))
    folderId.value = id
  if (id && !folders.value.some(f => f.id === id))
    await loadFolders()
})
onBeforeUnmount(offMeta)
if (folderId.value)
  loadFolders()

const folderChain = computed(() => {
  const chain: WorkbookFolder[] = []
  let cur = folders.value.find(f => f.id === folderId.value)
  while (cur && chain.length < 8) {
    chain.unshift(cur)
    cur = folders.value.find(f => f.id === cur!.parentId)
  }
  return chain
})

/** 保存状态显示文本 */
const saveStatusText = computed(() => {
  switch (uiStore.saveStatus) {
    case 'dirty': return '有未保存的修改'
    case 'saving': return '保存中...'
    case 'saved': return uiStore.lastSavedAt ? `已保存 · ${new Date(uiStore.lastSavedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}` : '已保存'
    case 'error': return '保存失败'
    case 'conflict': return '保存冲突'
    case 'offline': return '未连接服务'
    default: return ''
  }
})
</script>

<template>
  <header class="excel-topbar">
    <div class="topbar-left">
      <a class="home-link" :href="homeHref" title="返回首页：最近的工作簿、数据中心、自动化">
        <span class="logo-mark">
          <svg viewBox="0 0 16 16" width="14" height="14">
            <rect x="2" y="2" width="12" height="12" rx="2" fill="#107C41" />
            <text x="8" y="11" text-anchor="middle" fill="white" font-size="9" font-weight="700">E</text>
          </svg>
        </span>
        <span class="logo-text">Web Excel</span>
      </a>
      <span class="divider" />
      <span class="breadcrumb">
        <a class="crumb" :href="homeHref" title="返回首页">首页</a>
        <template v-for="f in folderChain" :key="f.id">
          <span class="sep">›</span>
          <button type="button" class="crumb" :title="`文件夹「${f.name}」：打开 / 新建 / 管理工作簿`" @click="emit('fileCommand', 'manage')">
            {{ f.name }}
          </button>
        </template>
        <span class="sep">›</span>
        <el-dropdown trigger="click" @command="(c: FileCommand) => emit('fileCommand', c)">
          <strong class="workbook-name" title="文件：打开 / 新建 / 导入导出 / 版本历史">{{ uiStore.fileName }} ▾</strong>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item command="manage">
                打开 / 新建 / 管理工作簿…
              </el-dropdown-item>
              <el-dropdown-item command="template">
                从模板新建…
              </el-dropdown-item>
              <el-dropdown-item command="publish">
                发布为模板…
              </el-dropdown-item>
              <el-dropdown-item command="import" divided>
                导入 .xlsx（追加为新页签）
              </el-dropdown-item>
              <el-dropdown-item command="export">
                导出为 .xlsx（保留公式与格式）
              </el-dropdown-item>
              <el-dropdown-item command="snapshots" divided>
                版本历史
              </el-dropdown-item>
              <el-dropdown-item command="save">
                <span class="flex-1">保存</span><span class="ml-6 text-xs text-gray-400">Ctrl+S</span>
              </el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
      </span>
    </div>
    <button type="button" class="palette-entry" title="搜索功能、数据集、工作表、工作簿（Alt+Q）" @click="emit('openPalette')">
      <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.6"><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5 14 14" /></svg>
      <span class="min-w-0 flex-1 truncate text-left">搜索功能、数据…</span>
      <kbd>Alt+Q</kbd>
    </button>
    <div class="topbar-right">
      <UserMenu v-if="ledgerUser.authRequired && ledgerUser.name">
        <span class="whitespace-nowrap text-xs text-gray-500 hover:text-gray-800" :title="`登录账号：${ledgerUser.name}${ledgerUser.isAdmin ? '（管理员）' : ''}`">{{ ledgerUser.name }}</span>
      </UserMenu>
      <el-tag v-if="ledgerUser.restricted" size="small" type="warning" :title="`你的数据范围被限制在店铺：${ledgerUser.scopeText}。读取、透视、公式、导出都只含这些店铺；导入、建视图、定时推送等功能不可用`">
        数据范围：{{ ledgerUser.scopeText }}
      </el-tag>
      <el-tag v-if="readOnly" size="small" type="warning" title="这个工作簿是别人共享给你的只读版本，修改不会被保存；可另存为副本">
        只读
      </el-tag>
      <NotificationBell v-if="ledgerUser.authRequired" @open="n => emit('openNotification', n)" />
      <!-- 保存状态（有未保存修改时点击保存） -->
      <div class="status-indicator cursor-pointer" title="点击保存（Ctrl+S）" @click="emit('save')">
        <span v-if="xlsxBusy" class="i-svg-spinners-90-ring-with-bg text-xs" />
        <span v-else class="dot" :class="uiStore.saveStatus" />
        <span>{{ xlsxBusy ? 'Excel 处理中…' : saveStatusText }}</span>
      </div>

      <!-- 协作头像（点击打开实时协作） -->
      <div class="avatars cursor-pointer" title="实时协作" @click="uiStore.togglePanel('collaboration')">
        <span class="avatar current" :style="{ background: collabSession.self.value?.color ?? collaborationSync.getUserColor() }">
          {{ (collabSession.self.value?.name ?? collaborationSync.getUserName()).charAt(0) }}
        </span>
        <span
          v-for="c in uiStore.collaborators"
          :key="c.id"
          class="avatar"
          :style="{ background: c.color }"
          :title="c.name"
          data-peer-avatar
        >
          {{ c.name.charAt(0) }}
        </span>
      </div>

      <!-- 资金台账：只在含「汇总表」的工作簿显示 -->
      <template v-if="isLedgerWorkbook">
        <span class="divider" />
        <el-select
          v-if="periods.length"
          :model-value="currentPeriod"
          size="small"
          class="w-28 shrink-0"
          placeholder="月份"
          @change="(p: string) => emit('loadPeriod', p)"
        >
          <el-option v-for="p in periods" :key="p.period" :label="p.period" :value="p.period" />
        </el-select>
        <el-tooltip
          v-if="currentLock.locked || currentLock.pending"
          :content="`${currentPeriod}：${currentLock.locked} 个店铺已锁账，${currentLock.pending} 个审批中（共 ${currentLock.total} 个），冻结的店铺不能再导入或修改`"
          placement="bottom"
        >
          <el-tag
            size="small"
            class="shrink-0 cursor-pointer"
            :type="currentLock.locked === currentLock.total ? 'success' : 'warning'"
            @click="emit('openMonthClose')"
          >
            🔒 {{ currentLock.locked }}/{{ currentLock.total }}
          </el-tag>
        </el-tooltip>
      </template>

      <!-- 选中汇总表 / 透视表的格子时出现：带上下文查看明细 -->
      <button
        v-if="detailTitle"
        type="button"
        class="tool active"
        :title="`查看「${detailTitle}」的明细`"
        @click="emit('openDetail')"
      >
        查看明细
        <span class="max-w-28 truncate text-xs font-normal opacity-75">{{ detailTitle }}</span>
      </button>

      <span class="divider" />

      <button type="button" class="tool ai" :class="{ active: uiStore.activePanel === 'ai' }" title="AI 分析助手：用中文提问，查询数据并生成透视表、图表、公式建议" @click="uiStore.togglePanel('ai')">
        ✦ AI 助手
      </button>

      <el-dropdown trigger="click" @command="(c: ApprovalCommand) => emit('approvalCommand', c)">
        <el-badge :value="approvalStore.pendingCount" :hidden="approvalStore.pendingCount === 0" type="warning">
          <el-button size="small" type="primary">
            审批 ▾
          </el-button>
        </el-badge>
        <template #dropdown>
          <el-dropdown-menu>
            <el-dropdown-item command="start" :disabled="!!openApproval">
              {{ openApproval ? `审批中（${openApproval.approverName}）…` : '发起审批' }}
            </el-dropdown-item>
            <el-dropdown-item command="center">
              {{ approvalStore.pendingCount ? `审批中心（${approvalStore.pendingCount} 条待办）` : '审批中心' }}
            </el-dropdown-item>
          </el-dropdown-menu>
        </template>
      </el-dropdown>
    </div>
  </header>
</template>

<style lang="scss" scoped>
/* Top Bar */
.excel-topbar {
  height: 48px;
  background: white;
  border-bottom: 1px solid #e2e8f0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 16px;
  flex-shrink: 0;
  z-index: 50;
}

.topbar-left {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.home-link {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 2px 6px;
  margin-left: -6px;
  border-radius: 4px;
  text-decoration: none;

  &:hover {
    background: #eef2ef;
  }
}

.logo-mark {
  display: flex;
  align-items: center;
}

.logo-text {
  font-weight: 600;
  color: #107c41;
  font-size: 14px;
}

.divider {
  width: 1px;
  height: 20px;
  background: #e2e8f0;
  margin: 0 4px;
}

.breadcrumb {
  display: flex;
  align-items: center;
  gap: 4px;
  color: #64748b;
  font-size: 13px;

  strong {
    color: #0f172a;
    font-weight: 600;
  }

  .sep {
    color: #cbd5e1;
  }

  .crumb {
    @apply max-w-32 cursor-pointer truncate border-none bg-transparent p-0 text-[13px] text-slate-500 no-underline hover:text-[#107c41];
  }
}

.palette-entry {
  @apply mx-3 h-7 max-w-64 min-w-28 flex flex-1 shrink cursor-pointer items-center gap-1.5 border border-[#e3e5e4] rounded-md border-solid bg-[#f6f7f6] px-2.5 text-xs text-gray-400 hover:border-[#c9d3cd] hover:bg-white hover:text-gray-600;

  kbd {
    @apply shrink-0 rounded border border-[#e5e7eb] border-solid bg-white px-1 text-[10px] text-gray-400 font-sans;
  }
}

.topbar-right {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: nowrap;
  overflow-x: auto;
}

.status-indicator {
  display: flex;
  align-items: center;
  gap: 6px;
  color: #64748b;
  font-size: 12px;
  white-space: nowrap;

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #16a34a;

    &.saving { background: #f59e0b; }
    &.dirty { background: #f59e0b; }
    &.error { background: #ef4444; }
    &.conflict { background: #ef4444; }
    &.offline { background: #94a3b8; }
    &.idle { background: #94a3b8; }
  }
}

.avatars {
  display: flex;
}

.avatar {
  width: 26px;
  height: 26px;
  border-radius: 50%;
  border: 2px solid white;
  margin-left: -6px;
  display: grid;
  place-items: center;
  font-size: 11px;
  font-weight: 600;
  color: white;

  &:first-child {
    margin-left: 0;
  }
}

.avatar.current {
  border-color: var(--el-color-primary);
}

.workbook-name {
  @apply cursor-pointer rounded px-1 hover:bg-[#E7F4EC] hover:text-[#0E5A31];
}

.tool {
  @apply flex shrink-0 cursor-pointer items-center gap-1 whitespace-nowrap rounded border-none bg-transparent px-2.5 py-1 text-[13px] text-gray-700 hover:bg-[#F1F5F2] hover:text-[#0E5A31];

  &.active {
    @apply bg-[#E7F4EC] font-medium text-[#0E5A31];
  }

  &.ai {
    @apply text-[#6D28D9] hover:bg-[#F3EEFF] hover:text-[#5B21B6];

    &.active {
      @apply bg-[#EDE5FF] text-[#5B21B6];
    }
  }
}
</style>
