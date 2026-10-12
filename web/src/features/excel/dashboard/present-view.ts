import type { FUniver } from '@univerjs/core/facade'

/**
 * 演示模式隐藏网格线与行号列标（行号列标见 hideHeadersForPresent）。
 *
 * 网格线：网格线开关是工作表数据（会被保存），所以只在演示期间临时关掉，
 * 退出时按进入前的状态恢复；用变更（mutation）直接改，不进撤销栈。调用方需要用 persistenceManager.withoutTracking 包住，
 * 否则这两次改动会让工作簿显示为「未保存」
 */
const TOGGLE = 'sheet.mutation.toggle-gridlines'

interface Target { unitId: string, subUnitId: string }

async function setGridlines(api: FUniver, t: Target, show: boolean) {
  await api.executeCommand(TOGGLE, { unitId: t.unitId, subUnitId: t.subUnitId, showGridlines: show ? 1 : 0 }).catch(() => false)
}

/** 隐藏当前工作簿所有显示着网格线的工作表的网格线；返回恢复函数 */
export async function hideGridlinesForPresent(api: FUniver): Promise<() => Promise<void>> {
  const wb = api.getActiveWorkbook()
  if (!wb)
    return async () => {}
  const unitId = wb.getId()
  const changed: Target[] = []
  for (const sheet of wb.getSheets()) {
    if (sheet.hasHiddenGridLines())
      continue
    const t = { unitId, subUnitId: sheet.getSheetId() }
    await setGridlines(api, t, false)
    changed.push(t)
  }
  return async () => {
    // 演示期间工作簿被换掉时，原来的工作表已不存在，不再恢复
    if (api.getActiveWorkbook()?.getId() !== unitId)
      return
    for (const t of changed)
      await setGridlines(api, t, true)
  }
}

/**
 * 隐藏行号列标：不改 Univer 的数据和骨架，只用 CSS 把画布整体向左上挪出行号列宽、列标高度，由外层容器裁掉。
 * 浮动部件（图表、切片器、指标卡）和画布在同一个容器里，一起平移，与单元格的相对位置不变
 * （直接把行列标题尺寸设为 0 的话，部件仍按原来的画布坐标摆放，会和单元格错开一个标题的宽高）；
 * 鼠标命中按元素的实际位置计算，同样不受影响。行号列宽随行数自动变宽，按当前工作表的实际尺寸挪；切换工作表时更新。返回恢复函数
 */
export function hideHeadersForPresent(api: FUniver, container: HTMLElement | null): () => void {
  if (!container)
    return () => {}
  const apply = () => {
    const sk = api.getActiveWorkbook()?.getActiveSheet()?.getSkeleton()
    container.style.setProperty('--present-hx', `${sk?.rowHeaderWidth ?? 0}px`)
    container.style.setProperty('--present-hy', `${sk?.columnHeaderHeight ?? 0}px`)
  }
  apply()
  container.classList.add('hide-headers')
  // 切换工作表后骨架才就绪
  const sub = api.addEvent(api.Event.ActiveSheetChanged, () => setTimeout(apply, 0))
  return () => {
    sub.dispose()
    container.classList.remove('hide-headers')
    container.style.removeProperty('--present-hx')
    container.style.removeProperty('--present-hy')
  }
}
