<script setup lang="ts">
import type { DatasetColumn, DatasetColumnKind, DatasetInfo, WritebackMode } from '@/api/dataset'
import { watch } from 'vue'
import { kindOptions } from '@/features/excel/dataset/column-kinds'
import { cellName } from '@/features/excel/writeback/input-table'

/** 新建填报区的表单：写入已有数据集（选填报列、方式、主键）或新建数据集（定义列与主键） */
export interface CreateForm {
  target: 'existing' | 'new'
  name: string
  datasetId: string
  fields: string[]
  mode: WritebackMode
  keys: string[]
  newName: string
  newColumns: { title: string, kind: DatasetColumnKind }[]
  newKeys: string[]
}

const props = defineProps<{
  /** 可写入的数据集（导入的数据表；关联视图、内置数据集不能写入） */
  writable: DatasetInfo[]
  anchor: { row: number, col: number }
  busy: boolean
  /** 选中的目标数据集及其可填报的列、是否登记了主键、新建数据集时已填的列名 */
  formDataset: DatasetInfo | null
  formColumns: DatasetColumn[]
  formHasPk: boolean
  newTitles: string[]
}>()

const emit = defineEmits<{
  (e: 'cancel'): void
  (e: 'create'): void
}>()

const form = defineModel<CreateForm>({ required: true })

const KIND_OPTIONS: { value: DatasetColumnKind, label: string }[] = kindOptions()

watch(() => form.value.datasetId, () => {
  const ds = props.formDataset
  form.value.fields = props.formColumns.map(c => c.title)
  form.value.keys = [...(ds?.primaryKey ?? [])]
  form.value.mode = ds?.primaryKey.length ? 'upsert' : 'append'
})

function addNewColumn() {
  form.value.newColumns.push({ title: '', kind: 'money' })
}
</script>

<template>
  <div class="card">
    <div class="flex items-center">
      <span class="font-semibold">{{ $t('新建填报区') }}</span>
      <span class="ml-2 text-xs text-gray-400">{{ $t('从当前选中的 {0} 开始（先在表格里点选位置）', [cellName(anchor.row, anchor.col)]) }}</span>
    </div>
    <div class="grid grid-cols-[72px_1fr] items-center gap-x-2 gap-y-2 text-xs text-gray-600">
      <span>{{ $t('名称') }}</span>
      <el-input v-model="form.name" size="small" maxlength="30" />
      <span>{{ $t('写入到') }}</span>
      <el-radio-group v-model="form.target" size="small">
        <el-radio-button value="existing">
          {{ $t('已有数据集') }}
        </el-radio-button>
        <el-radio-button value="new">
          {{ $t('新建数据集') }}
        </el-radio-button>
      </el-radio-group>

      <template v-if="form.target === 'existing'">
        <span>{{ $t('数据集') }}</span>
        <el-select v-model="form.datasetId" size="small" filterable :placeholder="$t('选择数据集')">
          <el-option v-for="d in writable" :key="d.id" :label="$t('{0}（{1} 行）', [d.name, d.rowCount.toLocaleString()])" :value="d.id" />
        </el-select>
        <template v-if="formDataset">
          <span class="self-start pt-1">{{ $t('填报列') }}</span>
          <el-checkbox-group v-model="form.fields" size="small" class="flex flex-wrap gap-x-3">
            <el-checkbox v-for="c in formColumns" :key="c.key" :value="c.title" :label="c.title" />
          </el-checkbox-group>
          <span>{{ $t('方式') }}</span>
          <el-radio-group v-model="form.mode" size="small">
            <el-radio value="upsert">
              {{ $t('按主键更新') }}
            </el-radio>
            <el-radio value="append">
              {{ $t('追加') }}
            </el-radio>
          </el-radio-group>
          <template v-if="form.mode === 'upsert'">
            <span>{{ $t('主键') }}</span>
            <span v-if="formHasPk">{{ $t('{0}（数据集登记的主键）', [formDataset.primaryKey.join('、')]) }}</span>
            <el-select v-else v-model="form.keys" size="small" multiple :placeholder="$t('选择主键列（可多列）')">
              <el-option v-for="f in form.fields" :key="f" :label="f" :value="f" />
            </el-select>
          </template>
        </template>
        <div v-if="!writable.length" class="col-span-2 text-gray-400">
          {{ $t('还没有可写入的数据集（关联视图、内置数据集不能写入），可选择「新建数据集」') }}
        </div>
      </template>

      <template v-else>
        <span>{{ $t('数据集名称') }}</span>
        <el-input v-model="form.newName" size="small" maxlength="40" :placeholder="$t('如：2026 年店铺预算')" />
        <span class="self-start pt-1">{{ $t('列') }}</span>
        <div class="flex flex-col gap-1">
          <div v-for="(c, i) in form.newColumns" :key="i" class="flex items-center gap-1">
            <el-input v-model="c.title" size="small" :placeholder="$t('列名')" class="flex-1" />
            <el-select v-model="c.kind" size="small" class="!w-20">
              <el-option v-for="k in KIND_OPTIONS" :key="k.value" :label="k.label" :value="k.value" />
            </el-select>
            <span class="remove" @click="form.newColumns.splice(i, 1)">×</span>
          </div>
          <el-button link size="small" type="primary" class="self-start" @click="addNewColumn">
            {{ $t('+ 加一列') }}
          </el-button>
        </div>
        <span>{{ $t('主键') }}</span>
        <el-select v-model="form.newKeys" size="small" multiple :placeholder="$t('可选：如 店铺 + 月份')">
          <el-option v-for="t in newTitles" :key="t" :label="t" :value="t" />
        </el-select>
        <span />
        <span class="text-gray-400">{{ $t('设了主键时按主键更新（同一店铺同一月份再次提交会覆盖），否则每次提交追加') }}</span>
      </template>
    </div>
    <div class="flex justify-end gap-2">
      <el-button size="small" @click="emit('cancel')">
        {{ $t('取消') }}
      </el-button>
      <el-button size="small" type="primary" :loading="busy" @click="emit('create')">
        {{ $t('放置填报区') }}
      </el-button>
    </div>
  </div>
</template>

<style scoped lang="scss">
.card {
  @apply flex flex-col gap-2 rounded border border-[#e5e7eb] border-solid p-2.5;
}

.remove {
  @apply cursor-pointer px-1 text-gray-400 hover:text-[#B91C1C];
}
</style>
