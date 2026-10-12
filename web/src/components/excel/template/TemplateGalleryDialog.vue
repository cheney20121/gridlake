<script setup lang="ts">
import type { DatasetInfo } from '@/api/dataset'
import type { TemplateInfo, TemplateUse } from '@/api/template'
import type { WorkbookMeta } from '@/api/workbook'
import { computed, reactive, ref, watch } from 'vue'
import { deleteTemplateApi, listTemplatesApi, PART_LABEL, templateUsesApi, updateTemplateApi } from '@/api/template'
import { t as tr } from '@/i18n'
import UseTemplateDialog from './UseTemplateDialog.vue'

/** 模板中心：内置模板与自建模板，使用、修改信息、删除、使用记录 */
const props = defineProps<{
  visible: boolean
  datasets: DatasetInfo[]
  operator?: string
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'created', meta: WorkbookMeta, warnings: string[]): void
  (e: 'importData'): void
  /** 把当前工作簿发布为模板 */
  (e: 'publish'): void
  (e: 'openWorkbook', id: string): void
}>()

const items = ref<TemplateInfo[]>([])
const loading = ref(false)
const category = ref('')
const keyword = ref('')
const useVisible = ref(false)
const using = ref<TemplateInfo | null>(null)

async function load() {
  loading.value = true
  try {
    items.value = await listTemplatesApi()
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, v => v && load(), { immediate: true })

const categories = computed(() => {
  const set = new Set(items.value.map(t => t.category).filter(Boolean))
  return [...set]
})

const shown = computed(() => {
  const kw = keyword.value.trim().toLowerCase()
  return items.value.filter((t) => {
    if (category.value === '__mine' ? t.builtin : category.value && t.category !== category.value)
      return false
    return !kw || `${t.name} ${t.description}`.toLowerCase().includes(kw)
  })
})

function partsText(t: TemplateInfo) {
  return Object.entries(t.summary)
    .filter(([k, n]) => k !== 'sheets' && n > 0)
    .map(([k, n]) => `${n} 个${PART_LABEL[k] ?? '部件'}`)
    .join(' · ')
}

function needsText(t: TemplateInfo) {
  if (!t.datasets.length)
    return tr('不需要数据')
  return t.datasets.map(d => d.builtinId ? `内置「${d.name}」` : `${d.name}（${d.columns.map(c => c.title).join('、')}）`).join(' + ')
}

function open(t: TemplateInfo) {
  using.value = t
  useVisible.value = true
}

function onCreated(meta: WorkbookMeta, warnings: string[]) {
  emit('update:visible', false)
  emit('created', meta, warnings)
}

// ── 自建模板：修改信息 / 删除 / 使用记录 ──

const editVisible = ref(false)
const editing = ref<TemplateInfo | null>(null)
const form = reactive({ name: '', description: '', category: '' })

function openEdit(t: TemplateInfo) {
  editing.value = t
  Object.assign(form, { name: t.name, description: t.description, category: t.category })
  editVisible.value = true
}

async function saveEdit() {
  if (!editing.value)
    return
  if (!form.name.trim())
    return ElMessage.warning(tr('请填写模板名称'))
  await updateTemplateApi(editing.value.id, { name: form.name.trim(), description: form.description.trim(), category: form.category.trim() })
  ElMessage.success(tr('已保存'))
  editVisible.value = false
  load()
}

async function remove(t: TemplateInfo) {
  const ok = await ElMessageBox.confirm(tr('删除模板「{0}」？已用它创建的工作簿不受影响。', [t.name]), tr('删除模板'), { type: 'warning', confirmButtonText: tr('删除') }).then(() => true, () => false)
  if (!ok)
    return
  await deleteTemplateApi(t.id)
  ElMessage.success(tr('已删除'))
  load()
}

const usesFor = ref<string | null>(null)
const uses = ref<TemplateUse[]>([])

async function toggleUses(t: TemplateInfo) {
  if (usesFor.value === t.id) {
    usesFor.value = null
    return
  }
  usesFor.value = t.id
  uses.value = []
  uses.value = await templateUsesApi(t.id)
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('模板中心')"
    width="1000px"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="mb-3 flex items-center gap-3 text-[13px]">
      <el-radio-group v-model="category" size="small">
        <el-radio-button value="">
          {{ $t('全部') }}
        </el-radio-button>
        <el-radio-button v-for="c in categories" :key="c" :value="c">
          {{ c }}
        </el-radio-button>
        <el-radio-button value="__mine">
          {{ $t('我发布的') }}
        </el-radio-button>
      </el-radio-group>
      <el-input v-model="keyword" size="small" clearable :placeholder="$t('搜索模板')" class="!w-48" />
      <span class="flex-1" />
      <el-button size="small" :title="$t('把当前工作簿（透视表、图表、仪表盘、公式）发布为模板，供同事一键使用')" @click="emit('publish')">
        {{ $t('把当前工作簿发布为模板') }}
      </el-button>
    </div>

    <div v-loading="loading" class="grid grid-cols-2 min-h-40 gap-3">
      <div v-if="!shown.length && !loading" class="col-span-2 py-10 text-center text-[13px] text-gray-400">
        {{ category === '__mine' ? $t('还没有发布过模板：做好一个工作簿后点右上角「把当前工作簿发布为模板」') : $t('没有符合条件的模板') }}
      </div>
      <div v-for="t in shown" :key="t.id" class="card">
        <div class="flex items-center gap-2">
          <span class="text-[15px] text-gray-900 font-medium">{{ t.name }}</span>
          <el-tag v-if="t.builtin" size="small" type="success" effect="plain">
            {{ $t('内置') }}
          </el-tag>
          <el-tag v-if="t.category" size="small" type="info" effect="plain">
            {{ t.category }}
          </el-tag>
          <span class="ml-auto text-xs text-gray-400">{{ t.useCount ? $t('已使用 {0} 次', [t.useCount]) : $t('尚未使用') }}</span>
        </div>
        <div class="mt-1 min-h-10 text-xs text-gray-600 leading-5">
          {{ t.description || $t('（无说明）') }}
        </div>
        <div class="mt-2 text-xs text-gray-500 leading-5">
          <div><span class="label">{{ $t('工作表') }}</span>{{ t.sheetNames.join('、') || $t('{0} 个', [t.summary.sheets ?? 0]) }}</div>
          <div v-if="partsText(t)">
            <span class="label">{{ $t('内容') }}</span>{{ partsText(t) }}
          </div>
          <div><span class="label">{{ $t('需要数据') }}</span>{{ needsText(t) }}</div>
          <div v-if="!t.builtin && t.updatedAt" class="text-gray-400">
            <span class="label">{{ $t('发布') }}</span>{{ t.createdBy || '—' }} · {{ t.updatedAt.slice(0, 16) }}
          </div>
        </div>
        <div class="mt-2 flex items-center gap-1">
          <el-button size="small" type="primary" @click="open(t)">
            {{ $t('使用模板') }}
          </el-button>
          <el-button v-if="t.useCount" link size="small" @click="toggleUses(t)">
            {{ $t('使用记录') }}
          </el-button>
          <template v-if="!t.builtin">
            <el-button link size="small" @click="openEdit(t)">
              {{ $t('修改信息') }}
            </el-button>
            <el-button link size="small" type="danger" @click="remove(t)">
              {{ $t('删除') }}
            </el-button>
          </template>
        </div>
        <div v-if="usesFor === t.id" class="mt-2 border-t border-t-[#eef0f3] border-t-solid pt-2 text-xs leading-6">
          <div v-for="u in uses" :key="u.workbookId + u.usedAt" class="flex gap-2">
            <span class="text-gray-400">{{ u.usedAt.slice(0, 16) }}</span>
            <span class="text-gray-500">{{ u.usedBy }}</span>
            <el-button v-if="u.workbookName" link type="primary" size="small" class="!p-0 !text-xs" @click="emit('openWorkbook', u.workbookId)">
              {{ u.workbookName }}
            </el-button>
            <span v-else class="text-gray-400">{{ $t('（工作簿已删除）') }}</span>
          </div>
        </div>
      </div>
    </div>

    <UseTemplateDialog
      v-model:visible="useVisible"
      :template="using"
      :datasets="datasets"
      :operator="operator"
      @created="onCreated"
      @import-data="() => { useVisible = false; emit('update:visible', false); emit('importData') }"
    />

    <el-dialog v-model="editVisible" :title="$t('修改模板信息')" width="480px" append-to-body>
      <el-form label-width="64px" @submit.prevent>
        <el-form-item :label="$t('名称')" required>
          <el-input v-model="form.name" maxlength="60" />
        </el-form-item>
        <el-form-item :label="$t('分类')">
          <el-select v-model="form.category" filterable allow-create default-first-option :placeholder="$t('选择或输入分类')" class="w-full">
            <el-option v-for="c in categories" :key="c" :label="c" :value="c" />
          </el-select>
        </el-form-item>
        <el-form-item :label="$t('说明')">
          <el-input v-model="form.description" type="textarea" :rows="3" maxlength="200" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="editVisible = false">
          {{ $t('取消') }}
        </el-button>
        <el-button type="primary" @click="saveEdit">
          {{ $t('保存') }}
        </el-button>
      </template>
    </el-dialog>
  </el-dialog>
</template>

<style scoped lang="scss">
.card {
  @apply rounded-md border border-[#e5e7eb] border-solid px-3.5 py-3 text-[13px];
}

.label {
  @apply mr-2 inline-block w-14 text-gray-400;
}
</style>
