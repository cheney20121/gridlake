import type { DatasetFilter, DatasetSort } from '@/api/dataset'
import { ref, shallowRef } from 'vue'

/**
 * 数据集页面正在看的数据集与它当前的筛选 / 搜索 / 排序：AI 助手提问时作为界面上下文带上，
 * 用户说「这些订单」「这张表」时助手知道指的是什么。列都换成列名（筛选里存的是列键）
 */
export interface DatasetViewContext {
  id: string
  name: string
  rows: number
  /** 按当前筛选 / 搜索命中的行数（游标建好之前没有） */
  matched?: number
  filters: DatasetFilter[]
  sort: DatasetSort[]
  keyword?: string
  /** 从透视表 / 公式下钻过来时的说明 */
  drill?: string
  /** 只看不符合这条校验规则的行（规则的说明） */
  violation?: string
}

const view = shallowRef<DatasetViewContext | null>(null)
/** 数据集页面是否在前台（切回工作簿时数据表组件还在，但不该再当作上下文） */
const shown = ref(false)

export function publishDatasetView(v: DatasetViewContext) {
  view.value = v
}

/** 数据表组件卸载：只清掉自己发布的 */
export function clearDatasetView(id: string) {
  if (view.value?.id === id)
    view.value = null
}

export function setDatasetViewShown(v: boolean) {
  shown.value = v
}

export function currentDatasetView(): DatasetViewContext | null {
  return shown.value ? view.value : null
}
