<script setup lang="ts">
import type { LedgerUserItem } from '@/api/approval'
import type { ScopeDatasetBinding, ScopeOverview, ScopeRule } from '@/api/auth'
import { computed, reactive, ref, watch } from 'vue'
import { listLedgerUsersApi } from '@/api/approval'
import { deleteScopeRuleApi, getScopeOverviewApi, saveScopeBindingApi, saveScopeRuleApi } from '@/api/auth'

/**
 * 数据权限（仅管理员）：按店铺限制用户能看到的数据。
 * ① 用户范围：给用户（或「默认规则」）指定可见的店铺，没有规则的用户和管理员不受限；
 * ② 数据集：指定每个数据集按哪一列过滤，或设为所有人可见——没有设置的数据集，受限用户看不到
 */
const props = defineProps<{
  visible: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
}>()

const DEFAULT_ID = '*'
const tab = ref<'rules' | 'datasets'>('rules')
const data = ref<ScopeOverview | null>(null)
const loading = ref(false)

async function load() {
  loading.value = true
  try {
    data.value = await getScopeOverviewApi()
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, (v) => {
  if (v) {
    tab.value = 'rules'
    load()
  }
})

const defaultRule = computed(() => data.value?.rules.find(r => r.subjectId === DEFAULT_ID) ?? null)
const userRules = computed(() => data.value?.rules.filter(r => r.subjectId !== DEFAULT_ID) ?? [])
const hiddenCount = computed(() => data.value?.datasets.filter(d => d.effective === 'hidden').length ?? 0)

function describe(r: ScopeRule | null) {
  if (!r || r.mode === 'all')
    return '不限（看到全部店铺）'
  return r.shops.length ? `仅 ${r.shops.join('、')}` : '看不到任何店铺'
}

// ── 编辑规则 ──
const edit = reactive({ visible: false, isDefault: false, userId: '', userName: '', mode: 'shops' as 'all' | 'shops', shops: [] as string[], saving: false })
const candidates = ref<LedgerUserItem[]>([])
const searching = ref(false)

async function searchUsers(keyword = '') {
  searching.value = true
  try {
    candidates.value = await listLedgerUsersApi(keyword)
  }
  finally {
    searching.value = false
  }
}

function openEdit(r: ScopeRule | null, isDefault = false) {
  Object.assign(edit, {
    visible: true,
    isDefault,
    userId: r?.subjectId ?? (isDefault ? DEFAULT_ID : ''),
    userName: r?.subjectName ?? '',
    mode: r?.mode ?? 'shops',
    shops: [...(r?.shops ?? [])],
  })
  if (!isDefault && !r)
    searchUsers()
}

const canSave = computed(() => !!edit.userId && (edit.mode === 'all' || edit.shops.length > 0))

async function saveRule() {
  edit.saving = true
  try {
    await saveScopeRuleApi({ subjectId: edit.userId, subjectName: edit.userName, mode: edit.mode, shops: edit.mode === 'shops' ? edit.shops : [] })
    ElMessage.success('已保存，立即生效')
    edit.visible = false
    await load()
  }
  finally {
    edit.saving = false
  }
}

async function removeRule(r: ScopeRule) {
  const ok = await ElMessageBox.confirm(
    r.subjectId === DEFAULT_ID ? '删除默认规则后，没有单独规则的用户将不受限制。' : `删除「${r.subjectName}」的规则后，该用户改用默认规则${defaultRule.value ? '' : '（目前没有，即不受限）'}。`,
    '删除规则',
    { type: 'warning', confirmButtonText: '删除' },
  ).then(() => true, () => false)
  if (!ok)
    return
  await deleteScopeRuleApi(r.subjectId)
  ElMessage.success('已删除')
  await load()
}

// ── 数据集绑定 ──
type BindMode = 'none' | 'open' | 'column'
const modeOf = (d: ScopeDatasetBinding): BindMode => (d.open ? 'open' : d.scopeColumn ? 'column' : 'none')

async function setBinding(d: ScopeDatasetBinding, mode: BindMode, column?: string) {
  const col = mode === 'column' ? (column ?? d.scopeColumn ?? d.columns.find(c => /店铺|门店|shop/i.test(c.title))?.key ?? d.columns[0]?.key ?? null) : null
  await saveScopeBindingApi(d.id, { scopeColumn: col, open: mode === 'open' })
  await load()
}

const EFFECT: Record<ScopeDatasetBinding['effective'], { label: (d: ScopeDatasetBinding) => string, type: 'success' | 'info' | 'warning' }> = {
  column: { label: d => `按「${d.columns.find(c => c.key === d.effectiveColumn)?.title ?? d.effectiveColumn}」列过滤`, type: 'success' },
  open: { label: () => '所有人可见', type: 'info' },
  inherit: { label: d => `沿用左表：按「${d.columns.find(c => c.key === d.effectiveColumn)?.title ?? d.effectiveColumn}」列过滤`, type: 'success' },
  hidden: { label: () => '受限用户看不到', type: 'warning' },
}
</script>

<template>
  <el-dialog :model-value="visible" title="数据权限" width="860px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div class="mb-3 rounded bg-[#f5f7fa] px-3 py-2 text-xs text-gray-500 leading-5">
      按店铺限制用户能看到的数据：<b>数据范围受限的用户</b>读取数据集、透视、公式、SQL、AI 助手、导出时只看到自己店铺的行；
      导入、建视图、填报、定时推送等会绕过过滤的功能对他们关闭。管理员和没有规则的用户不受限。
      已经写进工作簿单元格里的数值不在此限制内，共享工作簿前请确认内容。
    </div>
    <el-tabs v-model="tab">
      <el-tab-pane label="用户范围" name="rules">
        <div v-loading="loading">
          <div class="mb-3 flex items-center gap-3 border border-[#ebeef5] rounded-md border-solid p-3">
            <div class="min-w-0 flex-1">
              <div class="text-[13px] font-medium">
                默认规则 <span class="ml-1 text-xs text-gray-400 font-normal">没有单独规则的用户适用</span>
              </div>
              <div class="mt-1 text-xs" :class="defaultRule && defaultRule.mode === 'shops' ? 'text-[#B45309]' : 'text-gray-500'">
                {{ describe(defaultRule) }}
              </div>
            </div>
            <el-button size="small" @click="openEdit(defaultRule, true)">
              {{ defaultRule ? '修改' : '设置' }}
            </el-button>
            <el-button v-if="defaultRule" size="small" type="danger" plain @click="removeRule(defaultRule)">
              删除
            </el-button>
          </div>

          <div class="mb-2 flex items-center">
            <span class="text-[13px] font-medium">用户的单独规则</span>
            <el-button class="ml-auto" size="small" type="primary" @click="openEdit(null)">
              + 添加用户
            </el-button>
          </div>
          <el-table :data="userRules" size="small" max-height="300" empty-text="还没有单独规则：所有人按默认规则（没有默认规则则不受限）">
            <el-table-column label="用户" prop="subjectName" min-width="130" />
            <el-table-column label="可见范围" min-width="260">
              <template #default="{ row }">
                <span v-if="row.mode === 'all'" class="text-gray-500">不限</span>
                <template v-else>
                  <el-tag v-for="s in row.shops" :key="s" size="small" class="mr-1">
                    {{ s }}
                  </el-tag>
                  <span v-if="!row.shops.length" class="text-[#B45309]">看不到任何店铺</span>
                </template>
              </template>
            </el-table-column>
            <el-table-column label="设置人 / 时间" min-width="160">
              <template #default="{ row }">
                <span class="text-xs text-gray-400">{{ row.updatedBy }} · {{ row.updatedAt?.slice(5, 16) }}</span>
              </template>
            </el-table-column>
            <el-table-column width="120" align="right">
              <template #default="{ row }">
                <el-button link size="small" type="primary" @click="openEdit(row)">
                  修改
                </el-button>
                <el-button link size="small" type="danger" @click="removeRule(row)">
                  删除
                </el-button>
              </template>
            </el-table-column>
          </el-table>
        </div>
      </el-tab-pane>

      <el-tab-pane name="datasets">
        <template #label>
          数据集 <el-badge v-if="hiddenCount" :value="hiddenCount" type="warning" />
        </template>
        <div class="mb-2 text-xs text-gray-500">
          受限用户只能看到「按列过滤」和「所有人可见」的数据集；<b>没有设置的默认对他们隐藏</b>。「按列过滤」的列里的值要与规则里的店铺名一致。
        </div>
        <el-table v-loading="loading" :data="data?.datasets ?? []" size="small" max-height="380">
          <el-table-column label="数据集" prop="name" min-width="130" show-overflow-tooltip />
          <el-table-column label="受限用户看到的" min-width="220">
            <template #default="{ row }">
              <el-tag size="small" :type="EFFECT[row.effective as ScopeDatasetBinding['effective']].type">
                {{ EFFECT[row.effective as ScopeDatasetBinding['effective']].label(row) }}
              </el-tag>
            </template>
          </el-table-column>
          <el-table-column label="设置" min-width="280">
            <template #default="{ row }">
              <span v-if="row.kind === 'builtin'" class="text-xs text-gray-400">固定按「店铺」列过滤</span>
              <div v-else class="flex items-center gap-2">
                <el-select :model-value="modeOf(row)" size="small" class="!w-36" @change="(m: BindMode) => setBinding(row, m)">
                  <el-option value="none" label="不设置（隐藏）" />
                  <el-option value="open" label="所有人可见" />
                  <el-option value="column" label="按列过滤" />
                </el-select>
                <el-select v-if="modeOf(row) === 'column'" :model-value="row.scopeColumn" size="small" class="!w-36" placeholder="选择店铺列" @change="(c: string) => setBinding(row, 'column', c)">
                  <el-option v-for="c in row.columns" :key="c.key" :value="c.key" :label="c.title" />
                </el-select>
              </div>
            </template>
          </el-table-column>
        </el-table>
      </el-tab-pane>
    </el-tabs>

    <el-dialog v-model="edit.visible" :title="edit.isDefault ? '默认规则' : (edit.userName ? `「${edit.userName}」的数据范围` : '添加用户的数据范围')" width="480px" append-to-body>
      <el-form label-width="72px" size="small">
        <el-form-item v-if="!edit.isDefault" label="用户">
          <span v-if="edit.userName" class="text-[13px]">{{ edit.userName }}</span>
          <el-select v-else v-model="edit.userId" filterable remote :remote-method="searchUsers" :loading="searching" placeholder="输入姓名搜索（需登录过本系统）" class="w-full" no-data-text="没有找到：对方需要先登录过本系统">
            <el-option v-for="u in candidates" :key="u.id" :label="u.name" :value="u.id" />
          </el-select>
        </el-form-item>
        <el-form-item label="范围">
          <el-radio-group v-model="edit.mode">
            <el-radio value="shops">
              指定店铺
            </el-radio>
            <el-radio value="all">
              不限
            </el-radio>
          </el-radio-group>
        </el-form-item>
        <el-form-item v-if="edit.mode === 'shops'" label="店铺">
          <el-select v-model="edit.shops" multiple filterable allow-create default-first-option placeholder="选择或输入店铺名" class="w-full">
            <el-option v-for="s in data?.shops ?? []" :key="s" :label="s" :value="s" />
          </el-select>
        </el-form-item>
      </el-form>
      <div class="text-xs text-gray-400">
        保存后立即生效。店铺名要与数据里的值完全一致；管理员不受任何规则限制。
      </div>
      <template #footer>
        <el-button @click="edit.visible = false">
          取消
        </el-button>
        <el-button type="primary" :disabled="!canSave" :loading="edit.saving" @click="saveRule">
          保存
        </el-button>
      </template>
    </el-dialog>
  </el-dialog>
</template>
