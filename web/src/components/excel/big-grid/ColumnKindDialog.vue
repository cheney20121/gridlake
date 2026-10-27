<script setup lang="ts">
import type { ColumnKindPreview, DatasetColumnKind } from '@/api/dataset'
import { computed, ref, watch } from 'vue'
import { changeColumnKindApi, previewColumnKindApi } from '@/api/dataset'
import { formatBool, kindLabel, kindOptions } from '@/features/excel/dataset/column-kinds'
import { t } from '@/i18n'
import { errorText } from '@/utils/busy-hint'

/** 修改数据集某一列的类型：先预览（多少值转不过来、转换前后的样子），确认后就地转换 */
const props = defineProps<{
  datasetId: string
  /** 列名与当前类型；null 时关闭 */
  column: { title: string, kind: DatasetColumnKind } | null
}>()
const emit = defineEmits<{
  (e: 'close'): void
  (e: 'changed'): void
}>()

const kind = ref<DatasetColumnKind | ''>('')
const preview = ref<ColumnKindPreview | null>(null)
const loading = ref(false)
const applying = ref(false)
const error = ref('')

watch(() => props.column, () => {
  kind.value = ''
  preview.value = null
  error.value = ''
})

const options = computed(() => kindOptions().filter(o => o.value !== props.column?.kind))

let seq = 0
watch(kind, async (k) => {
  preview.value = null
  error.value = ''
  if (!k || !props.column)
    return
  const my = ++seq
  loading.value = true
  try {
    const p = await previewColumnKindApi(props.datasetId, props.column.title, k)
    if (my === seq)
      preview.value = p
  }
  catch (e) {
    if (my === seq)
      error.value = errorText(e)
  }
  finally {
    if (my === seq)
      loading.value = false
  }
})

function show(v: unknown) {
  if (v == null || v === '')
    return t('（空）')
  return typeof v === 'boolean' ? formatBool(v) : String(v)
}

async function apply() {
  if (!props.column || !kind.value || !preview.value)
    return
  applying.value = true
  error.value = ''
  try {
    await changeColumnKindApi(props.datasetId, props.column.title, kind.value, preview.value.lost > 0)
    ElMessage.success(t('「{0}」已改为{1}列', [props.column.title, kindLabel(kind.value)]))
    emit('changed')
    emit('close')
  }
  catch (e) {
    error.value = errorText(e)
  }
  finally {
    applying.value = false
  }
}
</script>

<template>
  <el-dialog :model-value="!!column" :title="$t('更改列类型')" width="560px" append-to-body :close-on-click-modal="!applying" @update:model-value="!$event && emit('close')">
    <div v-if="column" class="flex flex-col gap-3 text-[13px]">
      <div class="flex items-center gap-2">
        <span class="text-gray-900 font-medium">{{ column.title }}</span>
        <span class="text-gray-500">{{ $t('当前：{0}', [kindLabel(column.kind)]) }}</span>
        <span class="text-gray-400">→</span>
        <el-select v-model="kind" size="small" class="!w-36" :placeholder="$t('选择新类型')" data-testid="column-kind-select">
          <el-option v-for="o in options" :key="o.value" :label="o.label" :value="o.value" />
        </el-select>
      </div>
      <div v-if="loading" class="text-gray-500">
        {{ $t('正在检查每个值能否转换…') }}
      </div>
      <template v-if="preview">
        <div :class="preview.lost ? 'text-[#B91C1C]' : 'text-[#0B5A2E]'">
          <template v-if="preview.lost">
            {{ $t('{0} 个值里有 {1} 个转不成{2}，会变成空值：{3}', [preview.values.toLocaleString(), preview.lost.toLocaleString(), kindLabel(preview.to), preview.lostSamples.map(s => `「${s}」`).join('')]) }}
          </template>
          <template v-else>
            {{ $t('{0} 个值都能转换', [preview.values.toLocaleString()]) }}
          </template>
        </div>
        <div v-if="preview.kept" class="text-[#92400E]">
          {{ $t('{0} 个值认不出是日期，按原文保留', [preview.kept.toLocaleString()]) }}
        </div>
        <div v-for="w in preview.warnings" :key="w" class="text-[#92400E]">
          {{ w }}
        </div>
        <table v-if="preview.samples.length" class="w-full border-collapse text-xs">
          <thead>
            <tr class="text-gray-500">
              <th class="border-b py-1 text-left font-normal">
                {{ $t('现在') }}
              </th>
              <th class="border-b py-1 text-left font-normal">
                {{ $t('改成{0}后', [kindLabel(preview.to)]) }}
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="([before, after], i) in preview.samples" :key="i">
              <td class="py-0.5 pr-3">
                {{ show(before) }}
              </td>
              <td class="py-0.5" :class="{ 'text-[#B91C1C]': after == null }">
                {{ show(after) }}
              </td>
            </tr>
          </tbody>
        </table>
      </template>
      <div v-if="error" class="rounded bg-[#FEF2F2] px-3 py-2 text-[#B91C1C] leading-5" role="alert">
        {{ error }}
      </div>
    </div>
    <template #footer>
      <el-button @click="emit('close')">
        {{ $t('取消') }}
      </el-button>
      <el-button :type="preview?.lost ? 'danger' : 'primary'" :disabled="!preview" :loading="applying" @click="apply">
        {{ preview?.lost ? $t('仍然修改（{0} 个值变成空）', [preview.lost.toLocaleString()]) : $t('修改') }}
      </el-button>
    </template>
  </el-dialog>
</template>
