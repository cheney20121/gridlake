<script setup lang="ts">
import { Loading, WarningFilled } from '@element-plus/icons-vue'
import { toRef } from 'vue'
import { useBusy } from '@/composables/useBusy'

/**
 * 盖在数据区上的「正在查询 / 查询失败」提示：
 * 等待时显示已用时间与原因说明（排队 / 数据量大），等久了可以取消；失败时给出服务端的说明与重试
 * 前 0.25 秒不显示（缓存命中的查询瞬间完成，不闪一下）
 */
const props = defineProps<{
  loading: boolean
  error: string
  /** 正在做什么，如「正在查询 1,000,000 行」 */
  title: string
  /** 取消后能恢复到上一次的结果（有上一次结果时才提供取消） */
  cancellable?: boolean
  /** 失败后能还原到上一次的条件 */
  revertable?: boolean
}>()

const emit = defineEmits<{
  (e: 'cancel'): void
  (e: 'retry'): void
  (e: 'revert'): void
}>()

const { seconds, elapsed, note } = useBusy(toRef(props, 'loading'))
</script>

<template>
  <div v-if="loading" class="busy-overlay" role="status" aria-live="polite" data-busy="loading">
    <div class="card">
      <el-icon class="is-loading mt-0.5 shrink-0 text-[#217346]" :size="18">
        <Loading />
      </el-icon>
      <div class="min-w-0 flex-1">
        <div class="font-medium text-gray-800">
          {{ title }}… <span class="text-gray-500 font-normal tabular-nums">已用 {{ elapsed }}</span>
        </div>
        <div v-if="note" class="mt-1 text-xs text-gray-500 leading-5">
          {{ note }}
        </div>
      </div>
      <el-button v-if="cancellable && seconds >= 2" size="small" @click="emit('cancel')">
        取消
      </el-button>
    </div>
  </div>
  <div v-else-if="error" class="busy-overlay solid" role="alert" data-busy="error">
    <div class="card error">
      <el-icon class="mt-0.5 shrink-0 text-[#B91C1C]" :size="18">
        <WarningFilled />
      </el-icon>
      <div class="min-w-0 flex-1">
        <div class="font-medium text-gray-800">
          没有查询成功
        </div>
        <div class="mt-1 text-xs text-gray-600 leading-5">
          {{ error }}
        </div>
      </div>
      <div class="flex shrink-0 gap-2">
        <el-button v-if="revertable" size="small" @click="emit('revert')">
          还原条件
        </el-button>
        <el-button size="small" type="primary" @click="emit('retry')">
          重试
        </el-button>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.busy-overlay {
  @apply absolute inset-0 z-10 flex items-start justify-center bg-white/55 pt-24;

  animation: busy-in 0.15s 0.25s both;

  &.solid {
    @apply bg-white/80;

    animation-delay: 0s;
  }
}

.card {
  @apply flex max-w-[520px] min-w-[300px] items-start gap-3 rounded-lg border border-[#e5e7eb] border-solid bg-white px-4 py-3 text-[13px] shadow-lg;

  &.error {
    @apply border-[#fecaca];
  }
}

@keyframes busy-in {
  from {
    opacity: 0;
  }

  to {
    opacity: 1;
  }
}
</style>
