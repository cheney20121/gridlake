<script setup lang="ts">
import type { AccessToken, CreatedAccessToken } from '@/api/auth'
import { ref, watch } from 'vue'
import { createAccessTokenApi, listAccessTokensApi, revokeAccessTokenApi } from '@/api/auth'
import { getDatasetsApi } from '@/api/dataset'
import { t } from '@/i18n'

/**
 * 个人访问令牌：给脚本、CI 调用接口用。权限与自己的账号相同；只在创建时显示一次完整令牌；可设有效期、随时吊销。
 * 令牌不能用来创建 / 吊销令牌（泄露后攻击者没法给自己再造一批）
 */
const visible = defineModel<boolean>('visible', { default: false })
const items = ref<AccessToken[]>([])
const loading = ref(false)
const name = ref('')
const days = ref(90)
const readOnly = ref(false)
/** 限定的数据集（id）；空表示不限 */
const limitedTo = ref<string[]>([])
const datasetOptions = ref<{ id: string, name: string }[]>([])
const creating = ref(false)
const error = ref('')
const created = ref<CreatedAccessToken | null>(null)

const DAYS = [
  { value: 30, label: t('30 天') },
  { value: 90, label: t('90 天') },
  { value: 365, label: t('1 年') },
  { value: 0, label: t('永不过期') },
]

async function load() {
  loading.value = true
  try {
    items.value = await listAccessTokensApi()
  }
  finally {
    loading.value = false
  }
}

watch(visible, (v) => {
  if (v) {
    name.value = ''
    days.value = 90
    readOnly.value = false
    limitedTo.value = []
    error.value = ''
    created.value = null
    load()
    getDatasetsApi().then((ds) => {
      datasetOptions.value = ds.map(d => ({ id: d.id, name: d.name }))
    }).catch(() => {})
  }
})

async function create() {
  error.value = ''
  if (!name.value.trim())
    return (error.value = t('请填写令牌名称，如「CI」「数据同步脚本」'))
  creating.value = true
  try {
    created.value = await createAccessTokenApi(name.value.trim(), days.value, readOnly.value, limitedTo.value)
    name.value = ''
    limitedTo.value = []
    await load()
  }
  catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  }
  finally {
    creating.value = false
  }
}

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    ElMessage.success(t('已复制'))
  }
  catch {
    ElMessage.warning(t('浏览器不允许自动复制，请手动选中复制'))
  }
}

async function revoke(it: AccessToken) {
  const ok = await ElMessageBox.confirm(t('吊销令牌「{0}」？使用它的脚本将立即无法访问，此操作不能撤销。', [it.name]), t('吊销令牌'), {
    type: 'warning',
    confirmButtonText: t('吊销'),
    cancelButtonText: t('取消'),
  }).then(() => true, () => false)
  if (!ok)
    return
  await revokeAccessTokenApi(it.id)
  if (created.value?.id === it.id)
    created.value = null
  ElMessage.success(t('已吊销'))
  await load()
}

const origin = window.location.origin
</script>

<template>
  <el-dialog v-model="visible" :title="$t('访问令牌')" width="680px" append-to-body data-testid="token-dialog">
    <div class="mb-3 text-[13px] text-gray-500 leading-6">
      {{ $t('给脚本、CI 或其他系统调用接口用，权限与你的账号相同；勾选「只读令牌」则只能读取数据、不能修改。令牌只在创建时显示一次，请立即保存；不用时随时吊销。') }}
    </div>

    <div v-if="created" class="mb-4 border border-[#a7d7b8] rounded-lg bg-[#f1faf4] p-3" data-testid="token-created">
      <div class="mb-1 text-[13px] font-semibold">
        {{ $t('令牌「{0}」已创建，请立即复制保存——关闭后无法再次查看', [created.name]) }}
      </div>
      <div class="flex items-center gap-2">
        <code class="min-w-0 flex-1 break-all rounded bg-white px-2 py-1 text-xs" data-testid="token-value">{{ created.token }}</code>
        <el-button size="small" @click="copy(created.token)">
          {{ $t('复制') }}
        </el-button>
      </div>
      <div class="mt-2 text-xs text-gray-500 leading-5">
        {{ $t('用法：') }}<code>curl -H "Authorization: Bearer {{ created.token.slice(0, 8) }}…" {{ origin }}/api/ledger/workbooks</code>
      </div>
    </div>

    <div class="mb-3 flex items-center gap-2">
      <el-input v-model="name" :placeholder="$t('令牌名称，如「数据同步脚本」')" maxlength="40" class="max-w-72" data-testid="token-name" @keyup.enter="create" />
      <el-select v-model="days" class="w-32" data-testid="token-days">
        <el-option v-for="d in DAYS" :key="d.value" :label="d.label" :value="d.value" />
      </el-select>
      <el-checkbox v-model="readOnly" data-testid="token-readonly">
        {{ $t('只读令牌') }}
      </el-checkbox>
      <el-button type="primary" :loading="creating" data-testid="token-create" @click="create">
        {{ $t('创建令牌') }}
      </el-button>
    </div>
    <div class="mb-3 flex items-center gap-2">
      <el-select
        v-model="limitedTo"
        multiple
        collapse-tags
        collapse-tags-tooltip
        clearable
        filterable
        class="w-96"
        data-testid="token-datasets"
        :placeholder="$t('可访问的数据集（不选 = 不限）')"
      >
        <el-option v-for="d in datasetOptions" :key="d.id" :label="d.name" :value="d.id" />
      </el-select>
      <span class="text-xs text-gray-400">{{ $t('限定后只能读写这些数据集的数据，不能做其他操作') }}</span>
    </div>
    <div v-if="error" class="mb-2 text-[13px] text-[#B91C1C]" data-testid="token-error">
      {{ error }}
    </div>

    <el-table v-loading="loading" :data="items" size="small" :empty-text="$t('还没有访问令牌')" max-height="320">
      <el-table-column :label="$t('名称')" min-width="120">
        <template #default="{ row }">
          {{ row.name }}
          <el-tag v-if="row.readOnly" size="small" type="info" class="ml-1">
            {{ $t('只读令牌') }}
          </el-tag>
          <el-tag v-if="row.datasets.length" size="small" type="warning" class="ml-1" :title="row.datasets.map((d: { id: string, name: string | null }) => d.name ?? d.id).join('、')">
            {{ $t('限定 {0} 个数据集', [row.datasets.length]) }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column :label="$t('令牌')" width="110">
        <template #default="{ row }">
          <code class="text-xs">{{ row.prefix }}…</code>
        </template>
      </el-table-column>
      <el-table-column :label="$t('过期')" width="150">
        <template #default="{ row }">
          <span v-if="row.expired" class="text-[#B91C1C]">{{ $t('已过期') }}</span>
          <span v-else-if="row.expiresAt">{{ row.expiresAt.slice(0, 10) }}</span>
          <span v-else class="text-gray-400">{{ $t('永不过期') }}</span>
        </template>
      </el-table-column>
      <el-table-column :label="$t('最近使用')" width="150">
        <template #default="{ row }">
          {{ row.lastUsedAt ?? $t('从未使用') }}
        </template>
      </el-table-column>
      <el-table-column :label="$t('操作')" width="70" align="right">
        <template #default="{ row }">
          <el-button link type="danger" size="small" :data-testid="`token-revoke-${row.name}`" @click="revoke(row)">
            {{ $t('吊销') }}
          </el-button>
        </template>
      </el-table-column>
    </el-table>
    <template #footer>
      <el-button @click="visible = false">
        {{ $t('关闭') }}
      </el-button>
    </template>
  </el-dialog>
</template>
