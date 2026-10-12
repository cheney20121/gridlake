<script setup lang="ts">
import type { FolderForm } from './form-state'

/** 文件夹来源：收取服务器上某个目录里的 CSV / XLSX */
defineProps<{
  inboxRoots: string[]
}>()

const folder = defineModel<FolderForm>({ required: true })
</script>

<template>
  <div class="hint">
    {{ $t('收取服务器上某个目录（可以是挂载的共享盘）里的 CSV / XLSX：每次只导入还没导入过的文件；文件被覆盖更新后会重新导入。 可用目录：') }}<code v-for="r in inboxRoots" :key="r" class="mr-2">{{ r }}</code>
  </div>
  <div class="row">
    <span class="label">{{ $t('目录') }}</span>
    <el-input v-model="folder.path" class="!w-[480px]" :placeholder="$t('相对于 {0} 的子目录，如 抖店；或完整路径', [inboxRoots[0] ?? '收件目录'])" />
  </div>
  <div class="row">
    <span class="label">{{ $t('文件名规则') }}</span>
    <el-input v-model="folder.pattern" class="!w-72" :placeholder="$t('如 抖店资金流水_*.csv')" />
    <span class="text-xs text-gray-400">{{ $t('* 任意字符、? 单个字符，多个规则用 ; 分隔') }}</span>
  </div>
</template>

<style scoped lang="scss">
.row {
  @apply flex items-center gap-3;
}

.label {
  @apply w-20 shrink-0 text-gray-500;
}

.hint {
  @apply rounded bg-[#F7F8FA] px-3 py-2 text-xs text-gray-600 leading-5;
}
</style>
