<script setup lang="ts">
import { computed, nextTick, reactive, ref, watch } from 'vue'

/**
 * 透视表的「计算值」：给名称与公式，引用透视表里已有的值（点下面的名称插入），格式选金额 / 数字 / 百分比。
 * 公式在汇总之后逐格计算，所以毛利率这类比率在小计、总计行也是对的
 */
const props = defineProps<{
  visible: boolean
  /** 编辑已有的计算值时带入；新建时为 null */
  initial: { label: string, formula: string, format: 'money' | 'number' | 'percent' } | null
  /** 可引用的值（透视表里普通的汇总值的表头文字） */
  names: string[]
}>()

const emit = defineEmits<{
  (e: 'update:visible', v: boolean): void
  (e: 'save', value: { label: string, formula: string, format: 'money' | 'number' | 'percent' }): void
}>()

const form = reactive({ label: '', formula: '', format: 'number' as 'money' | 'number' | 'percent' })
const input = ref<{ textarea?: HTMLTextAreaElement } | null>(null)

watch(() => props.visible, (v) => {
  if (!v)
    return
  form.label = props.initial?.label ?? ''
  form.formula = props.initial?.formula ?? ''
  form.format = props.initial?.format ?? 'number'
})

const operators = ['+', '-', '*', '/', '(', ')']

async function insert(text: string) {
  const ta = input.value?.textarea
  const start = ta?.selectionStart ?? form.formula.length
  const end = ta?.selectionEnd ?? start
  form.formula = form.formula.slice(0, start) + text + form.formula.slice(end)
  await nextTick()
  ta?.focus()
  ta?.setSelectionRange(start + text.length, start + text.length)
}

const ready = computed(() => !!form.label.trim() && !!form.formula.trim())

function save() {
  if (!ready.value)
    return
  emit('save', { label: form.label.trim(), formula: form.formula.trim(), format: form.format })
  emit('update:visible', false)
}
</script>

<template>
  <el-dialog :model-value="visible" :title="initial ? $t('修改计算值') : $t('添加计算值')" width="520px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div class="flex flex-col gap-3 text-[13px]">
      <div class="flex items-center gap-2">
        <span class="w-14 shrink-0 text-gray-500">{{ $t('名称') }}</span>
        <el-input v-model="form.label" size="small" :placeholder="$t('如：毛利率')" maxlength="30" />
      </div>
      <div class="flex gap-2">
        <span class="w-14 shrink-0 pt-1 text-gray-500">{{ $t('公式') }}</span>
        <div class="min-w-0 flex-1">
          <el-input ref="input" v-model="form.formula" type="textarea" :rows="3" :placeholder="$t('例：([金额（求和）] - [成本（求和）]) / [金额（求和）]')" @keydown.ctrl.enter="save" @keydown.meta.enter="save" />
          <div class="mt-1.5 flex gap-1">
            <button v-for="op in operators" :key="op" class="op" @click="insert(op === '(' || op === ')' ? op : ` ${op} `)">
              {{ op }}
            </button>
          </div>
        </div>
      </div>
      <div class="flex gap-2">
        <span class="w-14 shrink-0 pt-0.5 text-gray-500">{{ $t('可引用') }}</span>
        <div class="min-w-0 flex flex-1 flex-wrap gap-1.5">
          <button v-for="n in names" :key="n" class="chip-ref" :title="$t('插入 [{0}]', [n])" @click="insert(`[${n}]`)">
            {{ n }}
          </button>
          <span v-if="!names.length" class="text-xs text-[#92400E]">{{ $t('还没有普通的值字段：请先把字段拖入「值」，再添加计算值') }}</span>
        </div>
      </div>
      <div class="flex items-center gap-2">
        <span class="w-14 shrink-0 text-gray-500">{{ $t('格式') }}</span>
        <el-radio-group v-model="form.format" size="small">
          <el-radio-button value="number">
            {{ $t('数字') }}
          </el-radio-button>
          <el-radio-button value="money">
            {{ $t('金额') }}
          </el-radio-button>
          <el-radio-button value="percent">
            {{ $t('百分比') }}
          </el-radio-button>
        </el-radio-group>
        <span class="text-xs text-gray-400">{{ form.format === 'percent' ? $t('结果 0.25 显示为 25%') : form.format === 'money' ? $t('两位小数、千分位') : '' }}</span>
      </div>
      <div class="rounded bg-[#F4F9F6] px-3 py-2 text-xs text-gray-600 leading-5">
        {{ $t('公式在汇总之后逐格计算：毛利率 = 总毛利 ÷ 总金额，小计、总计行也是这样算（不是各行比率的平均）。支持 + − × ÷ 与括号；任一值为空或除数为 0 时该格显示空白。') }}
      </div>
    </div>
    <template #footer>
      <el-button @click="emit('update:visible', false)">
        {{ $t('取消') }}
      </el-button>
      <el-button type="primary" :disabled="!ready" @click="save">
        {{ initial ? $t('保存') : $t('添加') }}
      </el-button>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.op {
  @apply h-6 w-8 cursor-pointer rounded border border-[#d1d5db] border-solid bg-white text-sm text-gray-700 hover:border-[#217346] hover:text-[#217346];
}

.chip-ref {
  @apply cursor-pointer rounded border border-[#cfe5d8] border-solid bg-[#F4F9F6] px-2 py-0.5 text-xs text-[#0E5A31] hover:border-[#217346];
}
</style>
