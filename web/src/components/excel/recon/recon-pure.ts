import type { ReconAutoResult, ReconMethod } from '@/api/recon'
import { t } from '@/i18n'

/** 对账里用到的纯函数（便于单测） */

/** 金额：千分位 + 两位小数 */
export function formatMoney(v: number): string {
  return v.toLocaleString('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
}

export const METHOD_LABEL: Record<ReconMethod, string> = {
  exact: t('自动'),
  tolerance: t('自动（容差）'),
  sum: t('自动（汇总）'),
  manual: t('手工'),
}

/** 分为单位算，避免 0.1 + 0.2 之类的浮点误差 */
const cents = (v: number) => Math.round(v * 100)

export interface SelectionSummary {
  leftCount: number
  rightCount: number
  leftTotal: number
  rightTotal: number
  /** A 方合计 - B 方合计 */
  diff: number
  /** 两边都选了 */
  ready: boolean
  /** 差额在容差内：可以直接勾对 */
  balanced: boolean
}

export function summarizeSelection(left: number[], right: number[], tolerance: number): SelectionSummary {
  const lc = left.reduce((s, v) => s + cents(v), 0)
  const rc = right.reduce((s, v) => s + cents(v), 0)
  const ready = left.length > 0 && right.length > 0
  return {
    leftCount: left.length,
    rightCount: right.length,
    leftTotal: lc / 100,
    rightTotal: rc / 100,
    diff: (lc - rc) / 100,
    ready,
    balanced: ready && Math.abs(lc - rc) <= cents(tolerance),
  }
}

/** 自动勾对前的确认文案（试算结果） */
export function autoConfirmText(r: ReconAutoResult, labels: { left: string, right: string }): string {
  if (!r.groups) {
    return r.clearedBroken > 0
      ? t('没有可以自动勾对的了；会清理 {0} 个失效的勾对组（对应的行被改过或删了）。', [r.clearedBroken])
      : t('没有可以自动勾对的了。剩下的请手工勾对，或调整容差 / 日期窗口 / 对账键。')
  }
  const parts = [
    r.exact ? t('金额相同 {0} 组', [r.exact.toLocaleString()]) : '',
    r.tolerance ? t('容差内 {0} 组', [r.tolerance.toLocaleString()]) : '',
    r.sum ? t('合计相等的多笔 {0} 组', [r.sum.toLocaleString()]) : '',
  ].filter(Boolean).join('、')
  return [
    t('将自动勾对 {0} 组（{1}），共 {2} 笔。', [r.groups.toLocaleString(), parts, r.matchedRows.toLocaleString()]),
    r.clearedBroken ? t('同时清理 {0} 个失效的勾对组（对应的行被改过或删了）。', [r.clearedBroken]) : '',
    t('勾对后仍未勾对：{0} {1} 笔，{2} {3} 笔。', [labels.left, r.leftOpen.toLocaleString(), labels.right, r.rightOpen.toLocaleString()]),
    ...r.warnings,
  ].filter(Boolean).join('\n')
}
