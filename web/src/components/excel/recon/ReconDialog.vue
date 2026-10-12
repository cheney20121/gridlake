<script setup lang="ts">
import type { DatasetColumn, DatasetInfo } from '@/api/dataset'
import type { ReconDef, ReconInfo, ReconPreview, ReconSide } from '@/api/recon'
import { computed, reactive, ref, watch } from 'vue'
import { createReconApi, getReconApi, previewReconApi, updateReconApi } from '@/api/recon'
import { t } from '@/i18n'

/**
 * 新建 / 修改对账：选两个数据集，对应金额 / 日期 / 对账键 / 摘要列，设置金额容差与日期窗口。
 * 对账方案会同时生成一个同名的结果数据集（含「状态」「勾对完成日」等列），公式、透视、图表直接可用
 */
const props = defineProps<{
  visible: boolean
  datasets: DatasetInfo[]
  /** 修改已有对账（为空时新建） */
  reconId?: string
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'saved', datasetId: string): void
}>()

interface SideForm {
  dataset: string
  label: string
  amount: string
  sign: 1 | -1
  date: string
  memo: string
}

const blank = (label: string): SideForm => ({ dataset: '', label, amount: '', sign: 1, date: '', memo: '' })
const form = reactive({
  name: '',
  left: blank('银行'),
  right: blank('账面'),
  /** 对账键：两侧按顺序一一对应 */
  keys: [] as { left: string, right: string }[],
  tolerance: 0,
  useDays: true,
  days: 3,
  groupByKey: false,
})
const info = ref<ReconInfo | null>(null)
const preview = ref<ReconPreview | null>(null)
const previewing = ref(false)
const saving = ref(false)

/** 对账来源只用实际的数据集，不用对账结果自己 */
const options = computed(() => props.datasets.filter(d => d.kind !== 'recon'))
const dsOf = (id: string) => props.datasets.find(d => d.id === id)
const cols = (id: string, kinds?: DatasetColumn['kind'][]) => (dsOf(id)?.columns ?? []).filter(c => !kinds || kinds.includes(c.kind))

const AMOUNT_HINT = /金额|收支|发生额|借贷|收入|支出/
const MEMO_HINT = /摘要|备注|说明|用途|对方|户名/

/** 选了数据集后猜一下金额 / 日期 / 摘要列（可再改） */
function guess(side: SideForm) {
  const money = cols(side.dataset, ['money', 'number'])
  side.amount = (money.find(c => AMOUNT_HINT.test(c.title)) ?? money[0])?.title ?? ''
  side.date = cols(side.dataset, ['datetime'])[0]?.title ?? ''
  side.memo = cols(side.dataset, ['text', 'id']).find(c => MEMO_HINT.test(c.title))?.title ?? ''
}

function onDataset(side: SideForm) {
  guess(side)
  preview.value = null
  // 两侧都选好后，同名的 id / 文本列提示为候选对账键
  if (!props.reconId && form.left.dataset && form.right.dataset && !form.keys.length) {
    const l = cols(form.left.dataset, ['id', 'text'])
    const r = cols(form.right.dataset, ['id', 'text'])
    const same = l.find(c => c.kind === 'id' && r.some(x => x.title === c.title))
    if (same)
      form.keys = [{ left: same.title, right: same.title }]
  }
  if (!props.reconId && !form.name.trim() && form.left.dataset && form.right.dataset)
    form.name = `${dsOf(form.left.dataset)?.name}对账`
}

watch(() => props.visible, async (v) => {
  if (!v)
    return
  preview.value = null
  info.value = null
  if (!props.reconId) {
    Object.assign(form, { name: '', left: blank('银行'), right: blank('账面'), keys: [], tolerance: 0, useDays: true, days: 3, groupByKey: false })
    return
  }
  const i = await getReconApi(props.reconId)
  info.value = i
  const d = i.def
  const titleOf = (id: string, key: string) => cols(id).find(c => c.key === key)?.title ?? key
  const sideForm = (s: ReconSide): SideForm => ({
    dataset: s.dataset,
    label: s.label,
    amount: titleOf(s.dataset, s.amount),
    sign: s.sign,
    date: s.date ? titleOf(s.dataset, s.date) : '',
    memo: s.memo ? titleOf(s.dataset, s.memo) : '',
  })
  form.name = i.name
  form.left = sideForm(d.left)
  form.right = sideForm(d.right)
  form.keys = d.left.keys.map((k, n) => ({ left: titleOf(d.left.dataset, k), right: titleOf(d.right.dataset, d.right.keys[n] ?? '') }))
  form.tolerance = d.tolerance
  form.useDays = d.days != null
  form.days = d.days ?? 3
  form.groupByKey = d.groupByKey
})

