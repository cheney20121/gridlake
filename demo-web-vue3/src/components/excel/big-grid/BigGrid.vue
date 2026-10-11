<script setup lang="ts">
import type { Hit, HitGeometry } from './grid-hit'
import type { GridColumn, GridSelection } from './types'
import type { CellValue } from '@/api/dataset'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { FIELD_H, FILTER_BTN, letterHeight, MAX_SCROLL, ROW_H } from './grid-constants'
import { drawGrid } from './grid-draw'
import { colAt, hitTest } from './grid-hit'
import { RowCache } from './row-cache'

/**
 * 大数据表：canvas 绘制、按可视区域取数，支撑千万行
 *
 * 浏览器单个元素高度上限约 3,300 万 px（1000 万行 × 24 px 远超），滚动条按比例映射：
 * 行数少时 1 px = 1 px；超过上限时滚动条位置 → 首行行号 线性换算，滚轮仍按行滚动
 */
const props = defineProps<{
  datasetId: string
  columns: GridColumn[]
  cursorId: string
  total: number
  /** 当前排序（列头显示箭头） */
  sort?: { column: string, desc: boolean } | null
  /** 有筛选条件的列（列头按钮高亮） */
  filtered?: Set<string>
  /** 按列名取数（行数据与 columns 一一对应，工作表里的数据表只显示部分列）；省略时取全部列 */
  fields?: string[]
  /** 不显示 A / B / C 列字母行（嵌在工作表里时避免与工作表列号混淆） */
  hideLetters?: boolean
}>()

const emit = defineEmits<{
  (e: 'selectionChange', sel: GridSelection | null): void
  /** 点击列头的筛选按钮；rect 为按钮在视口中的位置 */
  (e: 'headerMenu', column: GridColumn, rect: DOMRect): void
  (e: 'resizeColumn', key: string, width: number): void
  (e: 'rowOpen', row: number, values: CellValue[]): void
  /** 游标失效，需要父组件重新建游标 */
  (e: 'expired'): void
  /** 按了 Ctrl / ⌘ + G：请父组件弹出「跳转到行」 */
  (e: 'gotoRequest'): void
}>()

const LETTER_H = letterHeight(props.hideLetters)
const HEAD_H = LETTER_H + FIELD_H

const wrapRef = ref<HTMLDivElement>()
const canvasRef = ref<HTMLCanvasElement>()
const scrollerRef = ref<HTMLDivElement>()

const width = ref(0)
const height = ref(0)
const firstRow = ref(0)
const scrollLeft = ref(0)
const sel = ref<GridSelection | null>(null)

const cache = new RowCache(props.datasetId, () => requestDraw(), () => emit('expired'), props.fields)

// ── 几何 ──────────────────────────────────────────────
const numW = computed(() => Math.max(48, String(props.total).length * 8 + 20))
const colX = computed(() => {
  const xs = [0]
  for (const c of props.columns)
    xs.push(xs[xs.length - 1]! + c.width)
  return xs
})
const contentW = computed(() => numW.value + colX.value[colX.value.length - 1]!)
const viewRows = computed(() => Math.max(1, Math.floor((height.value - HEAD_H) / ROW_H)))
const maxFirst = computed(() => Math.max(0, props.total - viewRows.value))
const contentH = computed(() => props.total * ROW_H)
const scaled = computed(() => contentH.value > MAX_SCROLL)
const spacerH = computed(() => (scaled.value ? MAX_SCROLL : contentH.value) + HEAD_H)

function rowFromScrollTop(top: number) {
  if (!scaled.value)
    return Math.floor(top / ROW_H)
  const range = MAX_SCROLL - (height.value - HEAD_H)
  return Math.round((top / Math.max(1, range)) * maxFirst.value)
}

function scrollTopFromRow(row: number) {
  if (!scaled.value)
    return row * ROW_H
  const range = MAX_SCROLL - (height.value - HEAD_H)
  return maxFirst.value ? (row / maxFirst.value) * range : 0
}

let programmaticScroll = false
function setFirstRow(row: number) {
  const r = Math.min(maxFirst.value, Math.max(0, Math.round(row)))
  firstRow.value = r
  const el = scrollerRef.value
  if (el) {
    programmaticScroll = true
    el.scrollTop = scrollTopFromRow(r)
  }
  requestDraw()
}

function onScroll() {
  const el = scrollerRef.value!
  scrollLeft.value = el.scrollLeft
  if (programmaticScroll)
    programmaticScroll = false
  else
    firstRow.value = Math.min(maxFirst.value, Math.max(0, rowFromScrollTop(el.scrollTop)))
  requestDraw()
}

