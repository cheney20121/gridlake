<script setup lang="ts">
import type { AiAction } from '@/api/ai'
import AiActionBody from './AiActionBody.vue'
import { KIND_LABEL, stepTitle } from './constants'

/** AI 建议卡：单个建议或方案卡（有序步骤），预览后应用 / 忽略，应用后可撤销 */
defineProps<{
  action: AiAction
  /** 正在应用或撤销这张卡 */
  applying: boolean
}>()

const emit = defineEmits<{
  (e: 'apply'): void
  (e: 'dismiss'): void
  (e: 'undo'): void
}>()
</script>

<template>
  <div class="card" :class="action.status">
    <div class="flex items-center gap-2">
      <span class="tag">{{ KIND_LABEL[action.kind] }}</span>
      <span class="min-w-0 flex-1 truncate font-medium">{{ action.title }}</span>
    </div>
    <AiActionBody v-if="action.kind !== 'plan'" :kind="action.kind" :payload="action.payload" />
    <!-- 方案卡：有序步骤，预览整体后一次应用、一次撤销 -->
    <ol v-else class="plan-steps">
      <li v-for="(st, si) in action.payload.steps" :key="si">
        <div class="flex items-center gap-2">
          <span class="step-no">{{ Number(si) + 1 }}</span>
          <span class="tag">{{ KIND_LABEL[st.kind] }}</span>
          <span class="min-w-0 flex-1 truncate" :title="stepTitle(st)">{{ stepTitle(st) }}</span>
        </div>
        <AiActionBody :kind="st.kind" :payload="st" class="ml-6" />
      </li>
    </ol>
    <div class="mt-2 flex items-center gap-2">
      <template v-if="action.status === 'proposed'">
        <el-button size="small" type="primary" :loading="applying" @click="emit('apply')">
          {{ action.kind === 'plan' ? $t('应用方案（{0} 步）', [action.payload.steps.length]) : $t('应用') }}
        </el-button>
        <el-button size="small" text @click="emit('dismiss')">
          {{ $t('忽略') }}
        </el-button>
      </template>
      <template v-else-if="action.status === 'applied'">
        <span class="min-w-0 flex-1 text-xs text-[#0B5A2E]">
          ✓ {{ action.result }}<span class="text-gray-400">{{ action.decidedBy ? ` · ${action.decidedBy}` : '' }}</span>
        </span>
        <el-button v-if="action.undo" size="small" text :loading="applying" @click="emit('undo')">
          {{ $t('撤销') }}
        </el-button>
      </template>
      <span v-else-if="action.status === 'undone'" class="text-xs text-gray-500">{{ $t('已撤销：{0}', [action.result]) }}</span>
      <span v-else class="text-xs text-gray-400">{{ $t('已忽略') }}</span>
    </div>
  </div>
</template>

<style scoped lang="scss">
.card {
  @apply my-1.5 rounded-md border border-[#d7e9dd] border-solid bg-[#fbfdfb] px-2.5 py-2;

  &.dismissed {
    @apply opacity-60;
  }
}

.tag {
  @apply shrink-0 rounded bg-[#E7F4EC] px-1.5 text-xs text-[#217346];
}

.plan-steps {
  @apply m-0 mt-1.5 list-none p-0;

  li {
    @apply mb-1.5 border-l-2 border-l-[#d7e9dd] border-l-solid pl-2;
  }
}

.step-no {
  @apply inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-[#217346] text-[10px] text-white;
}
</style>
