import { toBlob } from 'html-to-image'
import { t } from '@/i18n'

/**
 * 看板部件（指标卡、切片器、表格卡片）保存为图片：把部件的界面画成 PNG（2 倍清晰度），不含编辑按钮；
 * 图表用 ECharts 自己的导出（见 ChartView），这里不处理。
 * 只保存看得到的部分：表格卡片里滚动到可视区域之外的行不会出现在图里
 */

/** 文件名里不能出现的字符换成下划线 */
export function safeFileName(name: string, fallback = '看板部件') {
  const n = name.replace(/[\\/:*?"<>|\s]+/g, '_').replace(/^_+|_+$/g, '')
  return n || fallback
}

export function downloadBlob(blob: Blob, fileName: string) {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = fileName
  document.body.append(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 10_000)
}

/** 点了部件上的「保存为图片」：找到部件根元素（selector）画成图片下载。返回文件名 */
export async function saveWidgetImage(trigger: Event, selector: string, title: string): Promise<string> {
  const root = (trigger.currentTarget as HTMLElement | null)?.closest<HTMLElement>(selector)
  if (!root)
    throw new Error(t('找不到要保存的部件'))
  const blob = await toBlob(root, {
    pixelRatio: 2,
    backgroundColor: '#ffffff',
    // 不画编辑按钮、清除按钮等悬停时才出现的操作
    filter: node => !(node instanceof HTMLElement && node.classList.contains('edit')),
  })
  if (!blob)
    throw new Error(t('生成图片失败'))
  const fileName = `${safeFileName(title)}.png`
  downloadBlob(blob, fileName)
  return fileName
}
