<script setup lang="ts">
import type { DatasetInfo } from '@/api/dataset'
import type { Agg, BuilderModel, FilterOp } from '@/features/excel/sql/builder'
import { computed } from 'vue'
import { AGG_LABEL, emptyModel, OP_LABEL, sortOptions } from '@/features/excel/sql/builder'

/** 查询构建器：选数据集、分组列、汇总、筛选、排序，实时预览生成的 SQL */
const props = defineProps<{
  datasets: DatasetInfo[]
  /** 按构建器模型生成的 SQL（预览用） */
  builderSql: string
}>()

const model = defineModel<BuilderModel>({ required: true })

const builderDs = computed(() => props.datasets.find(d => d.name === model.value.dataset))

function setBuilderDataset(name: string) {
  Object.assign(model.value, emptyModel(name))
}

function addField() {
  model.value.fields.push({ column: builderDs.value?.columns[0]?.title ?? '', granularity: null })
}
function addMeasure() {
  const money = builderDs.value?.columns.find(c => c.kind === 'money' || c.kind === 'number')
  model.value.measures.push(money ? { agg: 'sum', column: money.title, alias: '' } : { agg: 'count', column: null, alias: '' })
}
function addFilter() {
  model.value.filters.push({ column: builderDs.value?.columns[0]?.title ?? '', op: '=', value: '', value2: '' })
}
const isDate = (col: string) => builderDs.value?.columns.find(c => c.title === col)?.kind === 'datetime'
</script>

<template>
  <div class="builder">
    <div class="row">
      <span class="label">数据集</span>
      <el-select :model-value="model.dataset" size="small" filterable placeholder="选择数据集" class="!w-60" @update:model-value="setBuilderDataset">
        <el-option v-for="d in datasets" :key="d.id" :label="d.name" :value="d.name" />
      </el-select>
    </div>
    <template v-if="builderDs">
      <div class="row items-start">
        <span class="label">{{ model.measures.length ? '分组' : '列' }}</span>
        <div class="flex flex-1 flex-wrap items-center gap-2">
          <div v-for="(f, i) in model.fields" :key="i" class="chip">
            <el-select v-model="f.column" size="small" class="!w-36">
              <el-option v-for="c in builderDs.columns" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
            <el-select v-if="isDate(f.column)" v-model="f.granularity" size="small" class="!w-20" placeholder="原值">
              <el-option label="原值" :value="(null as unknown as string)" />
              <el-option label="按年" value="year" />
              <el-option label="按月" value="month" />
              <el-option label="按日" value="day" />
            </el-select>
            <span class="x" @click="model.fields.splice(i, 1)">×</span>
          </div>
          <el-button size="small" link type="primary" @click="addField">
            + 添加
          </el-button>
          <span v-if="!model.fields.length && !model.measures.length" class="text-xs text-gray-400">不选则列出全部列</span>
        </div>
      </div>
      <div class="row items-start">
        <span class="label">汇总</span>
        <div class="flex flex-1 flex-wrap items-center gap-2">
          <div v-for="(m, i) in model.measures" :key="i" class="chip">
            <el-select v-model="m.agg" size="small" class="!w-24">
              <el-option v-for="(label, k) in AGG_LABEL" :key="k" :label="label" :value="(k as Agg)" />
            </el-select>
            <el-select v-model="m.column" size="small" class="!w-36" :placeholder="m.agg === 'count' ? '全部行' : '选择列'" clearable>
              <el-option v-for="c in builderDs.columns.filter(c => m.agg === 'count' || m.agg === 'count_distinct' || m.agg === 'min' || m.agg === 'max' || ['money', 'number'].includes(c.kind))" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
            <el-input v-model="m.alias" size="small" class="!w-28" placeholder="结果列名" />
            <span class="x" @click="model.measures.splice(i, 1)">×</span>
          </div>
          <el-button size="small" link type="primary" @click="addMeasure">
            + 添加
          </el-button>
        </div>
      </div>
      <div class="row items-start">
        <span class="label">筛选</span>
        <div class="flex flex-1 flex-col gap-1.5">
          <div v-for="(f, i) in model.filters" :key="i" class="chip self-start">
            <el-select v-model="f.column" size="small" class="!w-36">
              <el-option v-for="c in builderDs.columns" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
            <el-select v-model="f.op" size="small" class="!w-40">
              <el-option v-for="(label, k) in OP_LABEL" :key="k" :label="label" :value="(k as FilterOp)" />
            </el-select>
            <template v-if="f.op !== 'empty' && f.op !== 'not_empty'">
              <el-input v-model="f.value" size="small" class="!w-40" :placeholder="isDate(f.column) ? '如 2026-09-01' : '值'" />
              <template v-if="f.op === 'between'">
                <span class="text-gray-400">~</span>
                <el-input v-model="f.value2" size="small" class="!w-40" :placeholder="isDate(f.column) ? '如 2026-09-30' : '值'" />
              </template>
            </template>
            <span class="x" @click="model.filters.splice(i, 1)">×</span>
          </div>
          <el-button size="small" link type="primary" class="self-start" @click="addFilter">
            + 添加条件
          </el-button>
        </div>
      </div>
      <div class="row">
        <span class="label">排序</span>
        <el-select :model-value="model.sort?.by ?? ''" size="small" class="!w-44" clearable placeholder="默认" @update:model-value="(v: string) => model.sort = v ? { by: v, desc: model.sort?.desc ?? true } : null">
          <el-option v-for="o in sortOptions(model, builderDs)" :key="o" :label="o" :value="o" />
        </el-select>
        <el-select v-if="model.sort" v-model="model.sort.desc" size="small" class="!w-24">
          <el-option label="从大到小" :value="true" />
          <el-option label="从小到大" :value="false" />
        </el-select>
        <span class="label ml-4">最多</span>
        <el-input-number v-model="model.limit" size="small" :min="1" :max="1000000" :step="100" controls-position="right" class="!w-32" />
        <span class="text-xs text-gray-400">行</span>
      </div>
      <pre class="preview-sql">{{ builderSql }}</pre>
    </template>
  </div>
</template>

<style scoped lang="scss">
.builder {
  @apply max-h-[50%] shrink-0 overflow-y-auto rounded-md border border-[#e5e7eb] border-solid p-3 text-[13px];

  .row {
    @apply mb-2 flex items-center gap-2;
  }

  .label {
    @apply w-12 shrink-0 text-gray-500 leading-6;
  }

  .chip {
    @apply flex items-center gap-1 rounded bg-[#f9f9f7] px-1.5 py-1;
  }

  .x {
    @apply cursor-pointer px-1 text-gray-400;

    &:hover {
      @apply text-[#B91C1C];
    }
  }
}

.preview-sql {
  @apply m-0 mt-1 whitespace-pre-wrap rounded bg-[#f4f4f1] p-2 text-xs text-gray-600 font-mono;
}
</style>
