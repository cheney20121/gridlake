<script setup lang="ts">
import type { Connector } from '@/api/connector'
import type { DatasetInfo } from '@/api/dataset'
import type { SavedQuery, SqlColumnKind, SqlSchedule } from '@/api/sql'
import { KIND_LABEL } from './format'

/** 保存查询：名称、说明，可另存为数据集（列类型、按计划 / 数据同步后重算） */
export interface SaveForm {
  name: string
  description: string
  asDataset: boolean
  datasetName: string
  kinds: Record<string, SqlColumnKind>
  scheduleKind: SqlSchedule['kind']
  at: string
  connectorId: string
  /** 「数据集更新后」重算：哪个数据集 */
  triggerDatasetId: string
}

defineProps<{
  /** 正在编辑的已保存查询；为空时新建 */
  current: SavedQuery | null
  connectors: Connector[]
  /** 「数据集更新后」可选的数据集 */
  datasets: DatasetInfo[]
  saving: boolean
}>()

const emit = defineEmits<{
  (e: 'save'): void
}>()

const visible = defineModel<boolean>('visible', { required: true })
const form = defineModel<SaveForm>('form', { required: true })
</script>

<template>
  <el-dialog v-model="visible" :title="current ? $t('保存查询 · {0}', [current.name]) : $t('保存查询')" width="600px" append-to-body :close-on-click-modal="false">
    <el-form label-width="96px" @submit.prevent>
      <el-form-item :label="$t('查询名称')" required>
        <el-input v-model="form.name" maxlength="60" :placeholder="$t('如：店铺月度 ROI')" />
      </el-form-item>
      <el-form-item :label="$t('说明')">
        <el-input v-model="form.description" maxlength="200" :placeholder="$t('口径说明（可选）')" />
      </el-form-item>
      <el-form-item :label="$t('另存为数据集')">
        <div class="w-full">
          <el-checkbox v-model="form.asDataset" :disabled="!!current?.datasetId">
            {{ $t('把查询结果保存为数据集（全部结果，不限 1,000 行）') }}
          </el-checkbox>
          <div class="text-xs text-gray-500 leading-5">
            {{ $t('之后可像导入的数据一样做透视、图表、仪表盘、DSUMIFS；源数据更新后按计划重算') }}
          </div>
        </div>
      </el-form-item>
      <template v-if="form.asDataset">
        <el-form-item :label="$t('数据集名称')" required>
          <el-input v-if="!current?.datasetId" v-model="form.datasetName" maxlength="40" :placeholder="$t('如：店铺月度ROI')" />
          <span v-else>{{ $t('{0}（保存后立即按新 SQL 重算）', [current.datasetName]) }}</span>
        </el-form-item>
        <el-form-item :label="$t('列类型')">
          <div class="max-h-48 w-full overflow-y-auto">
            <div v-for="(k, name) in form.kinds" :key="name" class="mb-1 flex items-center gap-2">
              <span class="w-40 truncate" :title="String(name)">{{ name }}</span>
              <el-select v-model="form.kinds[name]" size="small" class="!w-28">
                <el-option v-for="(label, kk) in KIND_LABEL" :key="kk" :label="label" :value="kk" />
              </el-select>
            </div>
          </div>
        </el-form-item>
        <el-form-item :label="$t('重算')">
          <div class="w-full flex flex-wrap items-center gap-2">
            <el-select v-model="form.scheduleKind" class="!w-36" data-testid="sql-schedule-kind">
              <el-option :label="$t('手动')" value="manual" />
              <el-option :label="$t('每天')" value="daily" />
              <el-option :label="$t('数据同步后')" value="after_sync" />
              <el-option :label="$t('数据集更新后')" value="after_data" />
            </el-select>
            <el-time-select v-if="form.scheduleKind === 'daily'" v-model="form.at" class="!w-28" start="00:00" step="00:30" end="23:30" :clearable="false" />
            <el-select v-if="form.scheduleKind === 'after_sync'" v-model="form.connectorId" class="flex-1" :placeholder="$t('选择数据连接')" :no-data-text="$t('还没有数据连接')">
              <el-option v-for="c in connectors" :key="c.id" :label="c.name" :value="c.id" />
            </el-select>
            <el-select v-if="form.scheduleKind === 'after_data'" v-model="form.triggerDatasetId" filterable class="flex-1" :placeholder="$t('选择数据集')" data-testid="trigger-dataset">
              <el-option v-for="d in datasets.filter(d => d.id !== current?.datasetId)" :key="d.id" :label="d.name" :value="d.id" />
            </el-select>
            <div v-if="form.scheduleKind === 'after_data'" class="w-full text-xs text-gray-500 leading-5">
              {{ $t('这个数据集导入了新数据、数据连接同步完成、外部文件有变化后重算。填报提交不触发；SQL 查询写入的数据集不会再触发别的查询重算。') }}
            </div>
          </div>
        </el-form-item>
      </template>
    </el-form>
    <template #footer>
      <el-button @click="visible = false">
        {{ $t('取消') }}
      </el-button>
      <el-button type="primary" :loading="saving" @click="emit('save')">
        {{ form.asDataset ? $t('保存并写入数据集') : $t('保存') }}
      </el-button>
    </template>
  </el-dialog>
</template>
