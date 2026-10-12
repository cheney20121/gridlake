<script setup lang="ts">
import { Loading } from '@element-plus/icons-vue'
import { toRef } from 'vue'
import { useBusy } from '@/composables/useBusy'

/** 面板里一行的「计算中…」：已用时间、等久了的原因说明与取消（透视表、图表等） */
const props = defineProps<{
  loading: boolean
  /** 正在做什么，如「计算中」 */
  text?: string
  cancellable?: boolean
}>()

const emit = defineEmits<{ (e: 'cancel'): void }>()

const { seconds, elapsed, note } = useBusy(toRef(props, 'loading'))
</script>

<template>
  <span v-if="loading" class="inline-flex flex-wrap items-center gap-x-2 text-gray-500" role="status" data-busy="inline">
    <el-icon class="is-loading text-[#217346]"><Loading /></el-icon>
    <span>{{ text ?? '计算中' }}… <span v-if="seconds >= 1" class="tabular-nums">{{ elapsed }}</span></span>
    <el-button v-if="cancellable && seconds >= 2" link type="primary" size="small" @click="emit('cancel')">取消</el-button>
    <span v-if="note" class="w-full text-gray-400 leading-5">{{ note }}</span>
  </span>
</template>
