<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import type { Dim, OutlineGroup } from '@/features/excel/outline/outline'
import { onBeforeUnmount, ref, shallowRef, watch } from 'vue'
import { getOutline, groupsOf, maxLevel, onOutlineChanged, showLevel, toggleGroup } from '@/features/excel/outline/outline'
import { t } from '@/i18n'

/**
 * 分组栏：叠加在行号左侧 / 列标上方，画分组线和 −/+ 按钮，左上角是级别按钮 1 2 3（与 Excel 一致）。
 * 有分组时加宽行号区 / 加高列标区留出位置；位置按画布上单元格的坐标计算，滚动、改行高后重画
 */
const props = defineProps<{
  univerApi: FUniver | null
  /** 工作表视图是否可见（数据集视图时隐藏） */
  visible: boolean
}>()

/** 每级占用的宽度 */
const STEP = 14
const BASE_ROW_HEADER = 46
const BASE_COL_HEADER = 20

interface Item {
  key: string
  kind: 'line' | 'btn' | 'level'
  style: Record<string, string>
  label?: string
  title?: string
  onClick?: () => void
}

const items = shallowRef<Item[]>([])
const sheetRef = shallowRef<FWorksheet | null>(null)
const px = (n: number) => `${Math.round(n)}px`

function mainCanvas(): DOMRect | null {
  const list = [...document.querySelectorAll<HTMLCanvasElement>('#univer-container canvas')]
  let best: DOMRect | null = null
  for (const c of list) {
    const r = c.getBoundingClientRect()
    if (r.width > 0 && r.height > 0 && (!best || r.width * r.height > best.width * best.height))
      best = r
  }
  return best
}

interface HeaderConfig { rowHeader?: { width?: number }, columnHeader?: { height?: number } }

function headerSize(sheet: FWorksheet) {
  const cfg = (sheet as unknown as { getSheet: () => { getConfig: () => HeaderConfig } }).getSheet().getConfig()
  return { w: cfg.rowHeader?.width ?? BASE_ROW_HEADER, h: cfg.columnHeader?.height ?? BASE_COL_HEADER }
}

type SizedSheet = FWorksheet & { setRowHeaderWidth: (w: number) => void, setColumnHeaderHeight: (h: number) => void }

function draw() {
  const api = props.univerApi
  const sheet = api?.getActiveWorkbook()?.getActiveSheet() as SizedSheet | undefined
  sheetRef.value = sheet ?? null
  const canvas = props.visible ? mainCanvas() : null
  if (!sheet || !canvas) {
    items.value = []
    return
  }
  const o = getOutline(sheet)
  const rowMax = maxLevel(o, 'rows')
  const colMax = maxLevel(o, 'cols')
  // 行号区 / 列标区留出分组栏
  const wantW = BASE_ROW_HEADER + (rowMax ? (rowMax + 1) * STEP + 4 : 0)
  const wantH = BASE_COL_HEADER + (colMax ? (colMax + 1) * STEP + 4 : 0)
  const cur = headerSize(sheet)
  if (cur.w !== wantW && (rowMax || cur.w > BASE_ROW_HEADER))
    sheet.setRowHeaderWidth(wantW)
  if (cur.h !== wantH && (colMax || cur.h > BASE_COL_HEADER))
    sheet.setColumnHeaderHeight(wantH)
  if (!rowMax && !colMax) {
    items.value = []
    return
  }
  const { w: headerW, h: headerH } = headerSize(sheet)
  const out: Item[] = []
  const rect = (r: number, c: number) => sheet.getRange(r, c).getCellRect()

  const lineColor = '#8C8C8C'
  const drawGroups = (dim: Dim, groups: OutlineGroup[]) => {
    for (const g of groups) {
      const slot = 2 + (g.level - 1) * STEP
      if (dim === 'rows') {
        const top = rect(g.start, 0).top
        const endR = rect(g.end, 0)
        const bottom = endR.top + endR.height
        const y1 = Math.max(top, headerH)
        const y2 = Math.min(bottom, canvas.height)
        const x = canvas.left + slot + STEP / 2
        if (!g.collapsed && y2 > y1 + 1) {
          out.push({ key: `rl${g.level}-${g.start}`, kind: 'line', style: { left: px(x), top: px(canvas.top + y1 + 3), width: '1px', height: px(y2 - y1 - 3), background: lineColor } })
          if (top >= headerH)
            out.push({ key: `rt${g.level}-${g.start}`, kind: 'line', style: { left: px(x), top: px(canvas.top + top + 3), width: '4px', height: '1px', background: lineColor } })
        }
        const btn = rect(g.end + 1, 0)
        const cy = btn.top + btn.height / 2
        if (btn.height > 0 && cy > headerH && cy < canvas.height)
          out.push({ key: `rb${g.level}-${g.end}`, kind: 'btn', label: g.collapsed ? '+' : '−', title: g.collapsed ? t('展开分组') : t('折叠分组'), style: { left: px(x - 6), top: px(canvas.top + cy - 6) }, onClick: () => toggleGroup(sheet, 'rows', g) })
      }
      else {
        const left = rect(0, g.start).left
        const endC = rect(0, g.end)
        const right = endC.left + endC.width
        const x1 = Math.max(left, headerW)
        const x2 = Math.min(right, canvas.width)
        const y = canvas.top + slot + STEP / 2
        if (!g.collapsed && x2 > x1 + 1) {
          out.push({ key: `cl${g.level}-${g.start}`, kind: 'line', style: { left: px(canvas.left + x1 + 3), top: px(y), width: px(x2 - x1 - 3), height: '1px', background: lineColor } })
          if (left >= headerW)
            out.push({ key: `ct${g.level}-${g.start}`, kind: 'line', style: { left: px(canvas.left + left + 3), top: px(y), width: '1px', height: '4px', background: lineColor } })
        }
        const btn = rect(0, g.end + 1)
        const cx = btn.left + btn.width / 2
        if (btn.width > 0 && cx > headerW && cx < canvas.width)
          out.push({ key: `cb${g.level}-${g.end}`, kind: 'btn', label: g.collapsed ? '+' : '−', title: g.collapsed ? t('展开分组') : t('折叠分组'), style: { left: px(canvas.left + cx - 6), top: px(y - 6) }, onClick: () => toggleGroup(sheet, 'cols', g) })
      }
    }
  }
  drawGroups('rows', groupsOf(o, 'rows'))
  drawGroups('cols', groupsOf(o, 'cols'))

  // 级别按钮：行的在左上角底部横排，列的在右侧竖排
  const rowLevels = rowMax ? rowMax + 1 : 0
  const colLevels = colMax ? colMax + 1 : 0
  for (let k = 1; k <= rowLevels; k++)
    out.push({ key: `rlv${k}`, kind: 'level', label: String(k), title: k === 1 ? t('只显示汇总（折叠全部行分组）') : t('显示到第 {0} 级', [k]), style: { left: px(canvas.left + 2 + (k - 1) * STEP), top: px(canvas.top + headerH - 14) }, onClick: () => showLevel(sheet, 'rows', k) })
  for (let k = 1; k <= colLevels; k++)
    out.push({ key: `clv${k}`, kind: 'level', label: String(k), title: k === 1 ? t('只显示汇总（折叠全部列分组）') : t('显示到第 {0} 级', [k]), style: { left: px(canvas.left + headerW - 14), top: px(canvas.top + 2 + (k - 1) * STEP) }, onClick: () => showLevel(sheet, 'cols', k) })
  items.value = out
}

