<script setup lang="ts">
import { ref } from 'vue'
import { aiSetSummaryApi } from '@/api/ai'
import { renderMarkdown } from '@/features/excel/ai/markdown'

/**
 * 较早对话的摘要：点分隔线查看；可以手动改写（修正整理错的数字、补充约定），也可以清除，清除后下次提问时服务端重新整理
 */
const props = defineProps<{
  threadId: string
  summary: string
  /** 回答进行中时不能修改 */
  busy: boolean
}>()

const emit = defineEmits<{
  /** 保存了新的摘要；清除时为 null（调用方重新载入会话） */
  (e: 'saved', summary: string | null): void
}>()

const MAX = 4000
const show = ref(false)
const editing = ref(false)
const draft = ref('')
const saving = ref(false)

function startEdit() {
  draft.value = props.summary
  editing.value = true
}

async function save(text: string | null) {
  saving.value = true
  try {
    await aiSetSummaryApi(props.threadId, text)
    editing.value = false
    emit('saved', text?.trim() || null)
  }
  finally {
    saving.value = false
  }
}

async function clear() {
  const ok = await ElMessageBox.confirm('清除后，下次提问时会把较早的对话重新整理成摘要。', '清除摘要', { type: 'warning', confirmButtonText: '清除' }).then(() => true, () => false)
  if (ok)
    await save(null)
}
</script>

<template>
  <div class="summary">
    <button type="button" class="summary-bar" :title="show ? '收起摘要' : '查看摘要'" @click="show = !show">
      <span class="line" />
      <span>以上较早的对话已整理为摘要，继续提问时带摘要 {{ show ? '▴' : '▾' }}</span>
      <span class="line" />
    </button>
    <template v-if="show">
      <template v-if="editing">
        <el-input v-model="draft" type="textarea" :autosize="{ minRows: 4, maxRows: 14 }" :maxlength="MAX" show-word-limit class="mt-1" placeholder="用几条要点写下之前问过什么、得出的结论和约定" />
        <div class="mt-1 flex justify-end gap-1">
          <el-button size="small" @click="editing = false">
            取消
          </el-button>
          <el-button size="small" type="primary" :loading="saving" :disabled="!draft.trim()" @click="save(draft)">
            保存
          </el-button>
        </div>
      </template>
      <template v-else>
        <!-- eslint-disable-next-line vue/no-v-html 已先转义全部 HTML，仅生成有限标签 -->
        <div class="summary-text" v-html="renderMarkdown(summary)" />
        <div class="mt-1 flex justify-end gap-1">
          <el-button size="small" text :disabled="busy" @click="startEdit">
            编辑摘要
          </el-button>
          <el-button size="small" text :disabled="busy" @click="clear">
            清除
          </el-button>
        </div>
      </template>
    </template>
  </div>
</template>

<style scoped lang="scss">
.summary {
  @apply my-2;
}

.summary-bar {
  @apply w-full flex cursor-pointer items-center gap-2 border-none bg-transparent p-0 text-[11px] text-gray-400 hover:text-gray-600;

  .line {
    @apply h-px flex-1 bg-[#e5e7eb];
  }
}

.summary-text {
  @apply mt-1 rounded-md bg-[#f9f9f7] px-2.5 py-1.5 text-xs text-gray-600 leading-5;
}
</style>
