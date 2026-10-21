<script setup lang="ts">
import type { TemplateInfo } from '@/api/template'
import { computed, reactive, ref, watch } from 'vue'
import { KIND_LABEL, listTemplatesApi, PART_LABEL, publishTemplateApi } from '@/api/template'
import { t as tr } from '@/i18n'

/** 把当前工作簿（已保存到服务端的版本）发布为模板；可覆盖自己之前发布的模板 */
const props = defineProps<{
  visible: boolean
  workbookId: string | null
  workbookName: string
  operator?: string
  /** 发布前保存当前工作簿；返回 false 表示保存失败 */
  beforePublish?: () => Promise<boolean>
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'openGallery'): void
}>()

const templates = ref<TemplateInfo[]>([])
const saving = ref(false)
const result = ref<TemplateInfo | null>(null)
const form = reactive({ mode: 'new' as 'new' | 'replace', replaceId: '', name: '', category: '', description: '' })

const mine = computed(() => templates.value.filter(t => !t.builtin))
const categories = computed(() => [...new Set(templates.value.map(t => t.category).filter(Boolean))])

watch(() => props.visible, async (v) => {
  if (!v)
    return
  result.value = null
  templates.value = await listTemplatesApi().catch(() => [])
  // 从这个工作簿发布过：默认覆盖原模板
  const prev = mine.value.find(t => t.sourceWorkbookId === props.workbookId)
  Object.assign(form, prev
    ? { mode: 'replace', replaceId: prev.id, name: prev.name, category: prev.category, description: prev.description }
    : { mode: 'new', replaceId: '', name: props.workbookName, category: '', description: '' })
}, { immediate: true })

function onReplaceChange(id: string) {
  const t = mine.value.find(x => x.id === id)
  if (t)
    Object.assign(form, { name: t.name, category: t.category, description: t.description })
}

async function publish() {
  if (!props.workbookId)
    return ElMessage.warning(tr('工作簿还没有保存到服务端'))
  if (!form.name.trim())
    return ElMessage.warning(tr('请填写模板名称'))
  if (form.mode === 'replace' && !form.replaceId)
    return ElMessage.warning(tr('请选择要覆盖的模板'))
  saving.value = true
  try {
    if (props.beforePublish && !(await props.beforePublish()))
      return
    result.value = await publishTemplateApi({
      workbookId: props.workbookId,
      name: form.name.trim(),
      category: form.category.trim(),
      description: form.description.trim(),
      replaceId: form.mode === 'replace' ? form.replaceId : null,
      operator: props.operator,
    })
    ElMessage.success(form.mode === 'replace' ? tr('模板已更新') : tr('已发布为模板'))
  }
  finally {
    saving.value = false
  }
}

function partsText(parts: Record<string, number>) {
  return Object.entries(parts).map(([k, n]) => tr('{0} 个{1}', [n, PART_LABEL[k] ?? tr('部件')])).join('、')
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('发布为模板')"
    width="620px"
    :close-on-click-modal="false"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <template v-if="!result">
      <div class="mb-3 text-xs text-gray-500 leading-5">
        {{ $t('把「{0}」的工作表、透视表、图表、仪表盘和公式保存为模板。同事使用时选择自己的数据集并对应列，即生成一份按其数据计算的新工作簿；之后修改这个工作簿不影响模板，需要时重新发布覆盖即可。', [workbookName]) }}
      </div>
      <el-form label-width="72px" @submit.prevent>
        <el-form-item v-if="mine.length" :label="$t('方式')">
          <el-radio-group v-model="form.mode">
            <el-radio value="new">
              {{ $t('发布新模板') }}
            </el-radio>
            <el-radio value="replace">
              {{ $t('覆盖已有模板') }}
            </el-radio>
          </el-radio-group>
        </el-form-item>
        <el-form-item v-if="form.mode === 'replace'" :label="$t('覆盖')" required>
          <el-select v-model="form.replaceId" class="w-full" :placeholder="$t('选择要覆盖的模板')" @change="onReplaceChange">
            <el-option v-for="t in mine" :key="t.id" :label="t.name" :value="t.id" />
          </el-select>
        </el-form-item>
        <el-form-item :label="$t('名称')" required>
          <el-input v-model="form.name" maxlength="60" />
        </el-form-item>
        <el-form-item :label="$t('分类')">
          <el-select v-model="form.category" filterable allow-create default-first-option clearable :placeholder="$t('选择或输入，如：电商财务')" class="w-full">
            <el-option v-for="c in categories" :key="c" :label="c" :value="c" />
          </el-select>
        </el-form-item>
        <el-form-item :label="$t('说明')">
          <el-input v-model="form.description" type="textarea" :rows="3" maxlength="200" :placeholder="$t('这个模板用来做什么、需要什么数据')" />
        </el-form-item>
      </el-form>
    </template>

    <template v-else>
      <div class="text-[13px]">
        <div class="mb-2 text-[#0B5A2E] font-medium">
          {{ $t('✓ 「{0}」已发布', [result.name]) }}
        </div>
        <div class="mb-1 text-xs text-gray-500">
          {{ $t('使用这个模板时需要以下数据（按列名对应，列名不同可在使用时选择）：') }}
        </div>
        <div v-if="!result.datasets.length" class="text-xs text-gray-500">
          {{ $t('不引用数据集') }}
        </div>
        <div v-for="d in result.datasets" :key="d.key" class="mb-2 rounded-md bg-[#f9f9f7] px-3 py-2">
          <div>
            <span class="font-medium">{{ d.name }}</span>
            <span v-if="d.builtinId" class="ml-1 text-xs text-gray-400">{{ $t('内置数据集，自动对应') }}</span>
            <span class="ml-2 text-xs text-gray-400">{{ $t('用于 {0}', [partsText(d.parts)]) }}</span>
          </div>
          <div class="mt-1 text-xs text-gray-600">
            <span v-for="c in d.columns" :key="c.title" class="mr-3">{{ c.title }}<span class="text-gray-400">（{{ KIND_LABEL[c.kind] }}）</span></span>
          </div>
        </div>
        <div v-if="result.warnings.length" class="mt-2 text-xs text-[#92400E] leading-5">
          <div v-for="w in result.warnings" :key="w">
            · {{ w }}
          </div>
        </div>
      </div>
    </template>

    <template #footer>
      <template v-if="!result">
        <el-button @click="emit('update:visible', false)">
          {{ $t('取消') }}
        </el-button>
        <el-button type="primary" :loading="saving" @click="publish">
          {{ form.mode === 'replace' ? $t('覆盖发布') : $t('发布') }}
        </el-button>
      </template>
      <template v-else>
        <el-button @click="emit('update:visible', false)">
          {{ $t('完成') }}
        </el-button>
        <el-button type="primary" @click="() => { emit('update:visible', false); emit('openGallery') }">
          {{ $t('查看模板中心') }}
        </el-button>
      </template>
    </template>
  </el-dialog>
</template>