/** 滚轮按行滚动（缩放模式下 1 px 可能对应多行，必须接管） */
let wheelAcc = 0
function onWheel(e: WheelEvent) {
  if (e.ctrlKey)
    return
  e.preventDefault()
  const dy = e.deltaMode === 1 ? e.deltaY * ROW_H : e.deltaMode === 2 ? e.deltaY * (height.value - HEAD_H) : e.deltaY
  wheelAcc += dy / ROW_H
  const step = Math.trunc(wheelAcc)
  if (step) {
    wheelAcc -= step
    setFirstRow(firstRow.value + step)
  }
  if (e.deltaX || e.shiftKey) {
    const el = scrollerRef.value!
    el.scrollLeft += e.deltaX || e.deltaY
  }
}

// ── 绘制 ──────────────────────────────────────────────
/** 当前选区归一化（r1 ≤ r2，c1 ≤ c2） */
const range = computed(() => {
  const s = sel.value
  if (!s)
    return null
  return {
    r1: Math.min(s.anchor.row, s.focus.row),
    r2: Math.max(s.anchor.row, s.focus.row),
    c1: Math.min(s.anchor.col, s.focus.col),
    c2: Math.max(s.anchor.col, s.focus.col),
  }
})

let frame = 0
function requestDraw() {
  if (!frame)
    frame = requestAnimationFrame(draw)
}

function draw() {
  frame = 0
  const ctx = canvasRef.value?.getContext('2d')
  if (!ctx || !width.value)
    return
  const first = firstRow.value
  const last = Math.min(props.total - 1, first + viewRows.value)
  cache.ensure(first - 40, last + 120)
  drawGrid(ctx, {
    width: width.value,
    height: height.value,
    dpr: window.devicePixelRatio || 1,
    firstRow: first,
    lastRow: last,
    numW: numW.value,
    colX: colX.value,
    scrollLeft: scrollLeft.value,
    contentW: contentW.value,
    letterH: LETTER_H,
    headH: HEAD_H,
    columns: props.columns,
    range: range.value,
    focus: sel.value?.focus ?? null,
    sort: props.sort,
    filtered: props.filtered,
    cell: r => cache.get(r),
  })
}

// ── 命中测试 ──────────────────────────────────────────
function geometry(): HitGeometry {
  return {
    numW: numW.value,
    scrollLeft: scrollLeft.value,
    colX: colX.value,
    colCount: props.columns.length,
    letterH: LETTER_H,
    headH: HEAD_H,
    firstRow: firstRow.value,
    total: props.total,
  }
}

function hitAt(e: MouseEvent): Hit {
  const rect = scrollerRef.value!.getBoundingClientRect()
  return hitTest(geometry(), e.clientX - rect.left, e.clientY - rect.top)
}

// ── 选择与交互 ────────────────────────────────────────
function setSelection(next: GridSelection | null) {
  sel.value = next
  requestDraw()
  emit('selectionChange', next)
}

function ensureVisible(row: number, col: number) {
  if (row < firstRow.value)
    setFirstRow(row)
  else if (row >= firstRow.value + viewRows.value)
    setFirstRow(row - viewRows.value + 1)
  const el = scrollerRef.value!
  const left = colX.value[col]!
  const right = colX.value[col + 1]!
  const viewW = width.value - numW.value
  if (left < el.scrollLeft)
    el.scrollLeft = left
  else if (right > el.scrollLeft + viewW)
    el.scrollLeft = right - viewW
}

let drag: { kind: 'select' | 'rows' | 'cols' } | { kind: 'resize', col: number, startX: number, startW: number } | null = null
let autoScroll = 0

function onMouseDown(e: MouseEvent) {
  scrollerRef.value?.focus()
  if (e.button !== 0)
    return
  const hit = hitAt(e)
  const lastRow = Math.max(0, props.total - 1)
  const lastCol = props.columns.length - 1
  const extend = e.shiftKey && sel.value
  switch (hit.area) {
    case 'corner':
      setSelection({ anchor: { row: 0, col: 0 }, focus: { row: lastRow, col: lastCol } })
      break
    case 'filter': {
      const rect = scrollerRef.value!.getBoundingClientRect()
      const right = rect.left + numW.value + colX.value[hit.col + 1]! - scrollLeft.value
      emit('headerMenu', props.columns[hit.col]!, new DOMRect(right - FILTER_BTN - 3, rect.top + LETTER_H, FILTER_BTN, FIELD_H))
      break
    }
    case 'resize':
      drag = { kind: 'resize', col: hit.col, startX: e.clientX, startW: props.columns[hit.col]!.width }
      break
    case 'col':
      setSelection(extend
        ? { anchor: { row: 0, col: sel.value!.anchor.col }, focus: { row: lastRow, col: hit.col } }
        : { anchor: { row: 0, col: hit.col }, focus: { row: lastRow, col: hit.col } })
      drag = { kind: 'cols' }
      break
    case 'row':
      setSelection(extend
        ? { anchor: { row: sel.value!.anchor.row, col: 0 }, focus: { row: hit.row, col: lastCol } }
        : { anchor: { row: hit.row, col: 0 }, focus: { row: hit.row, col: lastCol } })
      drag = { kind: 'rows' }
      break
    case 'cell':
      setSelection(extend
        ? { anchor: sel.value!.anchor, focus: { row: hit.row, col: hit.col } }
        : { anchor: { row: hit.row, col: hit.col }, focus: { row: hit.row, col: hit.col } })
      drag = { kind: 'select' }
      break
  }
  if (drag) {
    window.addEventListener('mousemove', onDragMove)
    window.addEventListener('mouseup', onDragEnd, { once: true })
  }
}

