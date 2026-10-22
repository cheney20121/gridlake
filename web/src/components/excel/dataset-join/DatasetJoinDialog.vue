<script setup lang="ts">
import type { DatasetInfo, JoinPreview, ViewDef } from '@/api/dataset'
import { computed, reactive, ref, watch } from 'vue'
import { createViewApi, previewJoinApi, updateViewApi } from '@/api/dataset'
import { t as tr } from '@/i18n'

/**
 * 关联数据集：左 ⟕ 右，按键带入右侧的列，生成新的数据集（关联视图）
 * 典型场景：资金流水 / 订单 ⟕ 商品成本 → 毛利；流水 ⟕ 店铺档案；达人 ⟕ 合同佣金率
 */
const props = defineProps<{
  visible: boolean
  datasets: DatasetInfo[]
  /** 默认的左侧数据集（当前打开的数据表） */
  leftId: string
  /** 修改已有的关联视图（为空时新建） */
  viewId?: string
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 新建或修改完成 */
  (e: 'saved', datasetId: string): void
}>()

const form = reactive({
  leftId: '',
  rightId: '',
  on: [] as { left: string, right: string }[],
  /** 右侧列：是否带入、带入后的列名 */
  columns: [] as { source: string, title: string, include: boolean }[],
  name: '',
})
const preview = ref<JoinPreview | null>(null)
const previewing = ref(false)
const creating = ref(false)

const left = computed(() => props.datasets.find(d => d.id === form.leftId))
const right = computed(() => props.datasets.find(d => d.id === form.rightId))
const editingView = computed(() => props.viewId ? props.datasets.find(d => d.id === props.viewId && d.kind === 'view') : undefined)
/** 修改时不能选自身作为左 / 右侧（更深的循环由服务端检查） */
const options = computed(() => props.datasets.filter(d => d.id !== props.viewId))
const rightOptions = computed(() => options.value.filter(d => d.id !== form.leftId))

watch(() => props.visible, (v) => {
  if (!v)
    return
  preview.value = null
  const me = editingView.value
  if (me?.view) {
    // 回填已有定义：右侧所有列都列出来，已带入的勾选并沿用列名
    const v = me.view
    const r = props.datasets.find(d => d.id === v.right)
    form.leftId = v.left
    form.rightId = v.right
    form.on = v.on.map(k => ({ ...k }))
    form.columns = (r?.columns ?? [])
      .filter(c => !v.on.some(k => k.right === c.title))
      .map((c) => {
        const used = v.columns.find(x => x.source === c.title)
        return { source: c.title, title: used?.title ?? `${c.title}_${r?.name}`, include: !!used }
      })
    form.name = me.name
    return
  }
  form.leftId = props.leftId
  form.rightId = ''
  form.on = []
  form.columns = []
  form.name = ''
})

const leftTitles = computed(() => new Set(left.value?.columns.map(c => c.title) ?? []))

/** 换左 / 右侧数据集：自动匹配同名的键列（优先编号类），列出可带入的列 */
function autoFill() {
  preview.value = null
  const l = left.value
  const r = right.value
  if (!l || !r) {
    form.columns = []
    return
  }
  const same = r.columns.filter(c => !c.computed && l.columns.some(x => x.title === c.title && !x.computed))
  const key = same.find(c => c.kind === 'id') ?? same[0]
  form.on = key ? [{ left: key.title, right: key.title }] : [{ left: '', right: '' }]
  form.columns = r.columns
    .filter(c => !form.on.some(k => k.right === c.title))
    .map(c => ({
      source: c.title,
      // 与左侧重名时加上右侧数据集名作后缀
      title: leftTitles.value.has(c.title) ? `${c.title}_${r.name}` : c.title,
      include: !leftTitles.value.has(c.title),
    }))
  if (!editingView.value)
    form.name = `${l.name}+${r.name}`
}

