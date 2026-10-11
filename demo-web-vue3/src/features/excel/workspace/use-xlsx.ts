import type { FUniver } from '@univerjs/core/facade'
import type { ShallowRef } from 'vue'
import { ref } from 'vue'
import { datasetFormulasBusy } from '@/features/excel/formula/dataset-functions'
import { useExcelUIStore } from '@/stores/excel-ui'
import { escapeHtml } from './html'

/**
 * Excel 文件互通：导出 .xlsx（保留公式、样式、图表、条件格式等）与导入 .xlsx（追加为新页签）。
 * exceljs 体积大，按需加载，只在第一次导入 / 导出时才下载。
 */
export function useXlsx(options: {
  univerAPI: ShallowRef<FUniver | null>
  /** 台账当前月份：导出文件名带上月份 */
  currentPeriod: { value: string }
}) {
  const { univerAPI, currentPeriod } = options
  const uiStore = useExcelUIStore()

  const xlsxInputRef = ref<HTMLInputElement>()
  const xlsxBusy = ref(false)

  async function onXlsxCommand(command: 'import' | 'export') {
    if (command === 'import') {
      xlsxInputRef.value?.click()
      return
    }
    const api = univerAPI.value
    if (!api)
      return
    xlsxBusy.value = true
    try {
      // 等平台函数（DSUMIFS 等）算完，导出的缓存结果才是数值
      const deadline = Date.now() + 60_000
      while (datasetFormulasBusy() && Date.now() < deadline)
        await new Promise(r => setTimeout(r, 50))
      const { exportWorkbookXlsx, DATA_TABLE_EXPORT_ROWS } = await import('@/features/excel/xlsx/export-xlsx')
      const base = uiStore.fileName.replace(/\.[^.]+$/, '') || '工作簿'
      const name = currentPeriod.value ? `${base}_${currentPeriod.value}.xlsx` : `${base}.xlsx`
      const r = await exportWorkbookXlsx(api, name)
      ElMessage.success({
        message: `已导出 ${r.sheets} 个页签、${r.formulas} 个公式${r.charts ? `、${r.charts} 个图表（Excel 中为图片）` : ''}${r.dataTables ? `、${r.dataTables} 个数据表（单独页签，Excel 表格）` : ''}${r.cellTables ? `、${r.cellTables} 个单元格模式的数据表（原位置包成 Excel 表格）` : ''}${r.tableFormulas ? `；${r.tableFormulas} 个数据表公式已转为 Excel 写法（如 SUM(表[列])），在 Excel 里照样能算` : ''}${r.tableFormulasAsValue ? `；${r.tableFormulasAsValue} 个数据表公式导出为数值` : ''}${r.dataTablesTruncated.length ? `；数据表「${r.dataTablesTruncated.join('」「')}」行数较多，只导出了前 ${DATA_TABLE_EXPORT_ROWS.toLocaleString()} 行` : ''}${r.arrayFormulas ? `；${r.arrayFormulas} 个动态数组公式（FILTER、UNIQUE 等）按溢出范围写成数组公式` : ''}${r.definedNames.exported ? `；命名区域 ${r.definedNames.exported} 个` : ''}${r.outlineGroups ? `；行列分组 ${r.outlineGroups} 个` : ''}${r.platformFormulas ? `；${r.platformFormulas} 个平台函数公式（DSUMIFS 等，Excel 无法计算）已转为数值，原公式见单元格批注` : ''}${r.chartsFailed.length ? `；图表「${r.chartsFailed.join('」「')}」未能导出` : ''}${exportFeatureText(r.features)}`,
        duration: 6000,
      })
    }
    catch (e) {
      ElMessage.error(`导出失败：${e instanceof Error ? e.message : String(e)}`)
    }
    finally {
      xlsxBusy.value = false
    }
  }

  /** 导出结果里的条件格式 / 数据验证 / 批注 / 超链接说明 */
  function exportFeatureText(f: { conditionalFormats: number, dataValidations: number, notes: number, hyperlinks: number, tables: number, skipped: string[] }) {
    const parts = [f.conditionalFormats && `条件格式 ${f.conditionalFormats} 条`, f.dataValidations && `数据验证 ${f.dataValidations} 条`, f.notes && `批注 ${f.notes} 个`, f.hyperlinks && `超链接 ${f.hyperlinks} 个`, f.tables && `表格 ${f.tables} 个`].filter(Boolean)
    return `${parts.length ? `；含${parts.join('、')}` : ''}${f.skipped.length ? `；${f.skipped.length} 项规则 Excel 无对应格式未导出（${f.skipped.slice(0, 3).join('、')}${f.skipped.length > 3 ? '…' : ''}）` : ''}`
  }

  async function onXlsxFile(e: Event) {
    const input = e.target as HTMLInputElement
    const file = input.files?.[0]
    input.value = ''
    const api = univerAPI.value
    if (!file || !api)
      return
    xlsxBusy.value = true
    try {
      const { importXlsx } = await import('@/features/excel/xlsx/import-xlsx')
      const r = await importXlsx(api, await file.arrayBuffer())
      const lines = r.sheets.map(s => `「${escapeHtml(s.name)}」${s.rows.toLocaleString()} 行 × ${s.cols} 列，公式 ${s.formulas.toLocaleString()} 个，合并 ${s.merges} 处${s.outlineGroups ? `，分组 ${s.outlineGroups} 个` : ''}`)
      for (const s of r.skipped)
        lines.push(`<span style="color:#92400E">「${escapeHtml(s.name)}」${s.rows.toLocaleString()} 行，超过表格页签上限，未导入。请用「+ 导入数据」导入为数据集（支持千万行）</span>`)
      const f = r.features
      const brought = [f.conditionalFormats && `条件格式 ${f.conditionalFormats} 条`, f.dataValidations && `数据验证 ${f.dataValidations} 条`, f.notes && `批注 ${f.notes} 个`, f.hyperlinks && `超链接 ${f.hyperlinks} 个`, f.tables && `表格 ${f.tables} 个`, r.definedNames?.imported && `命名区域 ${r.definedNames.imported} 个`].filter(Boolean)
      if (brought.length)
        lines.push(`<span style="color:#0B5A2E">同时导入：${brought.join('、')}</span>`)
      const dropped = [r.unsupported.images && `图片 ${r.unsupported.images} 张`, ...f.skipped.slice(0, 5).map(escapeHtml), r.definedNames?.skipped.length && `命名区域「${r.definedNames.skipped.slice(0, 3).map(escapeHtml).join('」「')}」${r.definedNames.skipped.length > 3 ? ` 等 ${r.definedNames.skipped.length} 个` : ''}（指向多个区域、未导入的页签或重名）`].filter(Boolean)
      if (dropped.length)
        lines.push(`<span style="color:#6B7280">未能导入：${dropped.join('、')}${f.skipped.length > 5 ? ` 等 ${f.skipped.length} 项` : ''}</span>`)
      if (r.unknownFunctions)
        lines.push(`<span style="color:#B91C1C">${r.unknownFunctions} 个公式使用了当前不支持的函数（显示 #NAME?）</span>`)
      await ElMessageBox.alert(lines.join('<br>') || '文件中没有可导入的页签', `已导入「${escapeHtml(file.name)}」`, {
        dangerouslyUseHTMLString: true,
        confirmButtonText: '知道了',
      }).catch(() => {})
    }
    catch (err) {
      ElMessage.error(`导入失败：${err instanceof Error ? err.message : String(err)}`)
    }
    finally {
      xlsxBusy.value = false
    }
  }

  return { xlsxInputRef, xlsxBusy, onXlsxCommand, onXlsxFile }
}
