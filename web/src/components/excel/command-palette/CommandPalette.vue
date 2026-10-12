<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { matchInitials } from '@/utils/pinyin-initials'

/**
 * 命令面板（Alt+Q，对应 Excel 的「告诉我」搜索）：搜功能、数据集、工作表、工作簿，回车执行。
 * 不知道功能在哪个页签时直接搜名字；空输入时列出最近用过的
 */
export interface PaletteCommand {
  id: string
  title: string
  /** 分组：功能 / 工作表 / 数据集 / 工作簿 */
  group: string
  /** 右侧的补充说明（行数、快捷键等） */
  hint?: string
  /** 额外的搜索词（英文名、同义词） */
  keywords?: string
  run: () => void
}

const props = defineProps<{
  visible: boolean
  commands: PaletteCommand[]
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
}>()

const RECENT_KEY = 'excel.palette.recent'
const MAX_RESULTS = 50

const query = ref('')
const activeIndex = ref(0)
const inputRef = ref<HTMLInputElement>()
const listRef = ref<HTMLElement>()
const recentIds = ref<string[]>(JSON.parse(localStorage.getItem(RECENT_KEY) || '[]'))

watch(() => props.visible, async (v) => {
  if (!v)
    return
  query.value = ''
  activeIndex.value = 0
  await nextTick()
  inputRef.value?.focus()
})

/** 匹配得分：标题开头 > 标题包含 > 拼音首字母（sjj → 数据集）> 搜索词包含 > 按顺序包含每个字；0 为不匹配 */
function score(c: PaletteCommand, q: string) {
  const title = c.title.toLowerCase()
  if (title.startsWith(q))
    return 4
  if (title.includes(q))
    return 3
  if (matchInitials(c.title, q))
    return 2.5
  if (c.keywords?.toLowerCase().includes(q))
    return 2
  let i = 0
  for (const ch of title) {
    if (ch === q[i])
      i++
    if (i === q.length)
      return 1
  }
  return 0
}

const results = computed(() => {
  const q = query.value.trim().toLowerCase()
  if (!q) {
    const recent = recentIds.value
      .map(id => props.commands.find(c => c.id === id))
      .filter((c): c is PaletteCommand => !!c)
      .map(c => ({ ...c, group: '最近使用' }))
    const rest = props.commands.filter(c => c.group === '功能' && !recentIds.value.includes(c.id))
    return [...recent, ...rest].slice(0, MAX_RESULTS)
  }
  return props.commands
    .map((c, i) => ({ c, s: score(c, q), i }))
    .filter(x => x.s > 0)
    .sort((a, b) => b.s - a.s || a.i - b.i)
    .slice(0, MAX_RESULTS)
    .map(x => x.c)
})

/** 结果按分组显示（保持排序后的先后） */
const grouped = computed(() => {
  const groups: { name: string, items: { cmd: PaletteCommand, index: number }[] }[] = []
  results.value.forEach((cmd, index) => {
    let g = groups.find(x => x.name === cmd.group)
    if (!g)
      groups.push(g = { name: cmd.group, items: [] })
    g.items.push({ cmd, index })
  })
  // 分组后的显示顺序与 results 下标对应
  return groups
})

const flatOrder = computed(() => grouped.value.flatMap(g => g.items.map(x => x.index)))

watch(query, () => {
  activeIndex.value = flatOrder.value[0] ?? 0
})

function move(delta: number) {
  const order = flatOrder.value
  if (!order.length)
    return
  const pos = order.indexOf(activeIndex.value)
  activeIndex.value = order[(pos + delta + order.length) % order.length]!
  nextTick(() => listRef.value?.querySelector('[data-active="true"]')?.scrollIntoView({ block: 'nearest' }))
}

function close() {
  emit('update:visible', false)
}

function run(cmd: PaletteCommand | undefined) {
  if (!cmd)
    return
  recentIds.value = [cmd.id, ...recentIds.value.filter(id => id !== cmd.id)].slice(0, 5)
  localStorage.setItem(RECENT_KEY, JSON.stringify(recentIds.value))
  close()
  // 等面板关闭、焦点交还后再执行（部分命令会打开对话框）
  setTimeout(cmd.run, 0)
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'ArrowDown') {
    e.preventDefault()
    move(1)
  }
  else if (e.key === 'ArrowUp') {
    e.preventDefault()
    move(-1)
  }
  else if (e.key === 'Enter' && !e.isComposing) {
    e.preventDefault()
    run(results.value[activeIndex.value])
  }
  else if (e.key === 'Escape') {
    e.preventDefault()
    close()
  }
}
</script>

<template>
  <Teleport to="body">
    <div v-if="visible" class="fixed inset-0 z-3000 flex justify-center bg-black/20 pt-[12vh]" @mousedown.self="close">
      <div class="h-fit max-h-[70vh] w-[560px] flex flex-col overflow-hidden rounded-lg bg-white shadow-2xl" @keydown.stop="onKeydown">
        <div class="flex items-center gap-2 border-b border-[#eee] border-b-solid px-4">
          <svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="#9ca3af" stroke-width="1.6"><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5 14 14" /></svg>
          <input
            ref="inputRef"
            v-model="query"
            class="h-12 flex-1 border-none bg-transparent text-[15px] outline-none"
            :placeholder="$t('搜索功能、数据集、工作表、工作簿…')"
          >
          <kbd class="kbd">Esc</kbd>
        </div>
        <div ref="listRef" class="min-h-0 flex-1 overflow-y-auto py-1">
          <template v-for="g in grouped" :key="g.name">
            <div class="px-4 pb-1 pt-2 text-[11px] text-gray-400">
              {{ g.name }}
            </div>
            <div
              v-for="{ cmd, index } in g.items"
              :key="`${g.name}-${cmd.id}`"
              class="mx-1.5 flex cursor-pointer items-center gap-2 rounded px-3 py-1.5 text-[13px]"
              :class="index === activeIndex ? 'bg-[#E7F4EC] text-[#0E5A31]' : 'text-gray-700'"
              :data-active="index === activeIndex"
              @mousemove="activeIndex = index"
              @click="run(cmd)"
            >
              <span class="min-w-0 flex-1 truncate">{{ cmd.title }}</span>
              <span v-if="cmd.hint" class="shrink-0 text-xs text-gray-400">{{ cmd.hint }}</span>
            </div>
          </template>
          <div v-if="!results.length" class="px-4 py-8 text-center text-gray-400">
            {{ $t('没有找到「{0}」', [query]) }}
          </div>
        </div>
        <div class="flex gap-4 border-t border-[#eee] border-t-solid px-4 py-1.5 text-[11px] text-gray-400">
          <span><kbd class="kbd">↑</kbd> <kbd class="kbd">↓</kbd> {{ $t('选择') }}</span>
          <span><kbd class="kbd">Enter</kbd> {{ $t('执行') }}</span>
          <span class="ml-auto">{{ $t('Alt+Q 随时打开') }}</span>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped lang="scss">
.kbd {
  @apply rounded border border-[#e5e7eb] border-solid bg-[#f9fafb] px-1 text-[11px] text-gray-500 font-sans;
}
</style>
