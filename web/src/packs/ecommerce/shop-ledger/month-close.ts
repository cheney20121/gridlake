import type { LedgerCloseAction, LedgerCloseStatus, LedgerWithdrawStatus } from '@/packs/ecommerce/api'
import { t } from '@/i18n'

export const CLOSE_STATUS: Record<LedgerCloseStatus, { label: string, type: 'info' | 'warning' | 'success' | 'danger' }> = {
  open: { label: t('未结账'), type: 'info' },
  rejected: { label: t('已驳回'), type: 'danger' },
  pending: { label: t('审批中'), type: 'warning' },
  locked: { label: t('已锁账'), type: 'success' },
}

export const CLOSE_ACTION_LABEL: Record<LedgerCloseAction, string> = {
  submit: '提交月结',
  approve: '审批通过',
  reject: '驳回',
  cancel: '撤回',
  reopen: '反结账',
}

export const CHECK_LEVEL = {
  pass: { icon: '✓', cls: 'lv-pass' },
  warn: { icon: '!', cls: 'lv-warn' },
  fail: { icon: '✕', cls: 'lv-fail' },
} as const

export const WITHDRAW_STATUS: Record<LedgerWithdrawStatus, { label: string, type: 'success' | 'warning' | 'info' | 'danger' }> = {
  matched: { label: t('已到账'), type: 'success' },
  in_transit: { label: t('在途'), type: 'warning' },
  no_bank: { label: t('缺银行流水'), type: 'info' },
  overdue: { label: t('逾期未到账'), type: 'danger' },
}

/** 冻结（审批中 / 已锁账）的店铺月份不能再改余额、配对、导入 */
export function isFrozen(status: LedgerCloseStatus | undefined) {
  return status === 'pending' || status === 'locked'
}
