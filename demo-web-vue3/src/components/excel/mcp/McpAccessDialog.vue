<script setup lang="ts">
import type { DatasetInfo } from '@/api/dataset'
import type { McpLog, McpToken } from '@/api/mcp'
import { computed, reactive, ref, watch } from 'vue'
import { createMcpTokenApi, listMcpTokensApi, MCP_ENDPOINT, mcpLogsApi, revokeMcpTokenApi, updateMcpTokenApi } from '@/api/mcp'

/** 外部 AI 接入（MCP）：令牌管理、各客户端的配置方法、调用记录 */
const props = defineProps<{
  visible: boolean
  datasets: DatasetInfo[]
  operator?: string
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
}>()

const tab = ref<'tokens' | 'guide' | 'logs'>('tokens')
const tokens = ref<McpToken[]>([])
const logs = ref<McpLog[]>([])
const loading = ref(false)
/** 刚创建的令牌明文（只在本次打开期间显示） */
const secret = ref('')
const secretName = ref('')
const client = ref<'cursor' | 'cherry' | 'claude' | 'bridge' | 'dify'>('cursor')

async function load() {
  loading.value = true
  try {
    tokens.value = await listMcpTokensApi()
    if (tab.value === 'logs')
      logs.value = await mcpLogsApi()
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, (v) => {
  if (v) {
    secret.value = ''
    load()
  }
}, { immediate: true })

watch(tab, async (t) => {
  if (t === 'logs')
    logs.value = await mcpLogsApi()
})

function datasetNames(ids: string[] | null) {
  if (!ids)
    return '全部数据集'
  const names = ids.map(id => props.datasets.find(d => d.id === id)?.name ?? '（已删除）')
  return names.length > 3 ? `${names.slice(0, 3).join('、')} 等 ${names.length} 个` : names.join('、')
}

// ── 新建 / 修改 ──

const editVisible = ref(false)
const editing = ref<McpToken | null>(null)
const saving = ref(false)
const form = reactive({ name: '', scope: 'all' as 'all' | 'some', datasets: [] as string[], allowSql: true })

function openEdit(t: McpToken | null) {
  editing.value = t
  Object.assign(form, { name: t?.name ?? '', scope: t?.datasets ? 'some' : 'all', datasets: t?.datasets ? [...t.datasets] : [], allowSql: t?.allowSql ?? true })
  editVisible.value = true
}

async function save() {
  if (!form.name.trim())
    return ElMessage.warning('请填写令牌名称，如「财务部 Cursor」')
  if (form.scope === 'some' && !form.datasets.length)
    return ElMessage.warning('请选择可访问的数据集')
  const input = { name: form.name.trim(), allowSql: form.allowSql, datasets: form.scope === 'all' ? null : form.datasets, operator: props.operator }
  saving.value = true
  try {
    if (editing.value) {
      await updateMcpTokenApi(editing.value.id, input)
      ElMessage.success('已保存，立即生效')
    }
    else {
      const r = await createMcpTokenApi(input)
      secret.value = r.secret
      secretName.value = r.token.name
      tab.value = 'tokens'
    }
    editVisible.value = false
    load()
  }
  finally {
    saving.value = false
  }
}

async function revoke(t: McpToken) {
  const ok = await ElMessageBox.confirm(`吊销令牌「${t.name}」？使用它的 AI 客户端将立即无法访问，此操作不能撤销。`, '吊销令牌', { type: 'warning', confirmButtonText: '吊销' }).then(() => true, () => false)
  if (!ok)
    return
  await revokeMcpTokenApi(t.id)
  ElMessage.success('已吊销')
  load()
}

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    ElMessage.success('已复制')
  }
  catch {
    ElMessage.warning('浏览器不允许自动复制，请手动选中复制')
  }
}

// ── 接入说明 ──

const tokenText = computed(() => secret.value || '<在这里填写令牌>')
/** mcp-remote 的环境变量引用写法（字面量，由 mcp-remote 展开） */
const AUTH_REF = `Authorization:$${'{'}AUTH_HEADER}`

const snippet = computed(() => {
  const ep = MCP_ENDPOINT
  const tk = tokenText.value
  switch (client.value) {
    case 'cursor':
      return JSON.stringify({ mcpServers: { webexcel: { url: ep, headers: { Authorization: `Bearer ${tk}` } } } }, null, 2)
    case 'claude':
      return JSON.stringify({ mcpServers: { webexcel: { command: 'npx', args: ['-y', 'mcp-remote', ep, '--header', AUTH_REF, '--allow-http'], env: { AUTH_HEADER: `Bearer ${tk}` } } } }, null, 2)
    case 'bridge':
      return JSON.stringify({ mcpServers: { webexcel: { command: '/opt/webexcel/bin/ledger-server', args: ['mcp-bridge'], env: { LEDGER_MCP_URL: ep, LEDGER_MCP_TOKEN: tk } } } }, null, 2)
    case 'cherry':
      return `类型：可流式传输的 HTTP（streamableHttp）\nURL：${ep}\n请求头：Authorization=Bearer ${tk}`
    default:
      return `MCP 服务地址（HTTP / Streamable HTTP）：${ep}\n请求头：Authorization: Bearer ${tk}`
  }
})

