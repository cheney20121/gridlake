<script setup lang="ts">
import type { DatasetColumn } from '@/api/dataset'
import type { CellTableSpec, DataTableSpec } from '@/features/excel/datatable/types'
import { computed } from 'vue'
import { MAX_CELL_ROWS } from '@/features/excel/datatable/types'

/**
 * 数据表（浮动 / 铺到单元格两种显示方式）的编辑表单：显示列、排序、筛选、跟随切片器，
 * 单元格模式下显示占用区域与刷新 / 解除关联，并给出可复制的公式示例
 */
const props = defineProps<{
  columns: DatasetColumn[]
  numeric: DatasetColumn[]
  /** 正在编辑的是铺到单元格的数据表 */
  editingCells: boolean
  /** 单元格模式写入中 */
  cellBusy: boolean
  /** 单元格模式写入的区域与数据列 */
  cellInfo: { range: string, written: number, total: number, firstNumeric: string | null, editCount: number, keyProblem: string | null } | null
}>()

const emit = defineEmits<{
  (e: 'switchMode', mode: 'cells' | 'float'): void
  (e: 'refreshCells'): void
  (e: 'detachCells'): void
  (e: 'resetEdits'): void
}>()

const form = defineModel<DataTableSpec | CellTableSpec>({ required: true })

const displayMode = computed({
  get: () => (props.editingCells ? 'cells' : 'float'),
  set: (mode: 'cells' | 'float') => emit('switchMode', mode),
})

const tableSort = computed({
  get: () => form.value.sort[0]?.column ?? '',
  set: (column: string) => {
    form.value.sort = column ? [{ column, desc: form.value.sort[0]?.desc ?? false }] : []
  },
})

function filterText(f: DataTableSpec['filters'][number]) {
  switch (f.op) {
    case 'in': return `${f.column}：${f.values.slice(0, 3).map(v => v || '（空白）').join('、')}${f.values.length > 3 ? ` 等 ${f.values.length} 项` : ''}`
    case 'between': return `${f.column}：${f.min ?? '…'} ~ ${f.max ?? '…'}`
    case 'contains': return `${f.column} 包含「${f.value}」`
    case 'prefix': return `${f.column} 以「${f.value || '（空白）'}」开头`
    case 'prefixes': return `${f.column} 以「${f.values.map(v => v || '（空白）').join('」「')}」之一开头`
  }
}

const formulaExamples = computed(() => {
  const f = form.value
  const shown = f.fields.length ? f.fields : props.columns.map(c => c.title)
  const num = shown.find(n => props.numeric.some(c => c.title === n))
  const first = shown[0] ?? '列名'
  return [
    num ? `=TSUM("${f.name}[${num}]")` : null,
    `=TCOUNT("${f.name}")`,
    `=TVALUE("${f.name}[${first}]", 1)`,
  ].filter((x): x is string => !!x)
})

async function copyText(text: string) {
  await navigator.clipboard.writeText(text)
  ElMessage.success('已复制，粘贴到单元格即可')
}
</script>

