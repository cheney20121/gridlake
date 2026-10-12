import type { ICellData, IWorkbookData } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'
import type { WorkbookMeta } from '@/api/workbook'
import { createWorkbookApi, getWorkbookContentApi, getWorkbookMetaApi, listWorkbooksApi } from '@/api/workbook'
import { disposeWorkbook } from '@/features/excel/data/selection'
import { setImportRangeContext } from '@/features/excel/formula/import-range'
import { persistenceManager } from './manager'

const LAST_KEY = 'gridlake-last-workbook'

/** 记住当前打开的工作簿：地址栏 ?wb=（可分享 / 刷新保持）+ 本地最近打开 */
export function rememberWorkbook(id: string) {
  localStorage.setItem(LAST_KEY, id)
  const u = new URL(window.location.href)
  if (u.searchParams.get('wb') !== id) {
    u.searchParams.set('wb', id)
    window.history.replaceState(window.history.state, '', u)
  }
}

/** 要打开的工作簿：地址栏指定 → 最近打开 → 最近更新；服务端一个都没有时返回 null */
export async function resolveInitialWorkbook(): Promise<WorkbookMeta | null> {
  const list = await listWorkbooksApi()
  const fromUrl = new URL(window.location.href).searchParams.get('wb')
  const wanted = fromUrl || localStorage.getItem(LAST_KEY)
  const hit = list.find(w => w.id === wanted)
  if (hit)
    return hit
  // 地址栏指定的工作簿不在列表里，但可能有权打开（如被指定为它的审批人）：直接问服务端，无权时服务端会说明原因
  if (fromUrl) {
    const meta = await getWorkbookMetaApi(fromUrl).catch(() => null)
    if (meta)
      return meta
  }
  return list[0] ?? null
}

/** 结果会溢出到多格的函数（溢出部分不存进工作簿，打开后要重算一次才显示） */
const SPILL_FUNCTION = /\b(?:IMPORTRANGE|FILTER|SORT|SORTBY|UNIQUE|SEQUENCE|RANDARRAY|VSTACK|HSTACK|TAKE|DROP|TOCOL|TOROW|CHOOSECOLS|CHOOSEROWS|EXPAND|WRAPROWS|WRAPCOLS|TEXTSPLIT|GROUPBY|MAKEARRAY|MAP|SCAN|BYROW|BYCOL|TRANSPOSE|MUNIT|REGEXEXTRACT)\s*\(/i

function hasSpillFormula(data: IWorkbookData) {
  for (const sheet of Object.values(data.sheets ?? {})) {
    for (const row of Object.values(sheet?.cellData ?? {})) {
      for (const cell of Object.values(row ?? {}) as (ICellData | undefined)[]) {
        if (typeof cell?.f === 'string' && SPILL_FUNCTION.test(cell.f))
          return true
      }
    }
  }
  return false
}

/** 用 JSON 重建 Univer 工作簿（销毁当前工作簿；期间的变更不记为用户修改） */
export async function loadWorkbookJson(api: FUniver, json: string) {
  const data = JSON.parse(json) as IWorkbookData
  await persistenceManager.withoutTracking(async () => {
    const current = api.getActiveWorkbook()
    if (current)
      disposeWorkbook(api, current.getId())
    api.createWorkbook(data)
    // 动态数组（FILTER、UNIQUE 等）的溢出结果不随工作簿保存，打开后不重算只显示第一格
    if (hasSpillFormula(data)) {
      api.getFormula().executeCalculation()
      await api.getFormula().onCalculationResultApplied(10_000).catch(() => {})
    }
  })
  return data
}

/** 打开服务端工作簿；返回本地是否有未同步草稿（基于同一版本，可恢复） */
export async function openServerWorkbook(api: FUniver, meta: WorkbookMeta) {
  const { json, version } = await getWorkbookContentApi(meta.id)
  // 跨工作簿引用不能指向自己；换工作簿时清掉上一个工作簿取回的结果
  setImportRangeContext({ id: meta.id, name: meta.name })
  await loadWorkbookJson(api, json)
  persistenceManager.attach({ ...meta, version: version || meta.version })
  rememberWorkbook(meta.id)
  const draft = await persistenceManager.getDraft(meta.id).catch(() => null)
  // 草稿基于的版本已被别人更新时不再提示（恢复会覆盖对方的修改），直接丢弃
  if (draft && draft.baseVersion !== (version || meta.version)) {
    await persistenceManager.clearDraft(meta.id).catch(() => {})
    return null
  }
  return draft
}

/** 把当前表格内容保存为新的服务端工作簿并切换过去（folderId：放到这个文件夹） */
export async function createServerWorkbook(name: string, operator: string, folderId?: string | null): Promise<WorkbookMeta> {
  const json = persistenceManager.serialize()
  if (!json)
    throw new Error('工作簿尚未就绪')
  const meta = await createWorkbookApi(name, json, operator, folderId)
  persistenceManager.attach(meta)
  rememberWorkbook(meta.id)
  return meta
}

/** 空白工作簿 */
export function blankWorkbookJson(name: string): string {
  const id = `wb-${Date.now().toString(36)}`
  return JSON.stringify({
    id,
    name,
    sheetOrder: ['sheet-1'],
    styles: {},
    sheets: {
      'sheet-1': {
        id: 'sheet-1',
        name: 'Sheet1',
        rowCount: 1000,
        columnCount: 26,
        defaultRowHeight: 24,
        defaultColumnWidth: 88,
        cellData: {},
        showGridlines: 1,
        rightToLeft: 0,
      },
    },
  } satisfies Partial<IWorkbookData>)
}
