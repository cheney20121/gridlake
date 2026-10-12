<script setup lang="ts">
import type { Connector, ConnectorRun } from '@/api/connector'
import type { DatasetInfo } from '@/api/dataset'
import { useIntervalFn } from '@vueuse/core'
import { computed, ref, watch } from 'vue'
import { connectorRunsApi, deleteConnectorApi, listConnectorsApi, runConnectorApi, updateConnectorApi } from '@/api/connector'
import { t } from '@/i18n'
import ConnectorEditDialog from './ConnectorEditDialog.vue'

/** 数据连接器管理：列表、立即同步、同步记录、启停、删除 */
const props = defineProps<{
  visible: boolean
  datasets: DatasetInfo[]
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 有同步完成、数据集有变化：刷新数据集列表 / 公式缓存 */
  (e: 'changed'): void
  /** 打开数据集 */
  (e: 'open', datasetId: string): void
  /** 最近一次同步失败的连接器数（顶部按钮红点） */
  (e: 'failures', count: number): void
}>()

const items = ref<Connector[]>([])
const inboxRoots = ref<string[]>([])
const loading = ref(false)
const editVisible = ref(false)
const editing = ref<Connector | null>(null)
const expanded = ref<string | null>(null)
const runs = ref<ConnectorRun[]>([])
const runsLoading = ref(false)

