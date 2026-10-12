<script setup lang="ts">
import type { WorkbookMeta } from '@/api/workbook'
import { ref } from 'vue'

/** 保存冲突：打开后别人保存了新版本，由用户决定怎么处理（不自动覆盖任何一方） */
defineProps<{
  visible: boolean
  conflict: WorkbookMeta | null
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 用我的内容覆盖（对方版本仍保留在版本历史里） */
  (e: 'overwrite'): void
  /** 放弃我的修改，加载对方的最新版本 */
  (e: 'loadTheirs'): void
  /** 把我的内容另存为新工作簿 */
  (e: 'saveCopy'): void
}>()

const busy = ref(false)

function act(event: 'overwrite' | 'loadTheirs' | 'saveCopy') {
  busy.value = true
  // 父组件处理完成后关闭对话框
  if (event === 'overwrite')
    emit('overwrite')
  else if (event === 'loadTheirs')
    emit('loadTheirs')
  else
    emit('saveCopy')
  setTimeout(() => (busy.value = false), 300)
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    title="保存冲突"
    width="520px"
    :close-on-click-modal="false"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div v-if="conflict" class="flex flex-col gap-2 text-[13px] leading-6">
      <div>
        「<b>{{ conflict.updatedBy || '其他人' }}</b>」于 {{ conflict.updatedAt }} 保存了新版本（第 {{ conflict.version }} 版），
        你打开后做的修改还没有保存。
      </div>
      <div class="rounded bg-[#F7F8FA] px-3 py-2 text-xs text-gray-600 leading-5">
        <div><b>覆盖保存</b>：用你的内容生成新版本；对方的版本仍在版本历史中，可随时恢复</div>
        <div><b>加载对方版本</b>：放弃你未保存的修改</div>
        <div><b>另存为副本</b>：你的内容保存为一个新的工作簿，两份都保留</div>
      </div>
      <div class="text-xs text-gray-400">
        在问题解决前自动保存已暂停
      </div>
    </div>
    <template #footer>
      <el-button :disabled="busy" @click="act('saveCopy')">
        另存为副本
      </el-button>
      <el-button :disabled="busy" @click="act('loadTheirs')">
        加载对方版本
      </el-button>
      <el-button type="primary" :disabled="busy" @click="act('overwrite')">
        覆盖保存
      </el-button>
    </template>
  </el-dialog>
</template>
