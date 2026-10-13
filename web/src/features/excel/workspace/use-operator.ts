import { computed, ref, watch } from 'vue'
import { useLedgerUserStore } from '@/stores/ledger-user'

const OPERATOR_KEY = 'ledger-operator'

/**
 * 操作人：写入余额修改人、配对人、月结日志、版本保存人等。
 * 表格服务开启登录校验后固定为登录用户（服务端也会覆盖请求里的名字，不能改）；
 * 没有开启时手动填写（保存在本地）
 */
export function useLedgerOperator() {
  const ledgerUser = useLedgerUserStore()
  const manual = ref(localStorage.getItem(OPERATOR_KEY) || '')
  watch(manual, v => localStorage.setItem(OPERATOR_KEY, v.trim()))
  return computed({
    get: () => (ledgerUser.authRequired && ledgerUser.name ? ledgerUser.name : manual.value),
    set: (v: string) => {
      if (!ledgerUser.authRequired)
        manual.value = v
    },
  })
}