let frame = 0
function schedule() {
  if (frame)
    return
  frame = requestAnimationFrame(() => {
    frame = 0
    try {
      draw()
    }
    catch {
      items.value = []
    }
  })
}

const offs: (() => void)[] = []
watch(() => props.univerApi, (api) => {
  offs.splice(0).forEach(off => off())
  if (!api)
    return
  const subs = [
    api.addEvent(api.Event.CommandExecuted, schedule),
    api.addEvent(api.Event.ActiveSheetChanged, schedule),
    api.addEvent(api.Event.Scroll, schedule),
  ]
  offs.push(() => subs.forEach(s => s.dispose()), onOutlineChanged(schedule))
  schedule()
}, { immediate: true })
watch(() => props.visible, schedule)
const onResize = () => schedule()
window.addEventListener('resize', onResize)
// 面板开合、布局变化不一定有事件：低频兜底
const timer = setInterval(schedule, 1000)
onBeforeUnmount(() => {
  offs.forEach(off => off())
  window.removeEventListener('resize', onResize)
  clearInterval(timer)
  cancelAnimationFrame(frame)
})

const hovered = ref('')
</script>

<template>
  <div class="outline-gutter">
    <template v-for="it in items" :key="it.key">
      <div v-if="it.kind === 'line'" class="line" :style="it.style" />
      <button
        v-else
        type="button"
        :class="[it.kind, { hover: hovered === it.key }]"
        :style="it.style"
        :title="it.title"
        @mouseenter="hovered = it.key"
        @mouseleave="hovered = ''"
        @mousedown.prevent.stop
        @click.stop="it.onClick?.()"
      >
        {{ it.label }}
      </button>
    </template>
  </div>
</template>

<style scoped>
.outline-gutter {
  position: fixed;
  left: 0;
  top: 0;
  width: 0;
  height: 0;
  /* 画布本身是 z-index 8 */
  z-index: 50;
  pointer-events: none;
}

.line {
  position: fixed;
}

.btn,
.level {
  position: fixed;
  width: 12px;
  height: 12px;
  padding: 0;
  border: 1px solid #8c8c8c;
  border-radius: 2px;
  background: #fff;
  color: #333;
  font-size: 11px;
  line-height: 9px;
  text-align: center;
  cursor: pointer;
  pointer-events: auto;
  font-family: system-ui, sans-serif;
}

.level {
  font-size: 9px;
  line-height: 10px;
}

.btn.hover,
.level.hover {
  border-color: #217346;
  color: #217346;
}
</style>
