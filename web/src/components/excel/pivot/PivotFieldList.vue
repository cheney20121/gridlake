<script setup lang="ts">
import type { Area } from './constants'
import type { DatasetColumn } from '@/api/dataset'
import draggable from 'vuedraggable'
import { AREAS, cloneField, KIND_ICON } from './constants'

/** 字段列表：可搜索，拖到下方区域或点 + 添加（已用的字段加粗） */
defineProps<{
  fields: DatasetColumn[]
  used: Set<string>
}>()

const emit = defineEmits<{
  (e: 'add', area: Area, col: DatasetColumn): void
}>()

const search = defineModel<string>('search', { required: true })
</script>

<template>
  <div class="section">
    <div class="section-title">
      {{ $t('字段') }}
      <el-input v-model="search" size="small" clearable :placeholder="$t('搜索')" class="!w-28" />
    </div>
    <draggable
      :list="fields"
      :group="{ name: 'pivot', pull: 'clone', put: false }"
      :clone="cloneField"
      :sort="false"
      item-key="key"
      class="field-list"
    >
      <template #item="{ element }">
        <div class="field" :class="{ used: used.has(element.title) }" :title="$t('拖到下方区域，或点右侧按钮添加')">
          <span class="kind">{{ KIND_ICON[element.kind] ?? 'Aa' }}</span>
          <span class="min-w-0 flex-1 truncate">{{ element.title }}</span>
          <el-dropdown trigger="click" size="small" @command="(a: Area) => emit('add', a, element)">
            <span class="add">+</span>
            <template #dropdown>
              <el-dropdown-menu>
                <el-dropdown-item v-for="a in AREAS" :key="a.key" :command="a.key">
                  {{ $t('添加到{0}', [a.label]) }}
                </el-dropdown-item>
              </el-dropdown-menu>
            </template>
          </el-dropdown>
        </div>
      </template>
    </draggable>
  </div>
</template>

<style scoped lang="scss">
.section {
  @apply flex flex-col gap-1;
}

.section-title {
  @apply flex items-center justify-between text-xs font-semibold text-gray-600;
}

.field-list {
  @apply max-h-44 overflow-auto rounded border border-[#e5e7eb] border-solid p-1;
}

.field {
  @apply flex cursor-grab items-center gap-1.5 rounded px-1.5 py-1 hover:bg-[#F3F4F6];

  &.used {
    @apply font-semibold text-[#0E5A31];
  }
}

.kind {
  @apply w-5 shrink-0 text-center text-[10px] text-gray-400;
}

.add {
  @apply cursor-pointer rounded px-1 text-gray-400 hover:bg-gray-200 hover:text-gray-700;
}
</style>