function sideDef(s: SideForm, keys: string[]): ReconSide {
  return { dataset: s.dataset, label: s.label.trim(), amount: s.amount, sign: s.sign, date: s.date || null, keys, memo: s.memo || null }
}

const def = computed<ReconDef>(() => {
  const ks = form.keys.filter(k => k.left && k.right)
  return {
    left: sideDef(form.left, ks.map(k => k.left)),
    right: sideDef(form.right, ks.map(k => k.right)),
    tolerance: Number(form.tolerance) || 0,
    days: form.useDays ? Math.max(0, Math.round(Number(form.days) || 0)) : null,
    groupByKey: form.groupByKey && ks.length > 0,
  }
})

watch(def, () => {
  preview.value = null
})

/** 缺什么，直接说出来 */
const missing = computed(() => {
  const f = form
  if (!f.left.dataset || !f.right.dataset)
    return '请选择两个数据集'
  if (f.left.dataset === f.right.dataset)
    return '两侧不能是同一个数据集'
  if (!f.left.amount || !f.right.amount)
    return '请选择两侧的金额列'
  if (!f.left.label.trim() || !f.right.label.trim() || f.left.label.trim() === f.right.label.trim())
    return '两侧的名称不能为空，也不能相同'
  if (form.useDays && (!f.left.date || !f.right.date))
    return '设置了日期窗口，两侧都要选日期列'
  if (form.keys.some(k => !!k.left !== !!k.right))
    return '对账键要两侧都选'
  return ''
})

async function runPreview() {
  if (missing.value)
    return
  previewing.value = true
  try {
    preview.value = await previewReconApi(def.value)
  }
  finally {
    previewing.value = false
  }
}

/** 换了数据集 / 金额列 / 金额方向：已有的勾对不再可信 */
function breaksMatches() {
  const o = info.value?.def
  if (!o || !info.value?.groups)
    return false
  const n = def.value
  const keyOf = (id: string, title: string) => cols(id).find(c => c.title === title)?.key
  const differs = (a: ReconSide, b: ReconSide) => a.dataset !== b.dataset || a.amount !== keyOf(b.dataset, b.amount) || a.sign !== b.sign
  return differs(o.left, n.left) || differs(o.right, n.right)
}

async function save() {
  if (missing.value || !form.name.trim())
    return
  saving.value = true
  try {
    if (props.reconId) {
      let reset = false
      if (breaksMatches()) {
        reset = await ElMessageBox.confirm(
          t('换了数据集、金额列或金额方向后，已有的 {0} 个勾对组不再可信，保存会清空勾对，需要重新勾对。', [info.value?.groups]),
          t('清空勾对？'),
          { type: 'warning', confirmButtonText: t('清空并保存'), confirmButtonClass: 'ec-button--danger' },
        ).then(() => true, () => false)
        if (!reset)
          return
      }
      await updateReconApi(props.reconId, form.name.trim(), def.value, reset)
      ElMessage.success(t('已保存'))
      emit('saved', props.reconId)
    }
    else {
      const id = await createReconApi(form.name.trim(), def.value)
      ElMessage.success(t('已创建对账「{0}」', [form.name.trim()]))
      emit('saved', id)
    }
    emit('update:visible', false)
  }
  finally {
    saving.value = false
  }
}