let lastDragEvent: MouseEvent | null = null
function onDragMove(e: MouseEvent) {
  lastDragEvent = e
  if (!drag)
    return
  if (drag.kind === 'resize') {
    const w = Math.max(40, Math.round(drag.startW + e.clientX - drag.startX))
    emit('resizeColumn', props.columns[drag.col]!.key, w)
    return
  }
  extendDragTo(e)
  // 拖出可视区时自动滚动
  const rect = scrollerRef.value!.getBoundingClientRect()
  const dir = e.clientY > rect.bottom - 8 ? 1 : e.clientY < rect.top + HEAD_H ? -1 : 0
  if (dir && !autoScroll) {
    autoScroll = window.setInterval(() => {
      setFirstRow(firstRow.value + dir * 3)
      if (lastDragEvent)
        extendDragTo(lastDragEvent)
    }, 40)
  }
  else if (!dir && autoScroll) {
    clearInterval(autoScroll)
    autoScroll = 0
  }
}

function extendDragTo(e: MouseEvent) {
  if (!sel.value || !drag || drag.kind === 'resize')
    return
  const rect = scrollerRef.value!.getBoundingClientRect()
  const py = Math.min(Math.max(e.clientY - rect.top, HEAD_H), rect.height - 1)
  const px = Math.min(Math.max(e.clientX - rect.left, numW.value), rect.width - 1)
  const row = Math.min(props.total - 1, firstRow.value + Math.floor((py - HEAD_H) / ROW_H))
  const col = Math.max(0, colAt(geometry(), px) < 0 ? props.columns.length - 1 : colAt(geometry(), px))
  const lastRow = Math.max(0, props.total - 1)
  const lastCol = props.columns.length - 1
  const focus = drag.kind === 'rows' ? { row, col: lastCol } : drag.kind === 'cols' ? { row: lastRow, col } : { row, col }
  if (focus.row !== sel.value.focus.row || focus.col !== sel.value.focus.col)
    setSelection({ anchor: sel.value.anchor, focus })
}

function onDragEnd() {
  window.removeEventListener('mousemove', onDragMove)
  if (autoScroll) {
    clearInterval(autoScroll)
    autoScroll = 0
  }
  drag = null
}

function onHover(e: MouseEvent) {
  if (drag)
    return
  const hit = hitAt(e)
  scrollerRef.value!.style.cursor = hit.area === 'resize' ? 'col-resize' : hit.area === 'filter' ? 'pointer' : 'cell'
}

function onDblClick(e: MouseEvent) {
  const hit = hitAt(e)
  if (hit.area === 'resize') {
    // 双击列边缘：按表头文字宽度 + 余量适配
    const col = props.columns[hit.col]!
    emit('resizeColumn', col.key, Math.max(80, col.title.length * 14 + 40))
    return
  }
  if (hit.area === 'cell' || hit.area === 'row') {
    const values = cache.get(hit.row)
    if (values)
      emit('rowOpen', hit.row, values)
  }
}

function onKeyDown(e: KeyboardEvent) {
  const s = sel.value
  const mod = e.metaKey || e.ctrlKey
  if (mod && e.key.toLowerCase() === 'c') {
    e.preventDefault()
    copySelection()
    return
  }
  if (mod && e.key.toLowerCase() === 'g') {
    e.preventDefault()
    emit('gotoRequest')
    return
  }
  if (mod && e.key.toLowerCase() === 'a') {
    e.preventDefault()
    setSelection({ anchor: { row: 0, col: 0 }, focus: { row: Math.max(0, props.total - 1), col: props.columns.length - 1 } })
    return
  }
  if (!s || !props.total)
    return
  const lastRow = props.total - 1
  const lastCol = props.columns.length - 1
  let { row, col } = s.focus
  const moves: Record<string, () => void> = {
    ArrowDown: () => { row = mod ? lastRow : row + 1 },
    ArrowUp: () => { row = mod ? 0 : row - 1 },
    ArrowRight: () => { col = mod ? lastCol : col + 1 },
    ArrowLeft: () => { col = mod ? 0 : col - 1 },
    PageDown: () => { row += viewRows.value },
    PageUp: () => { row -= viewRows.value },
    Home: () => {
      col = 0
      if (mod)
        row = 0
    },
    End: () => {
      col = lastCol
      if (mod)
        row = lastRow
    },
    Enter: () => { row += e.shiftKey ? -1 : 1 },
    Tab: () => { col += e.shiftKey ? -1 : 1 },
  }
  const move = moves[e.key]
  if (!move)
    return
  move()
  e.preventDefault()
  row = Math.min(lastRow, Math.max(0, row))
  col = Math.min(lastCol, Math.max(0, col))
  const extend = e.shiftKey && e.key !== 'Enter' && e.key !== 'Tab'
  setSelection({ anchor: extend ? s.anchor : { row, col }, focus: { row, col } })
  ensureVisible(row, col)
}

