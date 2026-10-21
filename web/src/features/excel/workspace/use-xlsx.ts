import type { FUniver } from '@univerjs/core/facade'
import type { ShallowRef } from 'vue'
import { ref } from 'vue'
import { datasetFormulasBusy } from '@/features/excel/formula/dataset-functions'
import { t } from '@/i18n'
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
      const base = uiStore.fileName.replace(/\.[^.]+$/, '') || t('工作簿')
      const name = currentPeriod.value ? `${base}_${currentPeriod.value}.xlsx` : `${base}.xlsx`
      const r = await exportWorkbookXlsx(api, name)
      ElMessage.success({
        message: [
          t('已导出 {0} 个页签、{1} 个公式', [r.sheets, r.formulas]),
          r.charts ? t('、{0} 个图表（Excel 中为图片）', [r.charts]) : '',
          r.dataTables ? t('、{0} 个数据表（单独页签，Excel 表格）', [r.dataTables]) : '',
          r.cellTables ? t('、{0} 个单元格模式的数据表（原位置包成 Excel 表格）', [r.cellTables]) : '',
          r.tableFormulas ? t('；{0} 个数据表公式已转为 Excel 写法（如 SUM(表[列])），在 Excel 里照样能算', [r.tableFormulas]) : '',
          r.tableFormulasAsValue ? t('；{0} 个数据表公式导出为数值', [r.tableFormulasAsValue]) : '',
          r.dataTablesTruncated.length ? t('；数据表「{0}」行数较多，只导出了前 {1} 行', [r.dataTablesTruncated.join('」「'), DATA_TABLE_EXPORT_ROWS.toLocaleString()]) : '',
          r.arrayFormulas ? t('；{0} 个动态数组公式（FILTER、UNIQUE 等）按溢出范围写成数组公式', [r.arrayFormulas]) : '',
          r.definedNames.exported ? t('；命名区域 {0} 个', [r.definedNames.exported]) : '',
          r.outlineGroups ? t('；行列分组 {0} 个', [r.outlineGroups]) : '',
          r.platformFormulas ? t('；{0} 个平台函数公式（DSUMIFS 等，Excel 无法计算）已转为数值，原公式见单元格批注', [r.platformFormulas]) : '',
          r.chartsFailed.length ? t('；图表「{0}」未能导出', [r.chartsFailed.join('」「')]) : '',
          exportFeatureText(r.features),
        ].join(''),
        duration: 6000,
      })
    }
    catch (e) {
      ElMessage.error(t('导出失败：{0}', [e instanceof Error ? e.message : String(e)]))
    }
    finally {
      xlsxBusy.value = false
    }
  }

  /** 条件格式 / 数据验证 / 批注 / 超链接 / 表格的个数说明（导出和导入共用） */
  function featureParts(f: { conditionalFormats: number, dataValidations: number, notes: number, hyperlinks: number, tables: number }) {
    return [
      f.conditionalFormats && t('条件格式 {0} 条', [f.conditionalFormats]),
      f.dataValidations && t('数据验证 {0} 条', [f.dataValidations]),
      f.notes && t('批注 {0} 个', [f.notes]),
      f.hyperlinks && t('超链接 {0} 个', [f.hyperlinks]),
      f.tables && t('表格 {0} 个', [f.tables]),
    ].filter(Boolean)
  }

  /** 导出结果里的条件格式 / 数据验证 / 批注 / 超链接说明 */
  function exportFeatureText(f: { conditionalFormats: number, dataValidations: number, notes: number, hyperlinks: number, tables: number, skipped: string[] }) {
    const parts = featureParts(f)
    return `${parts.length ? t('；含{0}', [parts.join('、')]) : ''}${f.skipped.length ? t('；{0} 项规则 Excel 无对应格式未导出（{1}）', [f.skipped.length, `${f.skipped.slice(0, 3).join('、')}${f.skipped.length > 3 ? '…' : ''}`]) : ''}`
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
      const lines = r.sheets.map(s => t('「{0}」{1} 行 × {2} 列，公式 {3} 个，合并 {4} 处{5}', [escapeHtml(s.name), s.rows.toLocaleString(), s.cols, s.formulas.toLocaleString(), s.merges, s.outlineGroups ? t('，分组 {0} 个', [s.outlineGroups]) : '']))
      for (const s of r.skipped)
        lines.push(t('<span style="color:#92400E">「{0}」{1} 行，超过表格页签上限，未导入。请用「+ 导入数据」导入为数据集（支持千万行）</span>', [escapeHtml(s.name), s.rows.toLocaleString()]))
      const f = r.features
      const brought = [...featureParts(f), r.definedNames?.imported && t('命名区域 {0} 个', [r.definedNames.imported])].filter(Boolean)
      if (brought.length)
        lines.push(t('<span style="color:#0B5A2E">同时导入：{0}</span>', [brought.join('、')]))
      const dropped = [r.unsupported.images && t('图片 {0} 张', [r.unsupported.images]), ...f.skipped.slice(0, 5).map(escapeHtml), r.definedNames?.skipped.length && t('命名区域「{0}」{1}（指向多个区域、未导入的页签或重名）', [r.definedNames.skipped.slice(0, 3).map(escapeHtml).join('」「'), r.definedNames.skipped.length > 3 ? t(' 等 {0} 个', [r.definedNames.skipped.length]) : ''])].filter(Boolean)
      if (dropped.length)
        lines.push(t('<span style="color:#6B7280">未能导入：{0}{1}</span>', [dropped.join('、'), f.skipped.length > 5 ? t(' 等 {0} 项', [f.skipped.length]) : '']))
      if (r.unknownFunctions)
        lines.push(t('<span style="color:#B91C1C">{0} 个公式使用了当前不支持的函数（显示 #NAME?）</span>', [r.unknownFunctions]))
      await ElMessageBox.alert(lines.join('<br>') || t('文件中没有可导入的页签'), t('已导入「{0}」', [escapeHtml(file.name)]), {
        dangerouslyUseHTMLString: true,
        confirmButtonText: t('知道了'),
      }).catch(() => {})
    }
    catch (err) {
      ElMessage.error(t('导入失败：{0}', [err instanceof Error ? err.message : String(err)]))
    }
    finally {
      xlsxBusy.value = false
    }
  }

  return { xlsxInputRef, xlsxBusy, onXlsxCommand, onXlsxFile }
}
