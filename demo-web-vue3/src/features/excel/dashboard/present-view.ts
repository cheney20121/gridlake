import type { FUniver } from '@univerjs/core/facade'

/**
 * 演示模式隐藏网格线。网格线开关是工作表数据（会被保存），所以只在演示期间临时关掉，
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
