<script setup lang="ts">
import type { LedgerUserItem } from '@/api/approval'
import type { Collect, CollectInput } from '@/api/collect'
import type { DatasetColumnKind, DatasetInfo } from '@/api/dataset'
import { computed, reactive, ref, watch } from 'vue'
import { listLedgerUsersApi } from '@/api/approval'
import { collectDetailApi, createCollectApi, updateCollectApi } from '@/api/collect'
import { getDatasetsApi } from '@/api/dataset'
import { t } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'

/** 新建 / 修改收集任务：填报表（新建或用已有的）、填报单位与负责人。修改时只能改名称和单位 / 负责人 */
const props = defineProps<{
  visible: boolean
  /** 修改的任务；为空时新建 */
  collect: Collect | null
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'saved', id: string): void
}>()

interface UnitRow {
  unit: string
  userId: string | null
}

const KIND_LABEL: Partial<Record<DatasetColumnKind, string>> = { money: t('金额'), number: t('数字'), text: t('文本'), datetime: t('日期') }
const ledgerUser = useLedgerUserStore()

const datasets = ref<DatasetInfo[]>([])
const userOptions = ref<LedgerUserItem[]>([])
const saving = ref(false)
const loading = ref(false)
const bulkOpen = ref(false)
const bulkText = ref('')
const bulkBusy = ref(false)

const form = reactive({
  name: '',
  source: 'new' as 'new' | 'existing',
  tableName: '',
  columns: [{ title: '', kind: 'money' as DatasetColumnKind }],
  keyColumns: [] as string[],
  datasetId: '',
  unitColumn: '填报单位',
  periodColumn: '期次',
  dimension: '填报单位',
  units: [] as UnitRow[],
})

const editing = computed(() => !!props.collect)
const plainDatasets = computed(() => datasets.value.filter(d => d.kind === 'table'))
const pickedDataset = computed(() => datasets.value.find(d => d.id === form.datasetId) ?? null)
const textColumns = computed(() => pickedDataset.value?.columns.filter(c => c.kind === 'text' || c.kind === 'id') ?? [])
const businessColumns = computed(() => form.columns.map(c => c.title.trim()).filter(Boolean))

function ensureUsers(list: LedgerUserItem[]) {
  const known = new Set(userOptions.value.map(u => u.id))
  userOptions.value = [...userOptions.value, ...list.filter(u => !known.has(u.id))]
}

async function searchUsers(keyword: string) {
  ensureUsers(await listLedgerUsersApi(keyword).catch(() => []))
}

watch(() => props.visible, async (v) => {
  if (!v)
    return
  Object.assign(form, { name: '', source: 'new', tableName: '', columns: [{ title: '', kind: 'money' }], keyColumns: [], datasetId: '', unitColumn: '填报单位', periodColumn: '期次', dimension: '填报单位', units: [] })
  bulkOpen.value = false
  bulkText.value = ''
  loading.value = true
  try {
    const [ds] = await Promise.all([getDatasetsApi().catch(() => [] as DatasetInfo[]), searchUsers('')])
    datasets.value = ds
    const c = props.collect
    if (c) {
      const d = await collectDetailApi(c.id)
      form.name = c.name
      form.units = d.units.map(u => ({ unit: u.unit, userId: u.userId }))
      ensureUsers(d.units.filter(u => u.userId).map(u => ({ id: u.userId!, name: u.userName ?? u.userId! })))
    }
  }
  finally {
    loading.value = false
  }
}, { immediate: true })

/** 选了已有的数据集：单位列、期次列优先用同名的列 */
function onDatasetChange() {
  const cols = textColumns.value
  form.unitColumn = cols.find(c => c.title === '填报单位')?.title ?? cols[0]?.title ?? ''
  form.periodColumn = cols.find(c => c.title === '期次')?.title ?? cols.find(c => c.title !== form.unitColumn)?.title ?? ''
}

function addColumn() {
  form.columns.push({ title: '', kind: 'money' })
}

function removeColumn(i: number) {
  const gone = form.columns[i]?.title.trim()
  form.columns.splice(i, 1)
  form.keyColumns = form.keyColumns.filter(k => k !== gone)
}

function addUnit() {
  form.units.push({ unit: '', userId: null })
}

