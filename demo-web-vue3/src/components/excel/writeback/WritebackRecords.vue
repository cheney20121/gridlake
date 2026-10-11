<script setup lang="ts">
import type { WritebackRecord, WritebackSubmitMode } from '@/api/dataset'

/** 某个数据集的提交记录：每次提交的时间、人、新增 / 更新条数，可撤销最近一次 */
defineProps<{
  datasetName: string
  records: WritebackRecord[]
}>()

const emit = defineEmits<{
  (e: 'refresh'): void
  (e: 'undo', r: WritebackRecord): void
}>()

const modeText = (m: WritebackSubmitMode) => (m === 'upsert' ? '按主键更新' : m === 'delete' ? '删除' : '追加')

function sourceText(r: WritebackRecord) {
  const s = r.source
  return s ? [s.sheetName, s.inputName].filter(Boolean).join(' · ') : ''
}
</script>

<template>
  <div class="section">
    <div class="section-title">
      「{{ datasetName }}」的提交记录
      <el-button link size="small" @click="emit('refresh')">
        刷新
      </el-button>
    </div>
    <div class="flex flex-col">
      <div v-for="r in records" :key="r.id" class="rec" :class="{ undone: r.undoneAt }">
        <div class="flex items-center gap-2">
          <span class="tabular-nums">{{ r.createdAt }}</span>
          <span class="text-gray-500">{{ r.operator }}</span>
          <el-tooltip v-if="!r.undoneAt" :disabled="r.canUndo" :content="r.undoBlocked ?? ''" placement="top">
            <span class="ml-auto">
              <el-button link size="small" type="primary" :disabled="!r.canUndo" @click="emit('undo', r)">撤销</el-button>
            </span>
          </el-tooltip>
          <span v-else class="ml-auto text-gray-400">{{ r.undoneBy }} 已撤销</span>
        </div>
        <div class="text-gray-500">
          {{ modeText(r.mode) }}<template v-if="r.mode === 'delete'">
            · 删除 {{ r.deleted }}
          </template><template v-else>
            · 新增 {{ r.inserted }} · 更新 {{ r.updated }}
          </template><template v-if="r.unchanged">
            · 无变化 {{ r.unchanged }}
          </template>
          <template v-if="sourceText(r)">
            · {{ sourceText(r) }}
          </template>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.section {
  @apply flex flex-col gap-1.5;
}

.section-title {
  @apply flex items-center justify-between text-xs font-semibold text-gray-600;
}

.rec {
  @apply flex flex-col gap-0.5 border-b border-[#f0f0f0] border-b-solid py-1.5 text-xs last:border-b-0;

  &.undone {
    @apply opacity-60;
  }
}
</style>
