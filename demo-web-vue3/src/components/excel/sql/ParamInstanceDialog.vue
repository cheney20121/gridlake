<script setup lang="ts">
import type { ParamDef, SavedQuery } from '@/api/sql'
import { computed, reactive, ref, watch } from 'vue'
import { instantiateQueryApi } from '@/api/sql'
import { KIND_OPTIONS, paramProblems } from '@/features/excel/sql/params'

/** 用一组参数值从参数化查询生成实例：独立的查询 + 一个新数据集（如「分店订单（店铺=B）」），可各自重算 */
const props = defineProps<{
  visible: boolean
  query: SavedQuery | null
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'created', q: SavedQuery): void
}>()

const values = reactive<Record<string, string>>({})
const name = ref('')
const saving = ref(false)

const defs = computed<ParamDef[]>(() => props.query?.params ?? [])
const problems = computed(() => paramProblems(defs.value, values))

watch(() => props.visible, (v) => {
  if (!v)
    return
  for (const k of Object.keys(values)) delete values[k]
  for (const d of defs.value) values[d.name] = props.query?.values[d.name] ?? d.default ?? ''
  name.value = ''
})

const kindLabel = (k: string) => KIND_OPTIONS.find(o => o.value === k)?.label ?? k

async function create() {
  if (!props.query || problems.value.length)
    return
  saving.value = true
  try {
    const q = await instantiateQueryApi(props.query.id, { ...values }, name.value.trim())
    ElMessage.success(`已生成数据集「${q.datasetName}」（${q.lastRows?.toLocaleString()} 行）`)
    emit('created', q)
    emit('update:visible', false)
  }
  finally {
    saving.value = false
  }
}
</script>

<template>
  <el-dialog :model-value="visible" class="param-instance" :title="`生成实例 · ${query?.name ?? ''}`" width="520px" append-to-body :close-on-click-modal="false" @update:model-value="emit('update:visible', $event)">
    <div class="flex flex-col gap-3 text-[13px]">
      <div class="text-xs text-gray-500 leading-5">
        用下面的参数值运行这个查询，把结果保存成一个新数据集。实例是独立的：之后可以单独重算、修改参数值，不影响原查询和其他实例。
      </div>
      <div v-for="d in defs" :key="d.name" class="grid grid-cols-[110px_1fr] items-center gap-2">
        <span class="truncate text-gray-600" :title="d.name">{{ d.name }} <span class="text-xs text-gray-400">{{ kindLabel(d.kind) }}</span></span>
        <el-date-picker v-if="d.kind === 'date'" v-model="values[d.name]" type="date" value-format="YYYY-MM-DD" size="small" class="!w-full" />
        <el-input v-else v-model="values[d.name]" size="small" :placeholder="d.default ? `默认 ${d.default}` : ''" />
      </div>
      <div class="grid grid-cols-[110px_1fr] items-center gap-2">
        <span class="text-gray-600">数据集名称</span>
        <el-input v-model="name" size="small" maxlength="40" placeholder="不填则按「查询名（参数=值）」" />
      </div>
      <div v-for="p in problems" :key="p" class="text-xs text-[#B45309]">
        {{ p }}
      </div>
    </div>
    <template #footer>
      <el-button @click="emit('update:visible', false)">
        取消
      </el-button>
      <el-button type="primary" :disabled="!!problems.length" :loading="saving" @click="create">
        生成数据集
      </el-button>
    </template>
  </el-dialog>
</template>
