<script setup lang="ts">
import type { ConnectorForm } from './form-state'
import type { DatasetInfo } from '@/api/dataset'
import { MODE_OPTIONS } from './form-state'

/** 第 ③ 步：同步计划（每天 / 定时间隔 / 仅手动）、启用状态，以及来源与目标的汇总 */
defineProps<{
  targetDataset?: DatasetInfo
}>()

const form = defineModel<ConnectorForm>({ required: true })
</script>

<template>
  <div class="flex flex-col gap-3 text-[13px]">
    <div class="row">
      <span class="label">{{ $t('同步计划') }}</span>
      <el-radio-group v-model="form.schedule">
        <el-radio-button value="daily">
          {{ $t('每天') }}
        </el-radio-button>
        <el-radio-button value="interval">
          {{ $t('定时间隔') }}
        </el-radio-button>
        <el-radio-button value="manual">
          {{ $t('仅手动') }}
        </el-radio-button>
      </el-radio-group>
      <el-time-select v-if="form.schedule === 'daily'" v-model="form.at" start="00:00" end="23:30" step="00:30" class="!w-28" :clearable="false" />
      <template v-else-if="form.schedule === 'interval'">
        <span class="text-gray-500">{{ $t('每') }}</span>
        <el-input-number v-model="form.minutes" :min="5" :max="1440" :step="5" class="!w-32" />
        <span class="text-gray-500">{{ $t('分钟') }}</span>
      </template>
    </div>
    <div class="hint">
      <template v-if="form.schedule === 'daily'">
        {{ $t('每天 {0} 自动同步一次（服务器时间）。适合 T+1 的报表、商品成本等。', [form.at]) }}
      </template>
      <template v-else-if="form.schedule === 'interval'">
        {{ $t('每 {0} 分钟同步一次，保存后很快会执行第一次。适合订单、收件目录等需要较新数据的场景。', [form.minutes]) }}
      </template>
      <template v-else>
        {{ $t('只在点击「立即同步」时执行。') }}
      </template>
      {{ $t('同步失败时记录原因，下次按计划自动重试；数据集与增量位置不会留下半截数据。') }}
    </div>
    <div class="row">
      <span class="label">{{ $t('状态') }}</span>
      <el-switch v-model="form.enabled" :active-text="$t('启用')" :inactive-text="$t('停用')" />
    </div>
    <div class="row">
      <span class="label" />
      <el-checkbox v-model="form.runNow">
        {{ $t('保存后立即同步一次') }}
      </el-checkbox>
    </div>
    <div class="rounded bg-[#F7F8FA] px-3 py-2 text-xs text-gray-600 leading-6">
      <div>{{ $t('来源：{0}', [form.sourceKind === 'folder' ? $t('文件夹 {0} / {1}', [form.folder.path, form.folder.pattern]) : `${form.sourceKind === 'mysql' ? 'MySQL' : 'PostgreSQL'} ${form.db.host}:${form.db.port}/${form.db.database}`]) }}</div>
      <div>{{ $t('目标：{0}，{1}', [form.targetType === 'new' ? $t('新建数据集「{0}」', [form.datasetName]) : $t('数据集「{0}」', [targetDataset?.name ?? '']), MODE_OPTIONS.find(m => m.value === form.mode)?.label]) }}<span v-if="form.primaryKey.length">{{ $t('，主键 {0}', [form.primaryKey.join(' + ')]) }}</span></div>
    </div>
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