<template>
  <div class="contents">
    <div class="row">
      <span class="label">显示方式</span>
      <el-radio-group v-model="displayMode" size="small" :disabled="cellBusy">
        <el-radio-button value="float" title="浮在工作表上、内部滚动，千万行也只取看得到的部分">
          浮动表格
        </el-radio-button>
        <el-radio-button value="cells" :title="`写成普通单元格（最多 ${MAX_CELL_ROWS.toLocaleString()} 行），可用 A:A、区域引用`">
          铺到单元格
        </el-radio-button>
      </el-radio-group>
      <span v-if="cellBusy" class="text-xs text-gray-400">写入中…</span>
    </div>
    <div v-if="cellInfo" class="flex flex-col gap-1 rounded bg-[#F2F7FD] p-2 text-xs text-gray-600">
      <span>
        占用 <b>{{ cellInfo.range }}</b>，写入 {{ cellInfo.written.toLocaleString() }} 行<template v-if="cellInfo.total > cellInfo.written">（共 {{ cellInfo.total.toLocaleString() }} 行，单元格模式最多 {{ MAX_CELL_ROWS.toLocaleString() }} 行）</template>
      </span>
      <span class="text-gray-400">随数据集、切片器自动刷新；默认情况下在表格里手动改的内容刷新时会被覆盖，要保留请勾选下面的「保留手工调整」。下方或右侧有内容挡住时不会覆盖，会提示</span>
      <div class="flex flex-wrap items-center gap-2">
        <el-checkbox v-model="(form as CellTableSpec).keepEdits" size="small" :disabled="!!cellInfo.keyProblem || cellBusy">
          保留手工调整
        </el-checkbox>
        <span v-if="cellInfo.keyProblem" class="text-[#B45309]">{{ cellInfo.keyProblem }}</span>
        <template v-else-if="(form as CellTableSpec).keepEdits">
          <span>按主键认行，改过的格子刷新后仍保留{{ cellInfo.editCount ? `（现有 ${cellInfo.editCount} 处）` : '' }}；改主键列不会保留</span>
          <el-button v-if="cellInfo.editCount" link size="small" type="danger" :disabled="cellBusy" @click="emit('resetEdits')">
            恢复数据集的值
          </el-button>
        </template>
      </div>
      <div class="flex gap-2">
        <el-button link size="small" type="primary" :disabled="cellBusy" @click="emit('refreshCells')">
          立即刷新
        </el-button>
        <el-button link size="small" :disabled="cellBusy" @click="emit('detachCells')">
          转为普通单元格
        </el-button>
      </div>
    </div>
    <div class="row items-start">
      <span class="label pt-1">显示列</span>
      <el-select v-model="form.fields" size="small" multiple filterable collapse-tags collapse-tags-tooltip :max-collapse-tags="4" class="flex-1" placeholder="全部列">
        <el-option v-for="c in columns" :key="c.key" :label="c.title" :value="c.title" />
      </el-select>
    </div>
    <div class="row">
      <span class="label">排序</span>
      <el-select v-model="tableSort" size="small" clearable filterable class="flex-1" placeholder="入库顺序">
        <el-option v-for="c in columns" :key="c.key" :label="c.title" :value="c.title" />
      </el-select>
      <el-radio-group v-if="form.sort[0]" v-model="form.sort[0].desc" size="small">
        <el-radio-button :value="false">
          升序
        </el-radio-button>
        <el-radio-button :value="true">
          降序
        </el-radio-button>
      </el-radio-group>
    </div>
    <div class="row items-start">
      <span class="label pt-0.5">筛选</span>
      <div class="flex flex-1 flex-wrap gap-1">
        <el-tag v-for="(f, i) in form.filters" :key="i" size="small" closable @close="form.filters.splice(i, 1)">
          {{ filterText(f) }}
        </el-tag>
        <span v-if="!form.filters.length" class="text-xs text-gray-400">无（点数据表列头的按钮添加）</span>
      </div>
    </div>
    <div class="row">
      <el-checkbox v-model="form.followSlicers" size="small">
        跟随本页切片器筛选
      </el-checkbox>
    </div>
    <div class="flex flex-col gap-1 rounded bg-[#F7F7F5] p-2 text-xs text-gray-600">
      <span>在任意单元格用公式引用（服务端按数据表当前的筛选计算）：</span>
      <div v-if="cellInfo?.firstNumeric" class="flex items-center gap-2">
        <code class="min-w-0 flex-1 truncate text-[#0B5A2E]">{{ cellInfo.firstNumeric }}</code>
        <el-button link size="small" @click="copyText(cellInfo.firstNumeric)">
          复制
        </el-button>
      </div>
      <div v-for="ex in formulaExamples" :key="ex" class="flex items-center gap-2">
        <code class="min-w-0 flex-1 truncate text-[#0B5A2E]">{{ ex }}</code>
        <el-button link size="small" @click="copyText(ex)">
          复制
        </el-button>
      </div>
      <span class="text-gray-400">TSUM / TAVERAGE / TMIN / TMAX 后面可再加条件，如 TSUM("表[金额]", "店铺", B2)</span>
    </div>
  </div>
</template>

<style scoped lang="scss">
.row {
  @apply flex items-center gap-2;
}

.label {
  @apply w-14 shrink-0 text-xs text-gray-500;
}
</style>