/** 批量粘贴：每行「单位」或「单位,负责人账号名」（逗号或制表符分隔，可以直接从表格里复制两列） */
async function applyBulk() {
  const lines = bulkText.value.split('\n').map(l => l.trim()).filter(Boolean)
  if (!lines.length)
    return
  bulkBusy.value = true
  try {
    const parsed = lines.map((l) => {
      const [unit, who] = l.split(/[\t,，]/).map(s => s.trim())
      return { unit: unit ?? '', who: who ?? '' }
    })
    const names = [...new Set(parsed.map(p => p.who).filter(Boolean))]
    const found = new Map<string, LedgerUserItem>()
    await Promise.all(names.map(async (n) => {
      const list = await listLedgerUsersApi(n).catch(() => [])
      const hit = list.find(u => u.name === n)
      if (hit)
        found.set(n, hit)
      ensureUsers(list)
    }))
    const have = new Set(form.units.map(u => u.unit.trim()))
    const missing = new Set<string>()
    let added = 0
    for (const p of parsed) {
      if (!p.unit || have.has(p.unit))
        continue
      have.add(p.unit)
      const user = p.who ? found.get(p.who) : undefined
      if (p.who && !user)
        missing.add(p.who)
      form.units.push({ unit: p.unit, userId: user?.id ?? null })
      added++
    }
    bulkText.value = ''
    bulkOpen.value = false
    ElMessage.success(t('已添加 {0} 个单位', [added]))
    if (missing.size)
      ElMessageBox.alert(t('这些负责人没有找到：{0}。对方需要先登录过一次系统；名字要和账号名完全一致。这些单位已添加，负责人为空，可以在列表里再选。', [[...missing].join('、')]), t('部分负责人没有匹配'), { type: 'warning' })
  }
  finally {
    bulkBusy.value = false
  }
}

function build(): CollectInput | string {
  if (!form.name.trim())
    return t('请填写任务名称')
  const units = form.units.map(u => ({ unit: u.unit.trim(), userId: u.userId }))
  if (!units.length || units.some(u => !u.unit))
    return units.length ? t('填报单位的名称不能为空') : t('请至少添加一个填报单位')
  const input: CollectInput = { name: form.name.trim(), units, dimension: form.dimension.trim() || undefined }
  if (form.source === 'new') {
    if (!form.tableName.trim())
      return t('请填写填报表的名称')
    const columns = form.columns.map(c => ({ title: c.title.trim(), kind: c.kind })).filter(c => c.title)
    if (!columns.length)
      return t('请至少添加一个要填写的列')
    if (!form.unitColumn.trim() || !form.periodColumn.trim())
      return t('请填写单位列和期次列的名称')
    input.newTable = { name: form.tableName.trim(), columns, keyColumns: form.keyColumns }
  }
  else {
    if (!form.datasetId)
      return t('请选择数据集')
    if (!form.unitColumn || !form.periodColumn)
      return t('请选择单位列和期次列')
    input.datasetId = form.datasetId
  }
  input.unitColumn = form.unitColumn.trim()
  input.periodColumn = form.periodColumn.trim()
  return input
}