const included = computed(() => form.columns.filter(c => c.include))
const conflict = computed(() => {
  const seen = new Set<string>()
  for (const c of included.value) {
    const t = c.title.trim()
    if (!t || leftTitles.value.has(t) || seen.has(t))
      return t || tr('（空）')
    seen.add(t)
  }
  return null
})

const def = computed<ViewDef>(() => ({
  left: form.leftId,
  right: form.rightId,
  on: form.on.filter(k => k.left && k.right),
  columns: included.value.map(c => ({ source: c.source, title: c.title.trim() })),
}))

// 关联键 / 带入列有变化时，之前的预览不再准确
watch(def, () => {
  preview.value = null
})

const ready = computed(() => !!left.value && !!right.value && def.value.on.length > 0 && def.value.columns.length > 0 && !conflict.value)

async function runPreview() {
  if (!ready.value)
    return
  previewing.value = true
  try {
    preview.value = await previewJoinApi(def.value)
  }
  finally {
    previewing.value = false
  }
}

async function create() {
  if (!ready.value || !form.name.trim())
    return
  creating.value = true
  try {
    let id = props.viewId
    if (id && editingView.value) {
      await updateViewApi(id, form.name.trim(), def.value)
      ElMessage.success(tr('已修改关联数据集「{0}」', [form.name.trim()]))
    }
    else {
      id = await createViewApi(form.name.trim(), def.value)
      ElMessage.success(tr('已创建关联数据集「{0}」', [form.name.trim()]))
    }
    emit('saved', id)
    emit('update:visible', false)
  }
  finally {
    creating.value = false
  }
}

const matchRate = computed(() => preview.value && preview.value.leftRows ? preview.value.matchedRows / preview.value.leftRows : 0)
</script>

