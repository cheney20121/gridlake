<script setup lang="ts">
import type { HomeSection } from './components/HomeNav.vue'
import type { DatasetInfo } from '@/api/dataset'
import type { WorkbookMeta } from '@/api/workbook'
import { Collection, Connection, Cpu, Delete, Document, Lock, Plus, Refresh, Tickets, Timer, Upload, User } from '@element-plus/icons-vue'
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { getDatasetsApi, listDatasetTrashApi } from '@/api/dataset'
import { createWorkbookApi, listWorkbooksApi, starWorkbookApi } from '@/api/workbook'
import AccountDialog from '@/components/excel/account/AccountDialog.vue'
import AuditLogDialog from '@/components/excel/audit/AuditLogDialog.vue'
import DataScopeDialog from '@/components/excel/audit/DataScopeDialog.vue'
import ConnectorManagerDialog from '@/components/excel/connector/ConnectorManagerDialog.vue'
import DatasetImportDialog from '@/components/excel/dataset-import/DatasetImportDialog.vue'
import DatasetTrashDialog from '@/components/excel/dataset-import/DatasetTrashDialog.vue'
import DatasetUsageDialog from '@/components/excel/dataset-import/DatasetUsageDialog.vue'
import McpAccessDialog from '@/components/excel/mcp/McpAccessDialog.vue'
import ReportManagerDialog from '@/components/excel/report/ReportManagerDialog.vue'
import SqlWorkbench from '@/components/excel/sql/SqlWorkbench.vue'
import TemplateGalleryDialog from '@/components/excel/template/TemplateGalleryDialog.vue'
import { useBodyClass } from '@/composables/useBodyClass'
import { blankWorkbookJson } from '@/features/excel/persistence/workbook-session'
import { useLedgerOperator } from '@/features/excel/workspace/use-operator'
import { t } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'
import DatasetTable from './components/DatasetTable.vue'
import HomeCard from './components/HomeCard.vue'
import HomeHeader from './components/HomeHeader.vue'
import HomeNav from './components/HomeNav.vue'
import WorkbookTable from './components/WorkbookTable.vue'

/**
 * 首页：打开表格前先到这里——最近的工作簿、新建（空白 / 模板）、数据中心（数据集、SQL、连接）、自动化（定时推送、外部 AI 接入）。
 * 编辑器里不常用的平台功能从这里进入，编辑器专注于表格本身
 */
useBodyClass('app-excel')
const router = useRouter()
const operator = useLedgerOperator()
const ledgerUser = useLedgerUserStore()
const operatorName = () => operator.value.trim() || '匿名用户'

const section = ref<HomeSection>('recent')
const workbooks = ref<WorkbookMeta[]>([])
const datasets = ref<DatasetInfo[]>([])
const trashCount = ref(0)
const loading = ref(false)
const keyword = ref('')

/** 问候语：按时段 + 登录用户名 */
const greeting = computed(() => {
  const h = new Date().getHours()
  const hello = h < 6 ? '夜深了' : h < 12 ? '早上好' : h < 18 ? '下午好' : '晚上好'
  const name = ledgerUser.authRequired ? ledgerUser.user?.name : ''
  return name ? `${hello}，${name}` : hello
})

async function loadWorkbooks() {
  loading.value = true
  try {
    workbooks.value = await listWorkbooksApi()
  }
  finally {
    loading.value = false
  }
}

async function loadDatasets() {
  datasets.value = await getDatasetsApi().catch(() => [])
  // 回收站对数据范围受限的用户不开放（请求会被拒绝并弹出提示）
  trashCount.value = ledgerUser.restricted ? 0 : (await listDatasetTrashApi().catch(() => ({ days: 0, items: [] }))).items.length
}

onMounted(() => {
  loadWorkbooks()
  loadDatasets()
})

const visibleWorkbooks = computed(() => {
  const k = keyword.value.trim().toLowerCase()
  return workbooks.value
    .filter(w => section.value !== 'starred' || w.starred)
    .filter(w => !k || w.name.toLowerCase().includes(k))
    .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
})

/** 编辑器整页打开：Univer 与协作、保存等单例按页面生命周期初始化，不走前端路由切换 */
function goEditor(query: Record<string, string>) {
  window.location.assign(router.resolve({ name: 'ExcelWorkspace', query }).href)
}
const openWorkbook = (id: string) => goEditor({ wb: id })
const openDataset = (id: string) => goEditor({ ds: id })

const creating = ref(false)
async function createBlank() {
  const value = await ElMessageBox.prompt(t('工作簿名称'), t('新建空白工作簿'), { inputValue: t('未命名工作簿'), inputPattern: /\S/, inputErrorMessage: t('请输入名称') })
    .then(r => r.value, () => null)
  if (!value)
    return
  creating.value = true
  try {
    const name = value.trim()
    const meta = await createWorkbookApi(name, blankWorkbookJson(name), operatorName())
    openWorkbook(meta.id)
  }
  finally {
    creating.value = false
  }
}

