import type { GridCell, GridColumn } from './types'
import type { CellValue } from '@/api/dataset'
import { isNumericKind } from '@/features/excel/dataset/column-kinds'
import { COLORS, FIELD_H, FILTER_BTN, FONT, HEAD_FONT, ROW_H, SMALL_FONT } from './grid-constants'
import { columnLetter, formatCell, integer } from './grid-format'

/** 归一化后的选区（r1 ≤ r2，c1 ≤ c2） */
export interface GridRange {
  r1: number
  r2: number
  c1: number
  c2: number
}

/** 画一帧所需要的全部状态：绘制是纯函数，不碰组件内部的 ref，便于单独测试与比对画面 */
export interface DrawState {
  width: number
  height: number
  dpr: number
  /** 可视区第一行 / 最后一行（含，已限制在总行数内） */
  firstRow: number
  lastRow: number
  /** 行号列宽度 / 各列左边缘（含末尾的总宽）/ 横向滚动 / 内容总宽 */
  numW: number
  colX: number[]
  scrollLeft: number
  contentW: number
  /** 字母行高度（嵌在工作表时为 0）/ 表头总高度 */
  letterH: number
  headH: number
  columns: GridColumn[]
  range: GridRange | null
  focus: GridCell | null
  sort?: { column: string, desc: boolean } | null
  filtered?: Set<string>
  /** 取已加载的行；还没加载出来时返回 undefined（画骨架占位） */
  cell: (row: number) => CellValue[] | undefined
  /** 分类汇总的小计行：返回各列显示的文字（已格式化）；不是小计行返回 null */
  /** 小计行：各列的文字，outer 为它外面还有几级（0 是最内层；越往外底色越深） */
  subtotal?: (row: number) => { cells: (string | null)[], outer: number } | null
  /** 行号列的文字（分类汇总时数据行显示数据行号、小计行空着）；省略为 row + 1 */
  rowLabel?: (row: number) => string
}

/** 当前可视范围内的列序号 */
/** 小计行的底色：最内层最浅 */
function subBg(outer: number): string {
  return outer > 0 ? COLORS.subBgOuter : COLORS.subBg
}

function visibleCols(st: DrawState): number[] {
  const left = st.scrollLeft
  const right = left + st.width - st.numW
  const out: number[] = []
  st.columns.forEach((_, i) => {
    if (st.colX[i + 1]! > left && st.colX[i]! < right)
      out.push(i)
  })
  return out
}