const CLIENT_HINT: Record<string, string> = {
  cursor: '编辑 ~/.cursor/mcp.json（或项目下 .cursor/mcp.json），粘贴后在 Cursor 设置的 MCP 页面启用',
  cherry: 'Cherry Studio → 设置 → MCP 服务器 → 添加服务器，按下方填写',
  claude: 'Claude Desktop → 设置 → 开发者 → 编辑配置（claude_desktop_config.json）；需要本机装有 Node.js（npx）。内网 HTTP 地址需要 --allow-http',
  bridge: 'macOS / Linux 上有 ledger-server 程序时可用内置转接（不需要 Node.js）；命令路径改成程序实际位置',
  dify: 'Dify：在「工具 → MCP」中添加 HTTP 类型的 MCP 服务；其他支持 MCP 的平台同样填写地址与请求头',
}
</script>

<template>
  <el-dialog :model-value="visible" title="外部 AI 接入（MCP）" width="980px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div class="mb-3 flex items-center gap-3 text-[13px]">
      <el-radio-group v-model="tab">
        <el-radio-button value="tokens">
          访问令牌 {{ tokens.length || '' }}
        </el-radio-button>
        <el-radio-button value="guide">
          接入方法
        </el-radio-button>
        <el-radio-button value="logs">
          调用记录
        </el-radio-button>
      </el-radio-group>
      <span class="flex-1 text-xs text-gray-500 leading-5">
        让 Claude、Cursor、Cherry Studio、Dify 等 AI 助手通过 MCP 协议<b>只读</b>查询数据集与仪表盘；必须持有令牌，可限定数据范围，每次调用都有记录
      </span>
      <el-button v-if="tab === 'tokens'" type="primary" @click="openEdit(null)">
        + 新建令牌
      </el-button>
    </div>

    <div v-if="secret" class="secret">
      <div class="mb-1 font-medium">
        令牌「{{ secretName }}」已创建，请立即复制保存——关闭后无法再次查看
      </div>
      <div class="flex items-center gap-2">
        <code class="min-w-0 flex-1 truncate">{{ secret }}</code>
        <el-button size="small" type="primary" @click="copy(secret)">
          复制令牌
        </el-button>
        <el-button size="small" @click="tab = 'guide'">
          查看接入方法
        </el-button>
      </div>
    </div>

    <div v-loading="loading" class="min-h-48">
      <template v-if="tab === 'tokens'">
        <div v-if="!tokens.length && !loading" class="empty">
          <div class="text-[15px] text-gray-800 font-medium">
            还没有访问令牌
          </div>
          <div class="text-xs text-gray-500 leading-5">
            没有令牌时 MCP 服务不可用。建议每个人 / 每个工具单独一个令牌，按需限定数据集，不用时随时吊销
          </div>
          <el-button type="primary" class="mt-2" @click="openEdit(null)">
            新建令牌
          </el-button>
        </div>
        <div v-for="t in tokens" :key="t.id" class="card">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="text-gray-900 font-medium">{{ t.name }}</span>
              <code class="text-xs text-gray-400">{{ t.prefix }}…</code>
              <el-tag size="small" :type="t.allowSql ? 'warning' : 'info'" effect="plain">
                {{ t.allowSql ? '允许 SQL' : '不允许 SQL' }}
              </el-tag>
            </div>
            <div class="mt-0.5 text-xs text-gray-500">
              可访问：{{ datasetNames(t.datasets) }}
              · 创建于 {{ t.createdAt.slice(0, 16) }}{{ t.createdBy ? `（${t.createdBy}）` : '' }}
              · {{ t.lastUsedAt ? `最近使用 ${t.lastUsedAt.slice(5, 16)}，共 ${t.calls} 次调用` : '尚未使用' }}
            </div>
          </div>
          <el-button link size="small" @click="openEdit(t)">
            修改
          </el-button>
          <el-button link size="small" type="danger" @click="revoke(t)">
            吊销
          </el-button>
        </div>
      </template>

      <template v-else-if="tab === 'guide'">
        <div class="mb-3 text-[13px]">
          <div class="mb-1 text-gray-600">
            MCP 服务地址
          </div>
          <div class="flex items-center gap-2">
            <code class="addr">{{ MCP_ENDPOINT }}</code>
            <el-button size="small" @click="copy(MCP_ENDPOINT)">
              复制
            </el-button>
          </div>
          <div class="mt-1 text-xs text-gray-500 leading-5">
            AI 客户端所在电脑必须能访问这个地址（内网部署时在内网使用；如经 nginx 转发，填写实际对外地址）。请求头带 <code>Authorization: Bearer 令牌</code>
          </div>
        </div>
        <el-radio-group v-model="client" size="small" class="mb-2">
          <el-radio-button value="cursor">
            Cursor
          </el-radio-button>
          <el-radio-button value="cherry">
            Cherry Studio
          </el-radio-button>
          <el-radio-button value="claude">
            Claude Desktop
          </el-radio-button>
          <el-radio-button value="bridge">
            内置转接
          </el-radio-button>
          <el-radio-button value="dify">
            Dify / 其他
          </el-radio-button>
        </el-radio-group>
        <div class="mb-1 text-xs text-gray-500">
          {{ CLIENT_HINT[client] }}
        </div>
        <div class="relative">
          <pre class="snippet">{{ snippet }}</pre>
          <el-button size="small" class="!absolute right-2 top-2" @click="copy(snippet)">
            复制
          </el-button>
        </div>
        <div v-if="!secret" class="mt-1 text-xs text-[#92400E]">
          把 &lt;在这里填写令牌&gt; 换成新建令牌时复制的令牌
        </div>
        <div class="mt-3 text-xs text-gray-500 leading-5">
          可用工具（全部只读）：数据集列表 / 结构 / 取值、汇总查询、两期对比归因、SQL 查询（令牌允许时）、仪表盘列表、读取仪表盘当前指标。
          可以这样问：「9 月各店铺销售额和 8 月比怎么样？」「读一下经营看板」「用 SQL 算各店铺月度 ROI」。
        </div>
      </template>

      <template v-else>
        <el-table :data="logs" size="small" max-height="460" empty-text="还没有调用记录">
          <el-table-column label="时间" prop="createdAt" width="150" />
          <el-table-column label="令牌" prop="tokenName" width="130" show-overflow-tooltip />
          <el-table-column label="调用" width="160">
            <template #default="{ row }">
              {{ (row as McpLog).tool ?? (row as McpLog).method }}
            </template>
          </el-table-column>
          <el-table-column label="结果" width="66">
            <template #default="{ row }">
              <span :class="(row as McpLog).ok ? 'text-[#0B5A2E]' : 'text-[#B91C1C]'">{{ (row as McpLog).ok ? '成功' : '失败' }}</span>
            </template>
          </el-table-column>
          <el-table-column label="耗时" width="70">
            <template #default="{ row }">
              {{ (row as McpLog).elapsedMs }} ms
            </template>
          </el-table-column>
          <el-table-column label="参数 / 原因" min-width="300" show-overflow-tooltip>
            <template #default="{ row }">
              <span v-if="!(row as McpLog).ok" class="text-[#B91C1C]">{{ (row as McpLog).message }}</span>
              <span v-else class="text-gray-500">{{ (row as McpLog).arguments }}</span>
            </template>
          </el-table-column>
          <el-table-column label="客户端" prop="client" width="140" show-overflow-tooltip />
        </el-table>
      </template>
    </div>

    <el-dialog v-model="editVisible" :title="editing ? `修改令牌 · ${editing.name}` : '新建访问令牌'" width="560px" append-to-body :close-on-click-modal="false">
      <el-form label-width="96px" @submit.prevent>
        <el-form-item label="名称" required>
          <el-input v-model="form.name" maxlength="40" placeholder="谁在用 / 用在哪，如「财务部 Cursor」" />
        </el-form-item>
        <el-form-item label="可访问数据">
          <div class="w-full">
            <el-radio-group v-model="form.scope">
              <el-radio value="all">
                全部数据集
              </el-radio>
              <el-radio value="some">
                指定数据集
              </el-radio>
            </el-radio-group>
            <el-select v-if="form.scope === 'some'" v-model="form.datasets" multiple filterable class="mt-1 w-full" placeholder="选择数据集">
              <el-option v-for="d in datasets" :key="d.id" :label="d.name" :value="d.id" />
            </el-select>
            <div class="mt-1 text-xs text-gray-500 leading-5">
              只能看到、查询所选数据集；仪表盘用到其他数据集时也不可读取
            </div>
          </div>
        </el-form-item>
        <el-form-item label="SQL 查询">
          <el-switch v-model="form.allowSql" active-text="允许" inactive-text="不允许" inline-prompt />
          <span class="ml-2 text-xs text-gray-500">只读 SQL，可做跨数据集关联等复杂计算</span>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="editVisible = false">
          取消
        </el-button>
        <el-button type="primary" :loading="saving" @click="save">
          {{ editing ? '保存' : '创建令牌' }}
        </el-button>
      </template>
    </el-dialog>
  </el-dialog>
</template>

<style scoped lang="scss">
.card {
  @apply mb-2 flex items-center gap-2 rounded-md border border-[#e5e7eb] border-solid px-3 py-2.5 text-[13px];
}

.empty {
  @apply flex flex-col items-center gap-2 rounded-md border border-dashed border-[#c8c8c8] px-6 py-10 text-center;
}

.secret {
  @apply mb-3 rounded-md border border-[#F59E0B] border-solid bg-[#FFF7E6] px-3 py-2 text-[13px] text-[#92400E];

  code {
    @apply rounded bg-white px-2 py-1 text-xs text-gray-800 font-mono;
  }
}

.addr {
  @apply min-w-0 flex-1 truncate rounded bg-[#f4f4f1] px-2 py-1 text-xs font-mono;
}

.snippet {
  @apply m-0 overflow-x-auto whitespace-pre rounded-md bg-[#f9f9f7] p-3 pr-16 text-xs leading-5 font-mono;
}
</style>