async function toggleStar(w: WorkbookMeta) {
  await starWorkbookApi(w.id, !w.starred)
  w.starred = !w.starred
}

const templateVisible = ref(false)
const importVisible = ref(false)
const sqlVisible = ref(false)
const connectorVisible = ref(false)
const trashVisible = ref(false)
const reportVisible = ref(false)
const mcpVisible = ref(false)
const auditVisible = ref(false)
const accountVisible = ref(false)
const dataScopeVisible = ref(false)
const usageVisible = ref(false)
const usageDataset = ref<DatasetInfo | null>(null)
const connectorFailures = ref(0)

function showUsage(d: DatasetInfo) {
  usageDataset.value = d
  usageVisible.value = true
}

function onTemplateCreated(meta: WorkbookMeta, warnings: string[]) {
  if (warnings.length)
    ElMessage.warning({ message: warnings.join('；'), duration: 5000 })
  openWorkbook(meta.id)
}

function onPublishHint() {
  ElMessage.info(t('请先打开要发布的工作簿，在文件菜单中选「发布为模板」'))
}
</script>

<template>
  <div class="h-screen flex flex-col bg-[#F5F7F6] text-[13px] text-gray-800">
    <HomeHeader
      :user-name="ledgerUser.authRequired ? ledgerUser.user?.name : undefined"
      :is-admin="ledgerUser.user?.admin"
      :creating="creating"
      @template="templateVisible = true"
      @create="createBlank"
    />

    <div v-if="ledgerUser.restricted" class="shrink-0 border-b border-[#f3d9a4] border-b-solid bg-[#fff8e6] px-6 py-2 text-xs text-[#8a5a00]">
      {{ $t('你的数据范围被限制在店铺：') }}<b>{{ ledgerUser.scopeText }}</b>{{ $t('。读取、透视、公式、导出都只含这些店铺；导入数据、建视图、定时推送等会绕过过滤的功能不可用。需要调整请联系管理员。') }}
    </div>

    <div class="min-h-0 flex flex-1">
      <HomeNav v-model:section="section" :dataset-count="datasets.length" :failures="connectorFailures" @templates="templateVisible = true" />

      <main class="min-w-0 flex-1 overflow-y-auto px-10 py-7">
        <div class="mx-auto max-w-[1100px]">
          <!-- 最近 / 已加星标 -->
          <template v-if="section === 'recent' || section === 'starred'">
            <template v-if="section === 'recent'">
              <h1 class="greeting">
                {{ greeting }}
              </h1>
              <div class="grid grid-cols-[repeat(auto-fill,minmax(250px,1fr))] mb-8 mt-4 gap-4">
                <HomeCard :title="$t('空白工作簿')" :desc="$t('从一张空表开始')" :icon="Plus" tone="green" :disabled="creating" @click="createBlank" />
                <HomeCard :title="$t('从模板新建')" :desc="$t('资金台账、销售看板…')" :icon="Collection" tone="blue" @click="templateVisible = true" />
                <HomeCard :title="$t('导入数据')" :desc="$t('CSV / XLSX，支持千万行')" :icon="Upload" tone="amber" @click="importVisible = true" />
              </div>
            </template>
            <WorkbookTable
              v-model:keyword="keyword"
              :items="visibleWorkbooks"
              :loading="loading"
              :auth-required="ledgerUser.authRequired"
              :title="section === 'recent' ? $t('最近的工作簿') : $t('已加星标')"
              :empty-text="keyword ? $t('没有匹配的工作簿') : section === 'starred' ? $t('还没有加星标的工作簿') : $t('还没有工作簿，新建一个吧')"
              @open="openWorkbook"
              @star="toggleStar"
              @changed="(w, meta) => Object.assign(w, meta)"
            />
          </template>

          <!-- 数据中心 -->
          <template v-else-if="section === 'data'">
            <div class="grid grid-cols-[repeat(auto-fill,minmax(230px,1fr))] mb-8 gap-4">
              <HomeCard v-if="!ledgerUser.restricted" :title="$t('导入数据')" :desc="$t('上传 CSV / XLSX 或填链接，生成可分析的数据集')" :icon="Upload" tone="amber" @click="importVisible = true" />
              <HomeCard :title="$t('SQL 查询')" :desc="$t('跨数据集查询，结果可另存为新数据集')" :icon="Tickets" tone="blue" @click="sqlVisible = true" />
              <HomeCard v-if="!ledgerUser.restricted" :title="$t('数据连接')" :desc="$t('接入数据库、接口，按计划自动同步')" :icon="Connection" tone="green" :badge="connectorFailures ? $t('{0} 个同步失败', [connectorFailures]) : undefined" @click="connectorVisible = true" />
              <HomeCard v-if="!ledgerUser.restricted" :title="$t('回收站')" :desc="trashCount ? $t('{0} 个已删除的数据集', [trashCount]) : $t('删除的数据集可在此恢复')" :icon="Delete" tone="slate" @click="trashVisible = true" />
            </div>
            <DatasetTable :items="datasets" :auth-required="ledgerUser.authRequired" :restricted="ledgerUser.restricted" @open="openDataset" @usage="showUsage" />
          </template>

          <!-- 自动化 -->
          <template v-else>
            <h2 class="mb-4 mt-0 text-base text-gray-800 font-semibold">
              {{ $t('自动化') }}
            </h2>
            <div v-if="ledgerUser.restricted" class="mb-4 text-gray-400">
              {{ $t('你的数据范围受限，定时推送、数据同步等自动化功能需要完整的数据权限，请联系管理员。') }}
            </div>
            <div class="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-4">
              <HomeCard v-if="!ledgerUser.restricted" :title="$t('定时推送')" :desc="$t('把仪表盘按日 / 周 / 月发到飞书、企业微信、邮件')" :icon="Timer" tone="green" @click="reportVisible = true" />
              <HomeCard v-if="!ledgerUser.restricted" :title="$t('数据同步')" :desc="$t('数据连接按计划拉取，看板与公式自动更新')" :icon="Refresh" tone="blue" :badge="connectorFailures ? $t('{0} 个失败', [connectorFailures]) : undefined" @click="connectorVisible = true" />
              <HomeCard v-if="ledgerUser.isAdmin" :title="$t('外部 AI 接入（MCP）')" :desc="$t('让 Claude 等 AI 助手按令牌权限查询数据集')" :icon="Cpu" tone="violet" @click="mcpVisible = true" />
              <HomeCard v-if="ledgerUser.authRequired && ledgerUser.isAdmin" :title="$t('数据权限（行级）')" :desc="$t('按某一列的取值（如店铺、地区、部门）限制用户能看到的行；数据集按哪一列过滤')" :icon="Lock" tone="amber" @click="dataScopeVisible = true" />
              <HomeCard v-if="ledgerUser.mode === 'local' && ledgerUser.isAdmin" :title="$t('账号管理')" :desc="$t('新建账号、设为管理员、停用、重置密码')" :icon="User" tone="green" data-testid="home-accounts" @click="accountVisible = true" />
              <HomeCard v-if="ledgerUser.authRequired && ledgerUser.isAdmin" :title="$t('操作审计')" :desc="$t('谁在什么时候改了什么：写操作、导出，含被拒绝的')" :icon="Document" tone="slate" @click="auditVisible = true" />
            </div>
          </template>
        </div>
      </main>
    </div>

    <TemplateGalleryDialog
      v-model:visible="templateVisible"
      :datasets="datasets"
      :operator="operatorName()"
      @created="onTemplateCreated"
      @import-data="() => { templateVisible = false; importVisible = true }"
      @publish="onPublishHint"
      @open-workbook="openWorkbook"
    />
    <DatasetImportDialog v-model:visible="importVisible" :datasets="datasets" @imported="(id: string, open: boolean) => open ? openDataset(id) : loadDatasets()" />
    <SqlWorkbench
      v-model:visible="sqlVisible"
      :datasets="datasets"
      :operator="operatorName()"
      @datasets-changed="loadDatasets()"
      @open-dataset="openDataset"
    />
    <ConnectorManagerDialog
      v-model:visible="connectorVisible"
      :datasets="datasets"
      @changed="loadDatasets()"
      @failures="(n: number) => connectorFailures = n"
      @open="openDataset"
    />
    <DatasetTrashDialog v-model:visible="trashVisible" @changed="loadDatasets()" />
    <DatasetUsageDialog v-model:visible="usageVisible" :dataset="usageDataset" @open-workbook="openWorkbook" />
    <ReportManagerDialog v-model:visible="reportVisible" @open="(wb: string, sheet: string) => goEditor({ wb, sheet })" />
    <McpAccessDialog v-model:visible="mcpVisible" :datasets="datasets" :operator="operatorName()" />
    <AuditLogDialog v-model:visible="auditVisible" />
    <AccountDialog v-model:visible="accountVisible" />
    <DataScopeDialog v-model:visible="dataScopeVisible" />
  </div>
</template>

<style scoped lang="scss">
.greeting {
  @apply m-0 text-[22px] font-semibold text-gray-800;
}
</style>
