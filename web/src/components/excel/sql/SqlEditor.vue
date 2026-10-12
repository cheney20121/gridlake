<script setup lang="ts">
import type { Completion, SqlDataset } from '@/features/excel/sql/complete'
import { computed, nextTick, ref } from 'vue'
import { complete } from '@/features/excel/sql/complete'
import { highlightHtml } from '@/features/excel/sql/highlight'

/**
 * SQL 编辑器：透明的 textarea 叠在着色的 <pre> 上（输入、选择、撤销、中文输入法都用原生的），
 * 加上表名 / 列名 / 关键字补全（Ctrl + 空格手动触发；Tab / 回车确认；Esc 关闭）
 */
const props = defineProps<{
  modelValue: string
  datasets: SqlDataset[]
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', v: string): void
  /** Ctrl / ⌘ + Enter */
  (e: 'run'): void
}>()

const ta = ref<HTMLTextAreaElement>()
const layer = ref<HTMLPreElement>()
const mirror = ref<HTMLDivElement>()
const html = computed(() => highlightHtml(props.modelValue))

const completion = ref<Completion | null>(null)
const active = ref(0)
const popupPos = ref({ left: 0, top: 0 })
/** 输入法组字期间（拼音还没上屏）不补全，否则会打断输入 */
const state = { composing: false }

function onComposition(on: boolean) {
  state.composing = on
  if (!on)
    refresh()
}

function syncScroll() {
  if (layer.value && ta.value) {
    layer.value.scrollTop = ta.value.scrollTop
    layer.value.scrollLeft = ta.value.scrollLeft
  }
  closePopup()
}

function closePopup() {
  completion.value = null
}

/** 光标在编辑器内的像素位置：用同样排版的隐藏镜像量出光标前文字的末尾 */
function caretPixel(caret: number) {
  const m = mirror.value!
  const t = ta.value!
  m.textContent = t.value.slice(0, caret)
  const marker = document.createElement('span')
  marker.textContent = '\u200B'
  m.appendChild(marker)
  const left = marker.offsetLeft - t.scrollLeft
  const top = marker.offsetTop - t.scrollTop + 20
  return { left: Math.max(0, Math.min(left, t.clientWidth - 260)), top }
}

function refresh(explicit = false) {
  const t = ta.value
  if (!t || state.composing || t.selectionStart !== t.selectionEnd) {
    closePopup()
    return
  }
  // 用输入框里此刻的文字：刚输入时 v-model 的值要等下一轮才更新
  const c = complete(t.value, t.selectionStart, props.datasets, explicit)
  completion.value = c
  active.value = 0
  if (c)
    popupPos.value = caretPixel(c.to)
}

function onInput(e: Event) {
  emit('update:modelValue', (e.target as HTMLTextAreaElement).value)
  refresh()
}

async function accept(i = active.value) {
  const c = completion.value
  const t = ta.value
  const item = c?.items[i]
  if (!c || !t || !item)
    return
  const next = t.value.slice(0, c.from) + item.insert + t.value.slice(c.to)
  emit('update:modelValue', next)
  closePopup()
  await nextTick()
  // 函数补成「name(」后光标在括号后；其余补完后光标在名称后
  const pos = c.from + item.insert.length
  t.focus()
  t.setSelectionRange(pos, pos)
}

function onKeydown(e: KeyboardEvent) {
  const mod = e.metaKey || e.ctrlKey
  if (completion.value) {
    const n = completion.value.items.length
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      active.value = (active.value + (e.key === 'ArrowDown' ? 1 : n - 1)) % n
      return
    }
    if ((e.key === 'Enter' && !mod) || e.key === 'Tab') {
      e.preventDefault()
      accept()
      return
    }
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      closePopup()
      return
    }
  }
  if (mod && e.key === 'Enter') {
    e.preventDefault()
    emit('run')
    return
  }
  if (e.ctrlKey && e.key === ' ') {
    e.preventDefault()
    refresh(true)
    return
  }
  if (e.key === 'Tab') {
    e.preventDefault()
    insertAtCaret('  ')
  }
}

/** 在光标处（或选区）插入文字；数据集侧栏点名称插入也走这里 */
async function insertAtCaret(text: string) {
  const t = ta.value
  if (!t) {
    emit('update:modelValue', props.modelValue + text)
    return
  }
  const start = t.selectionStart ?? t.value.length
  const end = t.selectionEnd ?? start
  emit('update:modelValue', t.value.slice(0, start) + text + t.value.slice(end))
  await nextTick()
  t.focus()
  t.setSelectionRange(start + text.length, start + text.length)
}