const money = (v: number) => v.toLocaleString('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
const sideRows = computed(() => [
  { title: t('数据集'), key: 'dataset' },
  { title: t('显示名称'), key: 'label' },
  { title: t('金额列'), key: 'amount' },
  { title: t('金额方向'), key: 'sign' },
  { title: t('日期列'), key: 'date' },
  { title: t('摘要列'), key: 'memo' },
] as const)
</script>

<template>
  <el-dialog :model-value="visible" :title="reconId ? $t('修改对账 · {0}', [info?.name ?? '']) : $t('新建对账')" width="820px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div class="flex flex-col gap-3 text-[13px]">
      <div class="text-xs text-gray-500 leading-5">
        {{ $t('把两个数据集（如银行流水与企业账）逐笔勾对，剩下没勾上的就是未达账项。会生成一个同名的结果数据集， 带「状态（已勾对 / 未勾对）、勾对组、勾对完成日」等列，公式、透视、图表直接可用，数据重新导入后自动更新。') }}
        <template v-if="reconId && info?.groups">
          <br><span class="text-[#92400E]">{{ $t('已有 {0} 个勾对组。换了数据集、金额列或金额方向会清空勾对；改容差、日期窗口、对账键不影响已有勾对。', [info.groups]) }}</span>
        </template>
      </div>

      <div class="flex items-center gap-2">
        <span class="w-20 text-gray-500">{{ $t('对账名称') }}</span>
        <el-input v-model="form.name" size="small" class="!w-72" maxlength="40" :placeholder="$t('如：招商银行对账')" />
      </div>

      <div class="grid grid-cols-[80px_1fr_1fr] items-center gap-x-3 gap-y-1.5">
        <span />
        <span class="font-semibold text-gray-700">{{ $t('A 方（如银行流水）') }}</span>
        <span class="font-semibold text-gray-700">{{ $t('B 方（如企业账）') }}</span>
        <template v-for="r in sideRows" :key="r.key">
          <span class="text-gray-500">{{ r.title }}</span>
          <template v-for="s in [form.left, form.right]" :key="s === form.left ? 'l' : 'r'">
            <el-select v-if="r.key === 'dataset'" v-model="s.dataset" size="small" filterable :placeholder="$t('选择数据集')" @change="onDataset(s)">
              <el-option v-for="d in options" :key="d.id" :label="$t('{0}（{1} 行）', [d.name, d.rowCount.toLocaleString()])" :value="d.id" />
            </el-select>
            <el-input v-else-if="r.key === 'label'" v-model="s.label" size="small" maxlength="12" />
            <el-select v-else-if="r.key === 'amount'" v-model="s.amount" size="small" filterable :placeholder="$t('选择金额列')">
              <el-option v-for="c in cols(s.dataset, ['money', 'number'])" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
            <el-radio-group v-else-if="r.key === 'sign'" v-model="s.sign" size="small">
              <el-radio-button :value="1">
                {{ $t('原样') }}
              </el-radio-button>
              <el-radio-button :value="-1">
                {{ $t('取反') }}
              </el-radio-button>
            </el-radio-group>
            <el-select v-else-if="r.key === 'date'" v-model="s.date" size="small" filterable clearable :placeholder="$t('（可选）日期列')">
              <el-option v-for="c in cols(s.dataset, ['datetime'])" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
            <el-select v-else v-model="s.memo" size="small" filterable clearable :placeholder="$t('（可选）显示在列表里的说明')">
              <el-option v-for="c in cols(s.dataset).filter(c => c.kind === 'text' || c.kind === 'id')" :key="c.key" :label="c.title" :value="c.title" />
            </el-select>
          </template>
        </template>
        <span class="text-xs text-gray-400" />
        <span class="col-span-2 text-xs text-gray-400 leading-5">
          {{ $t('金额方向：让同一笔业务在两边的正负一致。例如银行收入为正，企业账的收款记在贷方为负，就把企业账设为「取反」。') }}
        </span>
      </div>

      <div class="flex flex-col gap-1.5">
        <div class="text-gray-500">
          {{ $t('对账键（可选）：两边都相同才会自动勾对，如订单号、流水号、参考号；对账键为空的行不参与自动勾对') }}
        </div>
        <div v-for="(k, i) in form.keys" :key="i" class="grid grid-cols-[1fr_24px_1fr_40px] items-center gap-2">
          <el-select v-model="k.left" size="small" filterable :placeholder="$t('A 方的列')">
            <el-option v-for="c in cols(form.left.dataset, ['id', 'text', 'number'])" :key="c.key" :label="c.title" :value="c.title" />
          </el-select>
          <span class="text-center text-gray-400">=</span>
          <el-select v-model="k.right" size="small" filterable :placeholder="$t('B 方的列')">
            <el-option v-for="c in cols(form.right.dataset, ['id', 'text', 'number'])" :key="c.key" :label="c.title" :value="c.title" />
          </el-select>
          <el-button link size="small" type="danger" @click="form.keys.splice(i, 1)">
            {{ $t('删除') }}
          </el-button>
        </div>
        <el-button v-if="form.keys.length < 3" link size="small" type="primary" class="self-start" @click="form.keys.push({ left: '', right: '' })">
          {{ $t('+ 添加对账键') }}
        </el-button>
      </div>

      <div class="rounded bg-[#F7F8FA] px-3 py-2">
        <div class="mb-1.5 text-gray-500">
          {{ $t('自动勾对的规则') }}
        </div>
        <div class="flex flex-wrap items-center gap-x-5 gap-y-2">
          <label class="flex items-center gap-1.5">{{ $t('金额相差不超过') }}
            <el-input-number v-model="form.tolerance" size="small" :min="0" :max="1000000" :precision="2" :step="0.01" controls-position="right" class="!w-28" /> {{ $t('元') }}
          </label>
          <label class="flex items-center gap-1.5">
            <el-checkbox v-model="form.useDays" size="small" />{{ $t('日期相差不超过') }}
            <el-input-number v-model="form.days" size="small" :min="0" :max="366" :disabled="!form.useDays" controls-position="right" class="!w-24" /> {{ $t('天') }}
          </label>
          <el-checkbox v-model="form.groupByKey" size="small" :disabled="!form.keys.length">
            {{ $t('同一个对账键下两边合计相等的多笔，也自动勾对（一对多、多对多）') }}
          </el-checkbox>
        </div>
        <div class="mt-1 text-xs text-gray-400 leading-5">
          {{ $t('自动勾对分三轮：① 对账键与金额都相同，按日期顺序一一对应；② 金额容差 / 日期窗口内，差得最少的优先；③ 同一对账键下合计相等的多笔（不看日期窗口）。其余的手工勾对。') }}
        </div>
      </div>

      <div class="flex items-center gap-2">
        <el-button size="small" :disabled="!!missing" :loading="previewing" @click="runPreview">
          {{ $t('检查两侧数据') }}
        </el-button>
        <span v-if="missing" class="text-xs text-gray-400">{{ missing }}</span>
      </div>

      <div v-if="preview" class="rounded bg-[#F7F8FA] px-3 py-2 text-xs leading-6">
        <div v-for="s in [preview.left, preview.right]" :key="s.label" class="flex flex-wrap items-center gap-x-4">
          <b class="w-12">{{ s.label }}</b>
          <span>{{ $t('{0} 行', [s.rows.toLocaleString()]) }}</span>
          <span>{{ $t('金额合计') }} <b class="tabular-nums">{{ money(s.total) }}</b></span>
          <span v-if="s.minDate">{{ $t('日期 {0} ~ {1}', [s.minDate, s.maxDate]) }}</span>
          <span v-if="s.undated" class="text-[#92400E]">{{ $t('{0} 行没有日期', [s.undated.toLocaleString()]) }}</span>
          <span v-if="s.blankKeys" class="text-[#92400E]">{{ $t('{0} 行对账键为空', [s.blankKeys.toLocaleString()]) }}</span>
        </div>
        <div v-for="w in preview.warnings" :key="w" class="text-[#B45309]">
          ⚠ {{ w }}
        </div>
        <div v-if="!preview.warnings.length" class="text-[#0B5A2E]">
          {{ $t('✓ 没有发现明显问题') }}
        </div>
      </div>
    </div>
    <template #footer>
      <el-button @click="emit('update:visible', false)">
        {{ $t('取消') }}
      </el-button>
      <el-button type="primary" :disabled="!!missing || !form.name.trim()" :loading="saving" @click="save">
        {{ reconId ? $t('保存修改') : $t('创建对账') }}
      </el-button>
    </template>
  </el-dialog>
</template>
