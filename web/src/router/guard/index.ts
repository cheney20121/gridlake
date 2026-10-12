import type { Router } from 'vue-router'
import { useLedgerUserStore } from '@/stores/ledger-user'

/**
 * 表格服务开启了登录校验时，进入页面前先确认登录状态（页面创建之前拦截，避免一堆请求同时报「登录失效」）；
 * 没有开启时直接放行
 */
export function setupRouterGuard(router: Router) {
  router.beforeEach(async (to) => {
    if (to.meta.public)
      return
    if (!(await useLedgerUserStore().ensure()))
      return { name: 'Login', query: { redirect: to.fullPath } }
  })
  router.afterEach((to) => {
    document.title = to.meta.title ? `${String(to.meta.title)} · WebExcel` : 'WebExcel'
  })
}
