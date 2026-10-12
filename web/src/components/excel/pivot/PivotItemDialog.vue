<script setup lang="ts">
import { computed, nextTick, reactive, ref, watch } from 'vue'

/**
 * 透视表的「计算项」：在某个行 / 列字段的取值之间做运算，新增一个取值。
 * 如字段「单据类型」下新增「期末」= [期初] + [入库] - [出库]（库存进销存）
 */
export interface CalcItemForm {
  field: string
  name: string
  formula: string
}

const props = defineProps<{
  visible: boolean
  /** 编辑已有的计算项时带入；新建时为 null */
  initial: CalcItemForm | null
  /** 可选的字段：当前在行 / 列里的字段 */
  fields: string[]
  /** 某个字段下现有的取值（取自最近一次计算的结果） */
  namesOf: (field: string) => string[]
}>()

const emit = defineEmits<{
  (e: 'update:visible', v: boolean): void
  (e: 'save', value: CalcItemForm): void
}>()

const form = reactive<CalcItemForm>({ field: '', name: '', formula: '' })
const input = ref<{ textarea?: HTMLTextAreaElement } | null>(null)

watch(() => props.visible, (v) => {
  if (!v)
    return
  form.field = props.initial?.field ?? props.fields[0] ?? ''
  form.name = props.initial?.name ?? ''
  form.formula = props.initial?.formula ?? ''
})

const names = computed(() => props.namesOf(form.field))
const operators = ['+', '-', '*', '/', '(', ')']
const ready = computed(() => !!form.field && !!form.name.trim() && !!form.formula.trim())

async function insert(text: string) {
  const ta = input.value?.textarea
  const start = ta?.selectionStart ?? form.formula.length
  const end = ta?.selectionEnd ?? start
  form.formula = form.formula.slice(0, start) + text + form.formula.slice(end)
  await nextTick()
  ta?.focus()
  ta?.setSelectionRange(start + text.length, start + text.length)
}

function save() {
  if (!ready.value)
    return
  emit('save', { field: form.field, name: form.name.trim(), formula: form.formula.trim() })
  emit('update:visible', false)
}
</script>

<template>
  <el-dialog :model-value="visible" :title="initial ? '修改计算项' : '添加计算项'" width="520px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div class="flex flex-col gap-3 text-[13px]">
      <div class="flex items-center gap-2">
        <span class="w-14 shrink-0 text-gray-500">字段</span>
        <el-select v-model="form.field" size="small" class="flex-1" :disabled="!!initial" placeholder="选择一个行 / 列字段">
          <el-option v-for="f in fields" :key="f" :label="f" :value="f" />
        </el-select>
      </div>
      <div class="flex items-center gap-2">
        <span class="w-14 shrink-0 text-gray-500">新取值</span>
        <el-input v-model="form.name" size="small" placeholder="如：期末" maxlength="30" />
      </div>
      <div class="flex gap-2">
        <span class="w-14 shrink-0 pt-1 text-gray-500">公式</span>
        <div class="min-w-0 flex-1">
          <el-input ref="input" v-model="form.formula" type="textarea" :rows="3" placeholder="例：[期初] + [入库] - [出库]" @keydown.ctrl.enter="save" @keydown.meta.enter="save" />
          <div class="mt-1.5 flex gap-1">
            <button v-for="op in operators" :key="op" class="op" @click="insert(op === '(' || op === ')' ? op : ` ${op} `)">
              {{ op }}
            </button>
          </div>
        </div>
      </div>
      <div class="flex gap-2">
        <span class="w-14 shrink-0 pt-0.5 text-gray-500">可引用</span>
        <div class="min-w-0 flex flex-1 flex-wrap gap-1.5">
          <button v-for="n in names.slice(0, 40)" :key="n" class="chip-ref" :title="`插入 [${n}]`" @click="insert(`[${n}]`)">
            {{ n }}
          </button>
          <span v-if="!names.length" class="text-xs text-[#92400E]">还没有算出结果，没法列出该字段的取值；也可以直接手写 [取值名称]</span>
          <span v-else-if="names.length > 40" class="text-xs text-gray-400">… 共 {{ names.length }} 个取值，其余请手写 [取值名称]</span>
        </div>
      </div>
      <div class="rounded bg-[#F4F9F6] px-3 py-2 text-xs text-gray-600 leading-5">
        公式对<b>每个值、每个格子</b>分别计算（明细、小计、总计都是）；引用的取值在某一格没有记录时按 0 算。新取值排在该字段的最后；总计列不含它，避免把「入库 + 出库 + 期末」加在一起。
        要在<b>值之间</b>做运算（如毛利率），请用「+ 计算值」。
      </div>
    </div>
    <template #footer>
      <el-button @click="emit('update:visible', false)">
        取消
      </el-button>
      <el-button type="primary" :disabled="!ready" @click="save">
        {{ initial ? '保存' : '添加' }}
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