/** 画一帧：数据区 → 网格线 → 选区 → 列头 → 行号列 → 左上角 */
export function drawGrid(ctx: CanvasRenderingContext2D, st: DrawState) {
  ctx.setTransform(st.dpr, 0, 0, st.dpr, 0, 0)
  ctx.fillStyle = '#FFFFFF'
  ctx.fillRect(0, 0, st.width, st.height)

  const first = st.firstRow
  const last = st.lastRow
  const cols = visibleCols(st)
  const xOf = (c: number) => st.numW + st.colX[c]! - st.scrollLeft
  const yOf = (r: number) => st.headH + (r - first) * ROW_H
  const rg = st.range

  // 数据区（按列裁剪绘制文字，避免逐格 measureText）
  ctx.font = FONT
  ctx.textBaseline = 'middle'
  for (const c of cols) {
    const col = st.columns[c]!
    const x = xOf(c)
    ctx.save()
    ctx.beginPath()
    ctx.rect(Math.max(x, st.numW), st.headH, col.width, st.height - st.headH)
    ctx.clip()
    const right = isNumericKind(col.kind)
    ctx.textAlign = right ? 'right' : 'left'
    for (let r = first; r <= last; r++) {
      const y = yOf(r)
      const sub = st.subtotal?.(r)
      if (sub) {
        ctx.fillStyle = subBg(sub.outer)
        ctx.fillRect(x, y, col.width, ROW_H)
        const text = sub.cells[c]
        if (text) {
          ctx.fillStyle = COLORS.subText
          ctx.font = `600 ${FONT}`
          ctx.fillText(text, right ? x + col.width - 6 : x + 6, y + ROW_H / 2 + 1)
        }
        continue
      }
      const row = st.cell(r)
      if (!row) {
        ctx.fillStyle = COLORS.skeleton
        ctx.fillRect(x + 6, y + 8, Math.min(col.width - 12, 60 + ((r * 7 + c * 13) % 40)), 8)
        continue
      }
      const v = row[c]
      // 条件格式：只对屏幕上的格子求值，先铺底色（网格线、选区稍后画在上面）
      const hit = col.style?.(v) ?? null
      if (hit?.bg) {
        ctx.fillStyle = hit.bg
        ctx.fillRect(x, y, col.width, ROW_H)
      }
      const text = formatCell(v ?? null, col)
      if (!text)
        continue
      ctx.fillStyle = hit?.color ?? (typeof v === 'number' && v < 0 ? COLORS.negative : COLORS.text)
      ctx.font = col.emphasis || hit?.bold ? `600 ${FONT}` : FONT
      ctx.fillText(text, right ? x + col.width - 6 : x + 6, y + ROW_H / 2 + 1)
    }
    ctx.restore()
  }

  // 网格线
  ctx.strokeStyle = COLORS.grid
  ctx.lineWidth = 1
  ctx.beginPath()
  for (let r = first; r <= last + 1; r++) {
    const y = Math.round(yOf(r)) + 0.5
    ctx.moveTo(st.numW, y)
    ctx.lineTo(Math.min(st.width, st.contentW - st.scrollLeft), y)
  }
  for (const c of cols) {
    const x = Math.round(xOf(c) + st.columns[c]!.width) + 0.5
    if (x > st.numW) {
      ctx.moveTo(x, st.headH)
      ctx.lineTo(x, Math.min(st.height, yOf(last + 1)))
    }
  }
  ctx.stroke()

  // 选区
  if (rg) {
    const x1 = Math.max(st.numW, xOf(rg.c1))
    const x2 = Math.min(st.width, xOf(rg.c2) + st.columns[rg.c2]!.width)
    const y1 = Math.max(st.headH, yOf(rg.r1))
    const y2 = Math.min(st.height, yOf(rg.r2 + 1))
    if (x2 > x1 && y2 > y1) {
      ctx.fillStyle = COLORS.selFill
      ctx.fillRect(x1, y1, x2 - x1, y2 - y1)
      // 活动单元格不着色
      const f = st.focus!
      const fx = xOf(f.col)
      const fy = yOf(f.row)
      if (fy >= st.headH && fy < st.height && fx + st.columns[f.col]!.width > st.numW) {
        ctx.save()
        ctx.beginPath()
        ctx.rect(st.numW, st.headH, st.width, st.height)
        ctx.clip()
        ctx.clearRect(fx + 1, fy + 1, st.columns[f.col]!.width - 1, ROW_H - 1)
        const sub = st.subtotal?.(f.row)
        const v = sub ? null : st.cell(f.row)?.[f.col]
        const col = st.columns[f.col]!
        const hit = col.style?.(v) ?? null
        ctx.fillStyle = sub ? subBg(sub.outer) : hit?.bg ?? '#FFFFFF'
        ctx.fillRect(fx + 1, fy + 1, st.columns[f.col]!.width - 1, ROW_H - 1)
        const text = sub ? sub.cells[f.col] : formatCell(v ?? null, col)
        if (text) {
          ctx.beginPath()
          ctx.rect(fx, fy, col.width, ROW_H)
          ctx.clip()
          const right = isNumericKind(col.kind)
          ctx.textAlign = right ? 'right' : 'left'
          ctx.fillStyle = sub ? COLORS.subText : hit?.color ?? (typeof v === 'number' && v < 0 ? COLORS.negative : COLORS.text)
          ctx.font = sub || col.emphasis || hit?.bold ? `600 ${FONT}` : FONT
          ctx.fillText(text, right ? fx + col.width - 6 : fx + 6, fy + ROW_H / 2 + 1)
        }
        ctx.restore()
      }
      ctx.strokeStyle = COLORS.selBorder
      ctx.lineWidth = 2
      const top = yOf(rg.r1)
      const bottom = yOf(rg.r2 + 1)
      ctx.save()
      ctx.beginPath()
      ctx.rect(st.numW, st.headH, st.width, st.height)
      ctx.clip()
      ctx.strokeRect(xOf(rg.c1) + 1, top + 1, xOf(rg.c2) + st.columns[rg.c2]!.width - xOf(rg.c1) - 1, bottom - top - 1)
      ctx.restore()
    }
  }

  // 列头：字母行 + 字段行
  ctx.fillStyle = COLORS.headBg
  ctx.fillRect(0, 0, st.width, st.headH)
  for (const c of cols) {
    const col = st.columns[c]!
    const x = xOf(c)
    const selected = rg && c >= rg.c1 && c <= rg.c2
    if (selected) {
      ctx.fillStyle = COLORS.selHead
      ctx.fillRect(x, 0, col.width, st.headH)
    }
    ctx.save()
    ctx.beginPath()
    ctx.rect(Math.max(x, st.numW), 0, col.width, st.headH)
    ctx.clip()
    ctx.font = SMALL_FONT
    ctx.textAlign = 'center'
    ctx.fillStyle = selected ? COLORS.selHeadText : COLORS.headText
    if (st.letterH)
      ctx.fillText(columnLetter(c), x + col.width / 2, st.letterH / 2 + 1)
    ctx.font = HEAD_FONT
    ctx.textAlign = 'left'
    ctx.fillStyle = selected ? COLORS.selHeadText : COLORS.fieldText
    const sortMark = st.sort?.column === col.key ? (st.sort.desc ? ' ↓' : ' ↑') : ''
    ctx.fillText(col.title + sortMark, x + 6, st.letterH + FIELD_H / 2 + 1)
    // 筛选按钮
    const active = st.filtered?.has(col.key) || st.sort?.column === col.key
    const bx = x + col.width - FILTER_BTN - 3
    const by = st.letterH + (FIELD_H - FILTER_BTN) / 2
    ctx.fillStyle = active ? COLORS.accent : '#FFFFFF'
    ctx.strokeStyle = active ? COLORS.accent : '#C8C8C8'
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.roundRect(bx + 0.5, by + 0.5, FILTER_BTN - 1, FILTER_BTN - 1, 3)
    ctx.fill()
    ctx.stroke()
    ctx.fillStyle = active ? '#FFFFFF' : '#6B6B6B'
    ctx.beginPath()
    if (st.filtered?.has(col.key)) {
      // 漏斗
      ctx.moveTo(bx + 4.5, by + 5)
      ctx.lineTo(bx + 13.5, by + 5)
      ctx.lineTo(bx + 10, by + 9.5)
      ctx.lineTo(bx + 10, by + 13.5)
      ctx.lineTo(bx + 8, by + 12.5)
      ctx.lineTo(bx + 8, by + 9.5)
    }
    else {
      // 下拉三角
      ctx.moveTo(bx + 5.5, by + 7.5)
      ctx.lineTo(bx + 12.5, by + 7.5)
      ctx.lineTo(bx + 9, by + 11.5)
    }
    ctx.closePath()
    ctx.fill()
    ctx.restore()
  }
  ctx.strokeStyle = COLORS.grid
  ctx.lineWidth = 1
  ctx.beginPath()
  ctx.moveTo(0, st.letterH + 0.5)
  ctx.lineTo(st.width, st.letterH + 0.5)
  ctx.moveTo(0, st.headH - 0.5)
  ctx.lineTo(st.width, st.headH - 0.5)
  for (const c of cols) {
    const x = Math.round(xOf(c) + st.columns[c]!.width) + 0.5
    if (x > st.numW) {
      ctx.moveTo(x, 0)
      ctx.lineTo(x, st.headH)
    }
  }
  ctx.stroke()

  // 行号列
  ctx.fillStyle = COLORS.headBg
  ctx.fillRect(0, st.headH, st.numW, st.height - st.headH)
  ctx.font = SMALL_FONT
  ctx.textAlign = 'right'
  for (let r = first; r <= last; r++) {
    const y = yOf(r)
    const selected = rg && r >= rg.r1 && r <= rg.r2
    if (selected) {
      ctx.fillStyle = COLORS.selHead
      ctx.fillRect(0, y, st.numW, ROW_H)
    }
    ctx.fillStyle = selected ? COLORS.selHeadText : COLORS.headText
    ctx.fillText(st.rowLabel ? st.rowLabel(r) : integer.format(r + 1), st.numW - 6, y + ROW_H / 2 + 1)
  }
  ctx.beginPath()
  ctx.strokeStyle = COLORS.grid
  for (let r = first; r <= last + 1; r++) {
    const y = Math.round(yOf(r)) + 0.5
    ctx.moveTo(0, y)
    ctx.lineTo(st.numW, y)
  }
  ctx.moveTo(st.numW - 0.5, 0)
  ctx.lineTo(st.numW - 0.5, st.height)
  ctx.stroke()
  // 左上角
  ctx.fillStyle = COLORS.headBg
  ctx.fillRect(0, 0, st.numW - 1, st.headH - 1)
}