<template>
  <el-dialog :model-value="visible" :title="editingView ? $t('修改关联 · {0}', [editingView.name]) : $t('关联数据集')" width="760px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div class="flex flex-col gap-3 text-[13px]">
      <div class="text-xs text-gray-500 leading-5">
        {{ $t('以左侧数据集为主（行数不变），按关联键从右侧带入列，生成新的数据集，可浏览、透视、在 DSUMIFS 中引用。 右侧同一个键有多行时只取最先导入的一行，不会把左侧的行算重复。') }}
        <template v-if="editingView">
          <br><span class="text-[#92400E]">{{ $t('修改后，基于它的计算列、关联视图会同步更新；如果会导致它们失效（如去掉了被公式引用的列），将取消修改并提示。') }}</span>
        </template>
      </div>
      <div class="grid grid-cols-[72px_1fr_24px_1fr] items-center gap-2">
        <span class="text-gray-500">{{ $t('数据集') }}</span>
        <el-select v-model="form.leftId" size="small" @change="autoFill">
          <el-option v-for="d in options" :key="d.id" :label="$t('{0}（{1} 行）', [d.name, d.rowCount.toLocaleString()])" :value="d.id" />
        </el-select>
        <span class="text-center text-gray-400">⟕</span>
        <el-select v-model="form.rightId" size="small" :placeholder="$t('选择要关联的数据集')" @change="autoFill">
          <el-option v-for="d in rightOptions" :key="d.id" :label="$t('{0}（{1} 行）', [d.name, d.rowCount.toLocaleString()])" :value="d.id" />
        </el-select>
      </div>

      <template v-if="left && right">
        <div class="flex flex-col gap-1.5">
          <div class="text-gray-500">
            {{ $t('关联键（左侧的列 = 右侧的列）') }}
          </div>
          <div v-for="(k, i) in form.on" :key="i" class="grid grid-cols-[1fr_24px_1fr_40px] items-center gap-2">
            <el-select v-model="k.left" size="small" filterable :placeholder="$t('左侧列')">
              <el-option v-for="c in left.columns.filter(c => !c.computed)" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
            <span class="text-center text-gray-400">=</span>
            <el-select v-model="k.right" size="small" filterable :placeholder="$t('右侧列')">
              <el-option v-for="c in right.columns.filter(c => !c.computed)" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
            <el-button v-if="form.on.length > 1" link size="small" type="danger" @click="form.on.splice(i, 1)">
              {{ $t('删除') }}
            </el-button>
          </div>
          <el-button link size="small" type="primary" class="self-start" @click="form.on.push({ left: '', right: '' })">
            {{ $t('+ 再加一个关联键（多列组合匹配）') }}
          </el-button>
        </div>

        <div class="flex flex-col gap-1">
          <div class="text-gray-500">
            {{ $t('从「{0}」带入的列', [right.name]) }}
          </div>
          <div class="max-h-44 overflow-auto rounded border border-[#e5e7eb] border-solid p-1.5">
            <div v-for="c in form.columns" :key="c.source" class="flex items-center gap-2 py-0.5">
              <el-checkbox v-model="c.include" size="small" />
              <span class="w-36 truncate text-gray-600" :title="c.source">{{ c.source }}</span>
              <span class="text-gray-300">→</span>
              <el-input v-model="c.title" size="small" class="!w-48" :disabled="!c.include" />
            </div>
          </div>
          <div v-if="conflict" class="text-xs text-[#B91C1C]">
            {{ $t('带入列「{0}」为空、重复或与左侧列重名，请改名', [conflict]) }}
          </div>
        </div>

        <div class="flex items-center gap-2">
          <span class="w-[72px] text-gray-500">{{ $t('新数据集') }}</span>
          <el-input v-model="form.name" size="small" class="!w-72" />
          <el-button size="small" :disabled="!ready" :loading="previewing" @click="runPreview">
            {{ $t('预览匹配情况') }}
          </el-button>
        </div>

        <div v-if="preview" class="rounded bg-[#F7F8FA] px-3 py-2 text-xs leading-5">
          <div class="flex items-center gap-2">
            <span>{{ $t('左侧 {0} 行，关联上', [preview.leftRows.toLocaleString()]) }} <b>{{ preview.matchedRows.toLocaleString() }}</b> {{ $t('行') }}</span>
            <span class="font-semibold" :class="matchRate >= 0.95 ? 'text-[#0B5A2E]' : 'text-[#92400E]'">（{{ (matchRate * 100).toFixed(1) }}%）</span>
            <span v-if="preview.unmatchedRows" class="text-[#92400E]">{{ $t('{0} 行关联不上，带入列为空', [preview.unmatchedRows.toLocaleString()]) }}</span>
          </div>
          <div class="mt-1 h-1.5 rounded-full bg-[#e5e7eb]">
            <div class="h-full rounded-full bg-[#3b7dd8]" :style="{ width: `${matchRate * 100}%` }" />
          </div>
          <div v-if="preview.unmatchedKeys.length" class="mt-1.5 text-gray-600">
            {{ $t('关联不上的键（按行数）：') }}
            <span v-for="([k, n], i) in preview.unmatchedKeys.slice(0, 8)" :key="i" class="mr-2">{{ k }} <span class="text-gray-400">×{{ n.toLocaleString() }}</span></span>
          </div>
          <div v-if="preview.rightDuplicateKeys" class="mt-1 text-[#92400E]">
            {{ $t('「{0}」中有 {1} 个键对应多行，已按最先导入的一行关联；如需按其他行，请先整理右侧数据', [right.name, preview.rightDuplicateKeys]) }}
          </div>
        </div>
      </template>
    </div>
    <template #footer>
      <el-button @click="emit('update:visible', false)">
        {{ $t('取消') }}
      </el-button>
      <el-button type="primary" :disabled="!ready || !form.name.trim()" :loading="creating" @click="create">
        {{ editingView ? $t('保存修改') : $t('创建关联数据集') }}
      </el-button>
    </template>
  </el-dialog>
</template>
