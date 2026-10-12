<script setup lang="ts">
import type { ParamDef } from '@/api/sql'
import { KIND_OPTIONS, valueError } from '@/features/excel/sql/params'

/** SQL 里有 {{参数}} 时显示：每个参数的类型与当前值（运行、保存都用它；类型决定怎样代入 SQL，值不会被当作 SQL 语法） */
defineProps<{
  defs: ParamDef[]
}>()

const values = defineModel<Record<string, string>>('values', { required: true })

function set(name: string, v: string) {
  values.value = { ...values.value, [name]: v }
}
</script>

<template>
  <div class="param-bar" data-testid="sql-params">
    <span class="text-xs text-gray-500">{{ $t('参数') }}</span>
    <div v-for="d in defs" :key="d.name" class="param">
      <span class="name" :title="$t('在 SQL 里写 {{{0}}}', [d.name])">{{ d.name }}</span>
      <el-select v-model="d.kind" size="small" class="!w-[72px]" :title="$t('类型决定值怎样代入 SQL（文本加引号、数字不加、日期按日期）')">
        <el-option v-for="k in KIND_OPTIONS" :key="k.value" :label="k.label" :value="k.value" />
      </el-select>
      <el-date-picker
        v-if="d.kind === 'date'"
        :model-value="values[d.name] ?? ''"
        type="date"
        value-format="YYYY-MM-DD"
        size="small"
        class="!w-36"
        :placeholder="$t('选择日期')"
        @update:model-value="(v: string | null) => set(d.name, v ?? '')"
      />
      <el-input
        v-else
        :model-value="values[d.name] ?? ''"
        size="small"
        class="!w-36"
        :placeholder="d.kind === 'number' ? $t('数字') : $t('文本')"
        :class="{ bad: valueError(d.kind, values[d.name] ?? '') }"
        clearable
        @update:model-value="(v: string) => set(d.name, v)"
      />
    </div>
  </div>
</template>

<style scoped lang="scss">
.param-bar {
  @apply flex flex-wrap items-center gap-x-4 gap-y-1 rounded bg-[#F7F8FA] px-2 py-1.5;
}

.param {
  @apply flex items-center gap-1.5;
}

.name {
  @apply rounded bg-[#E7F4EC] px-1.5 py-0.5 text-xs text-[#0E5A31] font-medium;
}

.bad :deep(.#{$namespace}-input__wrapper) {
  box-shadow: 0 0 0 1px #dc2626 inset;
}
</style>
