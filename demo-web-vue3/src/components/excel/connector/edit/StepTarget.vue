<script setup lang="ts">
import type { ConnectorForm } from './form-state'
import type { ConnectorPreview } from '@/api/connector'
import type { DatasetInfo } from '@/api/dataset'
import { KIND_OPTIONS, MODE_OPTIONS, PH } from './form-state'

/** 第 ② 步：预览样例数据，选择同步目标（新建 / 已有数据集）、同步方式、主键与增量，并调整要导入的列 */
defineProps<{
  preview: ConnectorPreview | null
  previewing: boolean
  userDatasets: DatasetInfo[]
  targetDataset?: DatasetInfo
  targetMatch?: ConnectorPreview['matches'][number]
  keyOptions: string[]
  sourceColumns: string[]
  needsKey: boolean
  isDb: boolean
  isHttp: boolean
  /** 地址 / 请求体里用到的占位符（增量需要 {{watermark}}） */
  placeholders: string[]
  editing: boolean
  /** 修改时：当前同步到的增量位置 */
  watermark?: string | null
  targetError: string
}>()

const emit = defineEmits<{
  (e: 'reload'): void
}>()

const form = defineModel<ConnectorForm>({ required: true })
</script>

<template>
  <div v-loading="previewing" element-loading-text="正在读取样例数据…" class="flex flex-col gap-3 text-[13px]">
    <div v-if="preview" class="flex flex-wrap items-center gap-x-4 gap-y-1 text-gray-600">
      <span>样例：<span class="text-gray-900 font-medium">{{ preview.sampleOf }}</span></span>
      <span>{{ preview.columns.length }} 列</span>
      <el-button link type="primary" size="small" @click="emit('reload')">
        重新预览
      </el-button>
    </div>
    <div v-else-if="!previewing" class="hint">
      还没有样例数据（文件夹里没有匹配的文件，或连接失败）。
      <el-button link type="primary" @click="emit('reload')">
        重新预览
      </el-button>
    </div>

    <div class="row">
      <span class="label">同步到</span>
      <el-radio-group v-model="form.targetType">
        <el-radio-button value="new">
          新建数据集
        </el-radio-button>
        <el-radio-button value="existing" :disabled="!userDatasets.length">
          已有数据集
        </el-radio-button>
      </el-radio-group>
      <el-input v-if="form.targetType === 'new'" v-model="form.datasetName" class="!w-64" placeholder="数据集名称（公式里用它引用）" />
      <el-select v-else v-model="form.datasetId" class="!w-80" placeholder="选择数据集">
        <el-option v-for="d in userDatasets" :key="d.id" :label="d.name" :value="d.id">
          <span>{{ d.name }}</span>
          <span class="ml-2 text-xs text-gray-400">{{ d.rowCount.toLocaleString() }} 行</span>
          <span v-if="preview?.matches.find(m => m.datasetId === d.id)" class="ml-2 text-xs text-[#0B5A2E]">
            列匹配 {{ preview.matches.find(m => m.datasetId === d.id)!.matched }}/{{ preview.matches.find(m => m.datasetId === d.id)!.total }}
          </span>
        </el-option>
      </el-select>
    </div>
    <div v-if="form.targetType === 'existing' && targetDataset && preview" class="ml-[92px] text-xs" :class="targetMatch ? 'text-gray-500' : 'text-[#B91C1C]'">
      <template v-if="targetMatch">
        按列名对齐：对上 {{ targetMatch.matched }}/{{ targetMatch.total }} 列<span v-if="targetMatch.missing.length" class="text-[#92400E]">；来源缺少 {{ targetMatch.missing.join('、') }}（写入空值）</span>
      </template>
      <template v-else>
        来源的列名与该数据集对不上，可能选错了数据集
      </template>
    </div>

    <div class="row items-start!">
      <span class="label pt-1">同步方式</span>
      <el-radio-group v-model="form.mode" class="flex-col items-start! gap-1">
        <el-radio v-for="m in MODE_OPTIONS" :key="m.value" :value="m.value">
          <span class="font-medium">{{ m.label }}</span>
          <span class="ml-2 text-xs text-gray-400">{{ m.hint }}</span>
        </el-radio>
      </el-radio-group>
    </div>
    <div class="row">
      <span class="label">主键</span>
      <el-select v-model="form.primaryKey" multiple collapse-tags clearable class="!w-72" :placeholder="needsKey ? '必填：用于去重 / 覆盖' : '可选：用于去重（如订单号）'">
        <el-option v-for="t in keyOptions" :key="t" :label="t" :value="t" />
      </el-select>
      <template v-if="isDb">
        <span class="text-gray-500">增量列</span>
        <el-select v-model="form.db.incrementalColumn" clearable filterable class="!w-48" placeholder="不设置：每次全量读取">
          <el-option v-for="c in sourceColumns" :key="c" :label="c" :value="c" />
        </el-select>
      </template>
      <template v-if="isHttp">
        <span class="text-gray-500">增量字段</span>
        <el-select v-model="form.http.incrementalField" clearable filterable class="!w-48" placeholder="不设置：每次全量读取">
          <el-option v-for="c in sourceColumns" :key="c" :label="c" :value="c" />
        </el-select>
        <template v-if="form.http.incrementalField">
          <span class="text-gray-500">首次从</span>
          <el-input v-model="form.http.initialWatermark" class="!w-44" placeholder="如 2026-01-01 00:00:00" />
        </template>
      </template>
    </div>
    <div v-if="isHttp && form.http.incrementalField" class="ml-[92px] text-xs leading-5" :class="placeholders.includes(PH.watermark) ? 'text-gray-500' : 'text-[#B45309]'">
      <template v-if="placeholders.includes(PH.watermark)">
        每次把上次取到的「{{ form.http.incrementalField }}」最大值代入 {{ PH.watermark }}（首次用上面的起始值），重复取到的记录由主键去重。
        <span v-if="editing && watermark">当前同步到：{{ watermark }}</span>
      </template>
      <template v-else>
        增量同步需要在接口地址或请求体里写 {{ PH.watermark }}（返回第一步填写）
      </template>
    </div>
    <div v-if="isDb && form.db.incrementalColumn" class="ml-[92px] text-xs text-gray-500 leading-5">
      每次只读取「{{ form.db.incrementalColumn }}」≥ 上次同步到的最大值的行（如更新时间、自增 ID）；同一时刻后写入的行不会漏，重复读到的由主键去重。
      <span v-if="editing && watermark">当前同步到：{{ watermark }}</span>
    </div>

    <template v-if="form.targetType === 'new' && form.columns.length">
      <el-table :data="form.columns" size="small" max-height="300">
        <el-table-column width="48" align="center">
          <template #default="{ row }">
            <el-checkbox v-model="row.include" />
          </template>
        </el-table-column>
        <el-table-column label="来源列" prop="source" min-width="130" show-overflow-tooltip />
        <el-table-column label="列名" min-width="150">
          <template #default="{ row }">
            <el-input v-model="row.title" size="small" :disabled="!row.include" />
          </template>
        </el-table-column>
        <el-table-column label="类型" width="120">
          <template #default="{ row }">
            <el-select v-model="row.kind" size="small" :disabled="!row.include">
              <el-option v-for="k in KIND_OPTIONS" :key="k.value" :label="k.label" :value="k.value" />
            </el-select>
          </template>
        </el-table-column>
        <el-table-column label="样例" min-width="220" show-overflow-tooltip>
          <template #default="{ row }">
            <span class="text-gray-500">{{ row.samples.join(' · ') }}</span>
          </template>
        </el-table-column>
      </el-table>
    </template>
    <div v-if="targetError" class="text-xs text-[#B91C1C]">
      {{ targetError }}
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