const COPY_LIMIT = 20_000

/** 复制选区为制表符分隔文本（可直接粘贴到 Excel / 汇总表） */
async function copySelection() {
  const rg = range.value
  if (!rg)
    return
  const rows = rg.r2 - rg.r1 + 1
  if (rows > COPY_LIMIT) {
    ElMessage.warning(`一次最多复制 ${COPY_LIMIT.toLocaleString()} 行（当前选中 ${rows.toLocaleString()} 行），大量数据请用导出`)
    return
  }
  try {
    const data = rows <= 1 && cache.get(rg.r1) ? [cache.get(rg.r1)!] : await cache.fetchRange(rg.r1, rg.r2)
    const cols = props.columns.slice(rg.c1, rg.c2 + 1)
    const header = rows > 1 ? `${cols.map(c => c.title).join('\t')}\n` : ''
    const body = data.map(r => cols.map((c, i) => {
      const v = r[rg.c1 + i]
      return v == null ? '' : String(v).replace(/[\t\n]/g, ' ')
    }).join('\t')).join('\n')
    await navigator.clipboard.writeText(header + body)
    ElMessage.success(`已复制 ${rows.toLocaleString()} 行 × ${cols.length} 列${rows > 1 ? '（含表头）' : ''}`)
  }
  catch (e) {
    ElMessage.error(`复制失败：${e instanceof Error ? e.message : String(e)}`)
  }
}

// ── 生命周期 ──────────────────────────────────────────
let observer: ResizeObserver | null = null

function resize() {
  const wrap = wrapRef.value
  const canvas = canvasRef.value
  if (!wrap || !canvas)
    return
  const dpr = window.devicePixelRatio || 1
  width.value = wrap.clientWidth
  height.value = wrap.clientHeight
  canvas.width = Math.round(width.value * dpr)
  canvas.height = Math.round(height.value * dpr)
  canvas.style.width = `${width.value}px`
  canvas.style.height = `${height.value}px`
  firstRow.value = Math.min(firstRow.value, maxFirst.value)
  requestDraw()
}

onMounted(() => {
  observer = new ResizeObserver(resize)
  observer.observe(wrapRef.value!)
  resize()
})

onBeforeUnmount(() => {
  observer?.disconnect()
  cancelAnimationFrame(frame)
  onDragEnd()
})

watch(() => [props.cursorId, props.total] as const, ([cursorId, total]) => {
  cache.reset(cursorId, total)
  setSelection(null)
  setFirstRow(0)
}, { immediate: true })

watch(() => [props.columns, props.sort, props.filtered], requestDraw, { deep: true })

/** 跳到第 row 行（从 0 数）：选中该行第一格，并把它放在可视区域中间 */
function goToRow(row: number) {
  if (!props.total)
    return
  const r = Math.min(props.total - 1, Math.max(0, Math.round(row)))
  setSelection({ anchor: { row: r, col: 0 }, focus: { row: r, col: 0 } })
  setFirstRow(r - Math.floor(viewRows.value / 2))
  scrollerRef.value?.focus()
}

defineExpose({
  copySelection,
  goToRow,
  scrollToRow: (row: number) => setFirstRow(row),
  focus: () => scrollerRef.value?.focus(),
})
</script>

<template>
  <div ref="wrapRef" class="relative h-full w-full overflow-hidden bg-white">
    <canvas ref="canvasRef" class="absolute left-0 top-0" />
    <!-- 透明滚动层：提供原生滚动条并接收鼠标 / 键盘事件，内容由 canvas 绘制 -->
    <div
      ref="scrollerRef"
      class="scroller absolute inset-0 overflow-auto outline-none"
      tabindex="0"
      @scroll="onScroll"
      @wheel="onWheel"
      @mousedown="onMouseDown"
      @mousemove="onHover"
      @dblclick="onDblClick"
      @keydown="onKeyDown"
    >
      <div :style="{ width: `${contentW}px`, height: `${spacerH}px` }" />
    </div>
  </div>
</template>

<style scoped lang="scss">
.scroller {
  scrollbar-width: auto;
}
</style>
