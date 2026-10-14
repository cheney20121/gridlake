import type { CellValue } from '@/api/dataset'

/**
 * 固定列：填报区里不显示在表格上、提交时自动带上的列。
 * 收集任务生成的填报工作簿用它放「填报单位」和「期次」——负责人只填业务列，单位和期次不会填错也不用重复填。
 * 键是数据集的列名，值是固定的取值
 */
export type FixedColumns = Record<string, string>

export function fixedEntries(fixed?: FixedColumns | null): [string, string][] {
  return Object.entries(fixed ?? {})
}

/** 提交给服务端的列：固定列在前，后面是表格里的列 */
export function allFields(fields: string[], fixed?: FixedColumns | null): string[] {
  return [...fixedEntries(fixed).map(([k]) => k), ...fields]
}

/** 每一行前面补上固定列的值（与 allFields 的顺序一致） */
export function withFixed(rows: CellValue[][], fixed?: FixedColumns | null): CellValue[][] {
  const values = fixedEntries(fixed).map(([, v]) => v)
  return values.length ? rows.map(r => [...values, ...r]) : rows
}

/** 固定列的说明，如「填报单位 = 店铺A，期次 = 2026-10」 */
export function fixedText(fixed?: FixedColumns | null): string {
  return fixedEntries(fixed).map(([k, v]) => `${k} = ${v}`).join('，')
}
