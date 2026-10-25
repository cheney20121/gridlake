import { reactive } from 'vue'

/**
 * 数据集的数据在别处被改了（AI 助手应用 / 撤销了数据清洗）：打开着的数据表据此重新取数。
 * 行数可能不变（如查找替换），所以不能靠数据集列表里的行数判断
 */
const versions = reactive<Record<string, number>>({})

export function dataVersion(datasetId: string): number {
  return versions[datasetId] ?? 0
}

export function markDataChanged(datasetId: string) {
  versions[datasetId] = (versions[datasetId] ?? 0) + 1
}
