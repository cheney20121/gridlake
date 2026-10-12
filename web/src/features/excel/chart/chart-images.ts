import type { FUniver } from '@univerjs/core/facade'
import type { ChartSpec } from './types'
import { slicerFiltersFor } from '@/features/excel/dashboard/slicer'
import { resolveChartData } from './chart-data'
import { listCharts, sheetIdOf } from './chart-manager'
import { buildOption } from './chart-option'
import { echarts } from './echarts'

/**
 * 图表转图片（导出 xlsx 时嵌入）：页面上已渲染的图表直接取图；其他工作表上的图表离屏渲染
 */

const instances = new Map<string, echarts.ECharts>()

export function registerChartInstance(id: string, chart: echarts.ECharts) {
  instances.set(id, chart)
}

/** 页面上已渲染的图表实例（端到端测试里用来换算点击位置） */
export function chartInstance(id: string) {
  return instances.get(id)
}

export function unregisterChartInstance(id: string, chart: echarts.ECharts | null) {
  if (instances.get(id) === chart)
    instances.delete(id)
}

async function renderOffscreen(api: FUniver, spec: ChartSpec, width: number, height: number) {
  const div = document.createElement('div')
  const chart = echarts.init(div, null, { renderer: 'canvas', width, height })
  try {
    const extra = spec.source.kind === 'dataset' ? slicerFiltersFor(api, sheetIdOf(api, spec.id), spec.source.datasetId) : []
    const data = await resolveChartData(api, spec, undefined, extra)
    chart.setOption({ ...buildOption(spec, data).option, animation: false })
    return chart.getDataURL({ type: 'png', pixelRatio: 2, backgroundColor: '#fff' })
  }
  finally {
    chart.dispose()
  }
}

export interface ChartImage {
  sheetId: string
  spec: ChartSpec
  /** PNG base64（不含 data: 前缀） */
  base64: string
  position: { left: number, top: number, width: number, height: number }
}

/** 工作簿内所有图表的图片；单个图表失败（如数据集被删）时跳过并记录 */
export async function chartImages(api: FUniver): Promise<{ images: ChartImage[], failed: string[] }> {
  const images: ChartImage[] = []
  const failed: string[] = []
  for (const c of listCharts(api)) {
    const w = Math.max(200, Math.round(c.position.width))
    const h = Math.max(120, Math.round(c.position.height))
    try {
      const live = instances.get(c.spec.id)
      const url = live && !live.isDisposed()
        ? live.getDataURL({ type: 'png', pixelRatio: 2, backgroundColor: '#fff' })
        : await renderOffscreen(api, c.spec, w, h)
      images.push({ sheetId: c.sheet.getSheetId(), spec: c.spec, base64: url.replace(/^data:image\/png;base64,/, ''), position: { ...c.position, width: w, height: h } })
    }
    catch (e) {
      console.warn('[chart] 导出图片失败', c.spec.title, e)
      failed.push(c.spec.title || '未命名图表')
    }
  }
  return { images, failed }
}