async function load(silent = false) {
  if (!silent)
    loading.value = true
  try {
    const before = new Map(items.value.map(c => [c.id, c.running]))
    const r = await listConnectorsApi()
    items.value = r.items
    inboxRoots.value = r.inboxRoots
    emit('failures', r.items.filter(c => c.enabled && c.lastRun?.status === 'failed').length)
    // 有同步刚结束：通知刷新数据集；展开中的记录一并刷新
    const finished = r.items.filter(c => before.get(c.id) && !c.running)
    if (finished.length) {
      emit('changed')
      for (const c of finished) {
        const run = c.lastRun
        if (run?.status === 'failed')
          ElMessage.error(t('「{0}」同步失败：{1}', [c.name, run.message ?? '']))
        else if (run)
          ElMessage.success(t('「{0}」同步完成：{1}', [c.name, summary(run)]))
      }
      if (expanded.value && finished.some(c => c.id === expanded.value))
        loadRuns(expanded.value)
    }
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, (v) => {
  if (v)
    load()
}, { immediate: true })

// 打开期间：有同步在进行时每 2 秒刷新，否则每 30 秒
const anyRunning = computed(() => items.value.some(c => c.running))
const { pause, resume } = useIntervalFn(() => load(true), () => anyRunning.value ? 2000 : 30000, { immediate: false })
watch(() => props.visible, v => v ? resume() : pause(), { immediate: true })

function openCreate() {
  editing.value = null
  editVisible.value = true
}

function openEdit(c: Connector) {
  editing.value = c
  editVisible.value = true
}

async function runNow(c: Connector) {
  await runConnectorApi(c.id)
  c.running = true
  ElMessage.info(t('「{0}」开始同步', [c.name]))
  load(true)
}

async function onSaved(id: string, run: boolean) {
  await load(true)
  emit('changed')
  const c = items.value.find(x => x.id === id)
  if (run && c)
    await runNow(c)
}

async function toggle(c: Connector) {
  const { source, target, schedule, name } = c
  await updateConnectorApi(c.id, { name, source, target, schedule, enabled: !c.enabled })
  ElMessage.success(c.enabled ? `已停用「${c.name}」` : `已启用「${c.name}」`)
  load(true)
}

async function remove(c: Connector) {
  const ok = await ElMessageBox.confirm(
    `删除连接器「${c.name}」及其同步记录？已同步进数据集${c.datasetName ? `「${c.datasetName}」` : ''}的数据会保留。`,
    '删除连接器',
    { type: 'warning', confirmButtonText: t('删除') },
  ).then(() => true, () => false)
  if (!ok)
    return
  await deleteConnectorApi(c.id)
  ElMessage.success(t('已删除'))
  if (expanded.value === c.id)
    expanded.value = null
  load(true)
}

async function loadRuns(id: string) {
  runsLoading.value = true
  try {
    runs.value = await connectorRunsApi(id, 30)
  }
  finally {
    runsLoading.value = false
  }
}

function toggleRuns(c: Connector) {
  if (expanded.value === c.id) {
    expanded.value = null
    return
  }
  expanded.value = c.id
  runs.value = []
  loadRuns(c.id)
}

function summary(r: ConnectorRun) {
  if (r.status === 'skipped')
    return r.message || '没有新数据'
  const parts = [`读取 ${r.rowsRead.toLocaleString()} 行`]
  if (r.inserted)
    parts.push(`新增 ${r.inserted.toLocaleString()}`)
  if (r.updated)
    parts.push(`更新 ${r.updated.toLocaleString()}`)
  if (r.replaced)
    parts.push(`替换 ${r.replaced.toLocaleString()}`)
  if (r.skipped)
    parts.push(`跳过 ${r.skipped.toLocaleString()}`)
  return parts.join('，')
}

function sourceText(c: Connector) {
  const s = c.source
  if (s.kind === 'folder')
    return `${s.path} / ${s.pattern}`
  if (s.kind === 'http')
    return `${s.method.toUpperCase()} ${s.url.replace(/\?.*$/, '')}`
  return `${{ mysql: 'MySQL', postgres: 'PostgreSQL', clickhouse: 'ClickHouse' }[s.engine]} ${s.host}:${s.port}/${s.database}`
}

function scheduleText(c: Connector) {
  const s = c.schedule
  if (s.kind === 'daily')
    return `每天 ${s.at}`
  if (s.kind === 'interval')
    return s.minutes % 60 === 0 ? `每 ${s.minutes / 60} 小时` : `每 ${s.minutes} 分钟`
  return '手动'
}

const MODE_LABEL = { replace: '全量替换', append: '增量追加', upsert: '增量更新' } as const
const STATUS: Record<ConnectorRun['status'], { text: string, cls: string }> = {
  running: { text: t('同步中'), cls: 'text-[#1D4ED8]' },
  success: { text: t('成功'), cls: 'text-[#0B5A2E]' },
  skipped: { text: t('无新数据'), cls: 'text-gray-500' },
  failed: { text: t('失败'), cls: 'text-[#B91C1C]' },
}
function duration(r: ConnectorRun) {
  if (!r.finishedAt)
    return ''
  const s = (new Date(r.finishedAt.replace(' ', 'T')).getTime() - new Date(r.startedAt.replace(' ', 'T')).getTime()) / 1000
  return s < 60 ? `${Math.max(0, Math.round(s))} 秒` : `${Math.round(s / 60)} 分钟`
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('数据连接')"
    width="1040px"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="mb-3 flex items-center gap-3 text-[13px]">
      <span class="flex-1 text-xs text-gray-500 leading-5">
        {{ $t('按计划把共享文件夹、业务数据库（MySQL、PostgreSQL、ClickHouse）、HTTP 接口里的数据自动同步进数据集；之后的透视、DSUMIFS、关联、月结照常使用。') }}
      </span>
      <el-button type="primary" @click="openCreate">
        {{ $t('+ 新建连接') }}
      </el-button>
    </div>

    <div v-loading="loading" class="min-h-40">
      <div v-if="!items.length && !loading" class="empty">
        <div class="text-[15px] text-gray-800 font-medium">
          {{ $t('还没有数据连接') }}
        </div>
        <div class="text-xs text-gray-500 leading-5">
          {{ $t('例：运营每天把抖店导出的资金流水放进共享目录，平台每天 6:00 自动收取；ERP 商品成本表每天全量同步；订单库每 10 分钟增量同步') }}
        </div>
        <el-button type="primary" class="mt-2" @click="openCreate">
          {{ $t('新建连接') }}
        </el-button>
      </div>

      <div v-for="c in items" :key="c.id" class="card" :class="{ 'opacity-60': !c.enabled }">
        <div class="flex items-center gap-3">
          <span class="kind">{{ c.kind === 'folder' ? $t('文件夹') : c.kind === 'http' ? 'API' : 'DB' }}</span>
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="text-gray-900 font-medium">{{ c.name }}</span>
              <el-tag v-if="!c.enabled" size="small" type="info">
                {{ $t('已停用') }}
              </el-tag>
              <span class="truncate text-xs text-gray-400" :title="sourceText(c)">{{ sourceText(c) }}</span>
            </div>
            <div class="mt-0.5 flex flex-wrap items-center gap-x-3 text-xs text-gray-500">
              <span>
                → <template v-if="c.datasetName">
                  <el-button link type="primary" size="small" class="!p-0 !text-xs" @click="emit('open', c.target.datasetId!)">{{ c.datasetName }}</el-button>
                </template>
                <template v-else-if="c.target.datasetId">
                  <span class="text-[#B91C1C]">{{ $t('目标数据集已删除') }}</span>
                </template>
                <template v-else>{{ $t('新建「{0}」（首次同步时创建）', [c.target.name]) }}</template>
              </span>
              <span>{{ MODE_LABEL[c.target.mode] }}</span>
              <span>{{ scheduleText(c) }}</span>
              <span v-if="c.nextRun && !c.running">{{ $t('下次 {0}', [c.nextRun.slice(5, 16)]) }}</span>
            </div>
          </div>
          <div class="w-72 text-right text-xs leading-5">
            <template v-if="c.running">
              <span class="text-[#1D4ED8]">{{ $t('同步中…') }}</span>
            </template>
            <template v-else-if="c.lastRun">
              <div>
                <span :class="STATUS[c.lastRun.status].cls">{{ STATUS[c.lastRun.status].text }}</span>
                <span class="ml-1 text-gray-400">{{ c.lastRun.startedAt.slice(5, 16) }}</span>
              </div>
              <div class="truncate text-gray-500" :title="c.lastRun.status === 'failed' ? c.lastRun.message ?? '' : summary(c.lastRun)">
                {{ c.lastRun.status === 'failed' ? c.lastRun.message : summary(c.lastRun) }}
              </div>
            </template>
            <span v-else class="text-gray-400">{{ $t('尚未同步') }}</span>
          </div>
          <div class="flex shrink-0 items-center">
            <el-button size="small" type="primary" plain :loading="c.running" @click="runNow(c)">
              {{ $t('立即同步') }}
            </el-button>
            <el-button link size="small" @click="toggleRuns(c)">
              {{ $t('记录') }}
            </el-button>
            <el-button link size="small" @click="openEdit(c)">
              {{ $t('修改') }}
            </el-button>
            <el-button link size="small" @click="toggle(c)">
              {{ c.enabled ? $t('停用') : $t('启用') }}
            </el-button>
            <el-button link size="small" type="danger" :disabled="c.running" @click="remove(c)">
              {{ $t('删除') }}
            </el-button>
          </div>
        </div>

        <div v-if="expanded === c.id" v-loading="runsLoading" class="mt-2 border-t border-[#eef0f3] border-t-solid pt-2">
          <el-table :data="runs" size="small" max-height="280" :empty-text="$t('暂无同步记录')">
            <el-table-column :label="$t('开始时间')" prop="startedAt" width="150" />
            <el-table-column :label="$t('方式')" width="56">
              <template #default="{ row }">
                {{ row.trigger === 'manual' ? $t('手动') : $t('定时') }}
              </template>
            </el-table-column>
            <el-table-column :label="$t('结果')" width="76">
              <template #default="{ row }">
                <span :class="STATUS[row.status as ConnectorRun['status']].cls">{{ STATUS[row.status as ConnectorRun['status']].text }}</span>
              </template>
            </el-table-column>
            <el-table-column :label="$t('耗时')" width="66">
              <template #default="{ row }">
                {{ duration(row) }}
              </template>
            </el-table-column>
            <el-table-column :label="$t('内容')" min-width="300" show-overflow-tooltip>
              <template #default="{ row }">
                <span v-if="row.status === 'failed'" class="text-[#B91C1C]">{{ row.message }}</span>
                <template v-else>
                  {{ summary(row) }}
                  <span v-if="row.files.length" class="text-gray-400">{{ $t('；文件 {0}', [row.files.join('、')]) }}</span>
                  <span v-if="row.message && row.status !== 'skipped'" class="text-[#92400E]">；{{ row.message }}</span>
                </template>
              </template>
            </el-table-column>
            <el-table-column :label="$t('增量位置')" min-width="150" show-overflow-tooltip>
              <template #default="{ row }">
                <span v-if="row.watermarkTo" class="text-gray-500">{{ row.watermarkTo }}</span>
              </template>
            </el-table-column>
          </el-table>
        </div>
      </div>
    </div>

    <ConnectorEditDialog
      v-model:visible="editVisible"
      :connector="editing"
      :datasets="datasets"
      :inbox-roots="inboxRoots"
      @saved="onSaved"
    />
  </el-dialog>
</template>

<style scoped lang="scss">
.card {
  @apply mb-2 rounded-md border border-[#e5e7eb] border-solid px-3 py-2.5 text-[13px];
}

.kind {
  @apply w-12 shrink-0 rounded bg-[#E7F4EC] py-1 text-center text-xs text-[#217346];
}

.empty {
  @apply flex flex-col items-center gap-2 rounded-md border border-dashed border-[#c8c8c8] px-6 py-10 text-center;
}
</style>