defineExpose({ insertAtCaret, focus: () => ta.value?.focus() })

const TYPE_LABEL = { dataset: '数据集', column: '列', keyword: '关键字', function: '函数' } as const
</script>

<template>
  <div class="sql-editor">
    <pre ref="layer" class="face layer" aria-hidden="true" v-html="html" />
    <div ref="mirror" class="face mirror" aria-hidden="true" />
    <textarea
      ref="ta"
      class="face input editor"
      :value="modelValue"
      spellcheck="false"
      autocomplete="off"
      autocapitalize="off"
      @input="onInput"
      @scroll="syncScroll"
      @keydown="onKeydown"
      @keydown.stop
      @click="refresh()"
      @blur="closePopup"
      @compositionstart="onComposition(true)"
      @compositionend="onComposition(false)"
    />
    <ul v-if="completion" class="popup" role="listbox" :style="{ left: `${popupPos.left}px`, top: `${popupPos.top}px` }" @mousedown.prevent>
      <li
        v-for="(it, i) in completion.items"
        :key="`${it.type}:${it.label}`"
        class="row"
        :class="{ on: i === active }"
        role="option"
        :aria-selected="i === active"
        @mouseenter="active = i"
        @click="accept(i)"
      >
        <span class="tag" :class="it.type">{{ TYPE_LABEL[it.type] }}</span>
        <span class="min-w-0 flex-1 truncate">{{ it.label }}</span>
        <span v-if="it.detail" class="detail">{{ it.detail }}</span>
      </li>
      <li class="hint">
        {{ $t('↑↓ 选择 · Tab / 回车确认 · Esc 关闭') }}
      </li>
    </ul>
  </div>
</template>

<style scoped lang="scss">
.sql-editor {
  @apply relative h-40 shrink-0 resize-y overflow-hidden rounded-md border border-[#e5e7eb] border-solid bg-white focus-within:border-[#217346];
}

/* 三层（着色层 / 镜像 / 输入层）必须排版完全一致，光标和高亮才对得上 */
.face {
  @apply m-0 box-border h-full w-full whitespace-pre-wrap break-words border-0 p-3 text-[13px] leading-5 font-mono;

  tab-size: 2;
  overflow-wrap: anywhere;
}

.layer {
  @apply pointer-events-none absolute inset-0 overflow-hidden text-[#1f2328];
}

.mirror {
  @apply invisible pointer-events-none absolute inset-0 overflow-hidden;

  height: auto;
  min-height: 100%;
}

.input {
  @apply absolute inset-0 resize-none bg-transparent outline-none;

  color: transparent;
  caret-color: #111827;
}

.layer :deep(.tk-keyword) {
  color: #0b5ed7;
  font-weight: 600;
}

.layer :deep(.tk-function) {
  color: #8250df;
}

.layer :deep(.tk-string) {
  color: #0a7a3d;
}

.layer :deep(.tk-number) {
  color: #b35900;
}

.layer :deep(.tk-comment) {
  color: #8c959f;
  font-style: italic;
}

.layer :deep(.tk-quoted) {
  color: #1f2328;
  text-decoration: underline dotted #9aa4b2;
}

.popup {
  @apply absolute z-20 m-0 w-[300px] list-none overflow-hidden rounded-md border border-[#e5e7eb] border-solid bg-white p-1 text-xs shadow-lg;
}

.row {
  @apply flex cursor-pointer items-center gap-2 rounded px-2 py-1;

  &.on {
    @apply bg-[#E7F4EC] text-[#0E5A31];
  }
}

.tag {
  @apply w-10 shrink-0 rounded px-1 text-center text-[10px] leading-4;

  &.dataset {
    @apply bg-[#E8F0FE] text-[#1a56db];
  }

  &.column {
    @apply bg-[#E7F4EC] text-[#0E5A31];
  }

  &.keyword {
    @apply bg-[#F1F5F9] text-[#475569];
  }

  &.function {
    @apply bg-[#F3E8FF] text-[#7e22ce];
  }
}

.detail {
  @apply max-w-28 shrink-0 truncate text-gray-400;
}

.hint {
  @apply mt-1 border-0 border-t border-t-solid border-t-[#f0efec] px-2 pt-1 text-[10px] text-gray-400;
}
</style>
