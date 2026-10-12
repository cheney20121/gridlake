import type { Univer } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'

import { UniverSheetsConditionalFormattingPreset } from '@univerjs/preset-sheets-conditional-formatting'
import { UniverSheetsCorePreset } from '@univerjs/preset-sheets-core'
import { UniverSheetsDataValidationPreset } from '@univerjs/preset-sheets-data-validation'
import { UniverSheetsDrawingPreset } from '@univerjs/preset-sheets-drawing'
import { UniverSheetsFilterPreset } from '@univerjs/preset-sheets-filter'
import { UniverSheetsFindReplacePreset } from '@univerjs/preset-sheets-find-replace'
import { UniverSheetsHyperLinkPreset } from '@univerjs/preset-sheets-hyper-link'
import { UniverSheetsNotePreset } from '@univerjs/preset-sheets-note'
import { UniverSheetsSortPreset } from '@univerjs/preset-sheets-sort'
import { UniverSheetsTablePreset } from '@univerjs/preset-sheets-table'
import { createUniver, greenTheme, LocaleType, mergeLocales } from '@univerjs/presets'
import { lang } from '@/i18n'
import { univerLocalePacks } from '@/i18n/univer-locale'

import '@univerjs/preset-sheets-core/lib/index.css'
import '@univerjs/preset-sheets-drawing/lib/index.css'
import '@univerjs/preset-sheets-conditional-formatting/lib/index.css'
import '@univerjs/preset-sheets-data-validation/lib/index.css'
import '@univerjs/preset-sheets-filter/lib/index.css'
import '@univerjs/preset-sheets-sort/lib/index.css'
import '@univerjs/preset-sheets-find-replace/lib/index.css'
import '@univerjs/preset-sheets-hyper-link/lib/index.css'
import '@univerjs/preset-sheets-note/lib/index.css'
import '@univerjs/preset-sheets-table/lib/index.css'

export interface UniverContext {
  univer: Univer
  univerAPI: FUniver
}

let univerContext: UniverContext | null = null

/**
 * 初始化 Univer 实例并挂载到指定 DOM 容器
 * 使用 preset-sheets-core 预设，自带完整 Excel UI（工具栏、公式栏、编辑器）
 * 自定义配置：保留核心编辑能力 + 上下文菜单；隐藏自带工具栏/页眉/页脚/公式栏（由我们自己的 UI 提供）
 */
export function initUniver(container: string | HTMLElement): UniverContext {
  if (univerContext)
    disposeUniver()

  const { univer, univerAPI } = createUniver({
    locale: lang === 'en-US' ? LocaleType.EN_US : LocaleType.ZH_CN,
    // Excel 品牌绿：选区框、激活标签、工具栏高亮统一为绿色
    theme: greenTheme,
    // 当前语言的界面语言包在应用挂载前已加载好（见 main.ts）
    locales: {
      [lang === 'en-US' ? LocaleType.EN_US : LocaleType.ZH_CN]: mergeLocales(...univerLocalePacks() as Parameters<typeof mergeLocales>),
    },
    presets: [
      UniverSheetsCorePreset({
        container,
        // 启用 Univer 全套 UI（像 Excel 一样）
        header: true,
        toolbar: true,
        footer: {},
        contextMenu: true,
        formulaBar: true,
        // 中文字体优先级
        customFontFamily: {
          override: false,
          list: [
            { label: '微软雅黑', value: 'Microsoft YaHei' },
            { label: '苹方', value: 'PingFang SC' },
            { label: '等线', value: 'DengXian' },
          ],
        },
      }),
      // 浮动对象（图表、图片）：随工作表滚动、可拖动缩放，随工作簿快照保存
      UniverSheetsDrawingPreset(),
      // Excel 基础能力（均为 Univer 开源插件）：条件格式、数据验证 / 下拉、筛选、排序、查找替换、超链接、批注、表格
      UniverSheetsConditionalFormattingPreset(),
      UniverSheetsDataValidationPreset(),
      UniverSheetsFilterPreset(),
      UniverSheetsSortPreset(),
      UniverSheetsFindReplacePreset(),
      UniverSheetsHyperLinkPreset(),
      UniverSheetsNotePreset(),
      // 表格（Excel 的「超级表」）：套用样式、自带筛选、新增行自动扩展、结构化引用
      UniverSheetsTablePreset(),
    ],
  })

  univerContext = { univer, univerAPI }
  return univerContext
}

/**
 * 销毁 Univer 实例，释放资源
 */
export function disposeUniver(): void {
  if (univerContext) {
    try {
      univerContext.univer.dispose()
    }
    catch {
      // ignore disposal errors
    }
    univerContext = null
  }
}

/**
 * 获取当前 Univer 上下文
 */
export function getUniver(): UniverContext | null {
  return univerContext
}