async function save() {
  const built = build()
  if (typeof built === 'string')
    return ElMessage.warning(built)
  saving.value = true
  try {
    if (props.collect) {
      await updateCollectApi(props.collect.id, { name: built.name, units: built.units })
      ElMessage.success(t('已保存'))
      emit('update:visible', false)
      emit('saved', props.collect.id)
    }
    else {
      const id = await createCollectApi(built)
      ElMessage.success(t('已创建，下一步：开启第一期'))
      emit('update:visible', false)
      emit('saved', id)
    }
  }
  finally {
    saving.value = false
  }
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="editing ? $t('修改收集任务 · {0}', [collect?.name]) : $t('新建收集任务')"
    width="780px"
    :close-on-click-modal="false"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div v-loading="loading">
      <el-form label-width="96px" @submit.prevent>
        <el-form-item :label="$t('任务名称')" required>
          <el-input v-model="form.name" maxlength="60" :placeholder="$t('如：月度经营数据收集')" />
        </el-form-item>

        <template v-if="!editing">
          <el-form-item :label="$t('填报表')" required>
            <el-radio-group v-model="form.source" size="small">
              <el-radio-button value="new">
                {{ $t('新建一张填报表') }}
              </el-radio-button>
              <el-radio-button value="existing">
                {{ $t('用已有的数据集') }}
              </el-radio-button>
            </el-radio-group>
          </el-form-item>

          <template v-if="form.source === 'new'">
            <el-form-item :label="$t('表名称')" required>
              <el-input v-model="form.tableName" maxlength="40" :placeholder="$t('如：月度经营数据')" />
            </el-form-item>
            <el-form-item :label="$t('单位列 / 期次列')">
              <div class="w-full flex items-center gap-2">
                <el-input v-model="form.unitColumn" class="!w-40" maxlength="20" />
                <el-input v-model="form.periodColumn" class="!w-40" maxlength="20" />
                <span class="text-xs text-gray-500">{{ $t('这两列自动加在最前面') }}</span>
              </div>
            </el-form-item>
            <el-form-item :label="$t('填写的列')" required>
              <div class="w-full">
                <div v-for="(c, i) in form.columns" :key="i" class="mb-1 flex gap-2">
                  <el-input v-model="c.title" class="min-w-0 flex-1" maxlength="30" :placeholder="$t('列名，如：营收')" />
                  <el-select v-model="c.kind" class="!w-24">
                    <el-option v-for="(label, k) in KIND_LABEL" :key="k" :label="label" :value="k" />
                  </el-select>
                  <el-button link type="danger" :disabled="form.columns.length <= 1" @click="removeColumn(i)">
                    {{ $t('删除') }}
                  </el-button>
                </div>
                <el-button link type="primary" size="small" @click="addColumn">
                  {{ $t('+ 添加列') }}
                </el-button>
              </div>
            </el-form-item>
            <el-form-item :label="$t('区分不同行')">
              <el-select v-model="form.keyColumns" class="w-full" multiple clearable :placeholder="$t('不选：每个单位每期只填一行')">
                <el-option v-for="c in businessColumns" :key="c" :label="c" :value="c" />
              </el-select>
              <div class="hint">
                {{ $t('同一单位同一期要填多行时（如多个商品），选出区分每一行的列；只填一行汇总就不用选') }}
              </div>
            </el-form-item>
          </template>

          <template v-else>
            <el-form-item :label="$t('数据集')" required>
              <el-select v-model="form.datasetId" class="w-full" filterable :placeholder="$t('选择已有的数据集')" @change="onDatasetChange">
                <el-option v-for="d in plainDatasets" :key="d.id" :label="d.name" :value="d.id" />
              </el-select>
              <div class="hint">
                {{ $t('数据集的主键必须包含单位列和期次列，否则不同单位 / 不同期的数据会互相覆盖') }}
              </div>
            </el-form-item>
            <el-form-item :label="$t('单位列 / 期次列')" required>
              <div class="w-full flex gap-2">
                <el-select v-model="form.unitColumn" class="flex-1" :placeholder="$t('单位列')">
                  <el-option v-for="c in textColumns" :key="c.key" :label="c.title" :value="c.title" />
                </el-select>
                <el-select v-model="form.periodColumn" class="flex-1" :placeholder="$t('期次列')">
                  <el-option v-for="c in textColumns" :key="c.key" :label="c.title" :value="c.title" />
                </el-select>
              </div>
            </el-form-item>
          </template>

          <el-form-item :label="$t('权限维度')">
            <el-input v-model="form.dimension" class="!w-48" maxlength="20" />
            <div class="hint">
              {{ $t('负责人在这个维度上只能看 / 填自己负责的单位。同一个维度下的任务共用，不要在「数据权限」里手动改这个维度') }}
            </div>
          </el-form-item>
        </template>

        <el-form-item v-else :label="$t('填报表')">
          <div class="text-[13px] leading-6">
            {{ collect?.datasetName ?? $t('（数据集已删除）') }}
            <span class="text-xs text-gray-500">· {{ $t('单位列「{0}」，期次列「{1}」，权限维度「{2}」', [collect?.unitColumn, collect?.periodColumn, collect?.dimension]) }}</span>
          </div>
        </el-form-item>

        <el-form-item :label="$t('填报单位')" required>
          <div class="w-full">
            <div class="mb-2 flex items-center gap-2">
              <el-button size="small" @click="addUnit">
                {{ $t('+ 添加单位') }}
              </el-button>
              <el-button size="small" @click="bulkOpen = !bulkOpen">
                {{ $t('批量粘贴') }}
              </el-button>
              <span class="text-xs text-gray-500">{{ $t('共 {0} 个', [form.units.length]) }}</span>
            </div>
            <div v-if="bulkOpen" class="mb-2">
              <el-input v-model="bulkText" type="textarea" :rows="5" :placeholder="$t('每行一个单位，可以在后面加负责人的账号名（逗号或制表符分隔）：\n店铺A,张三\n店铺B,李四')" />
              <el-button class="mt-1" size="small" type="primary" :loading="bulkBusy" @click="applyBulk">
                {{ $t('添加') }}
              </el-button>
            </div>
            <div class="max-h-72 overflow-auto">
              <div v-for="(u, i) in form.units" :key="i" class="mb-1 flex gap-2">
                <el-input v-model="u.unit" class="min-w-0 flex-1" maxlength="60" :placeholder="$t('单位名称，如：店铺A')" />
                <el-select
                  v-model="u.userId"
                  class="!w-52"
                  clearable
                  filterable
                  remote
                  :remote-method="searchUsers"
                  :placeholder="ledgerUser.authRequired ? $t('负责人（可搜索）') : $t('没开登录，不能指派')"
                  :disabled="!ledgerUser.authRequired"
                >
                  <el-option v-for="p in userOptions" :key="p.id" :label="p.name" :value="p.id" />
                </el-select>
                <el-button link type="danger" @click="form.units.splice(i, 1)">
                  {{ $t('删除') }}
                </el-button>
              </div>
              <div v-if="!form.units.length" class="text-xs text-gray-400">
                {{ $t('还没有填报单位') }}
              </div>
            </div>
            <div class="hint">
              {{ $t('保存后，负责人在权限维度上只能看到、只能填自己负责的单位（自动同步成数据权限），同时成为「数据范围受限用户」：导入数据、建视图、定时推送、告警等功能对他们关闭。负责人需要先登录过一次系统；不指派负责人的单位只能由管理员代填。') }}
            </div>
          </div>
        </el-form-item>
      </el-form>
    </div>

    <template #footer>
      <el-button @click="emit('update:visible', false)">
        {{ $t('取消') }}
      </el-button>
      <el-button type="primary" :loading="saving" @click="save">
        {{ editing ? $t('保存') : $t('创建') }}
      </el-button>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.hint {
  @apply mt-1 w-full text-xs text-gray-500 leading-5;
}
</style>
