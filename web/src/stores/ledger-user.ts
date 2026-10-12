import type { LedgerMe, LedgerScope, LedgerUser } from '@/api/auth'
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { getLedgerMeApi } from '@/api/auth'
import { removeToken } from '@/utils/auth'

/** 工作簿的共享范围：private 仅自己 / view 所有人可查看 / edit 所有人可编辑 */
export type Visibility = 'private' | 'view' | 'edit'

export const VISIBILITY_LABEL: Record<Visibility, string> = {
  private: '仅自己',
  view: '所有人可查看',
  edit: '所有人可编辑',
}

/** 对一个对象的权限等级：0 无权 1 查看 2 编辑 3 管理（与服务端 auth::level 一致） */
export type AccessLevel = 0 | 1 | 2 | 3

/**
 * 表格服务的当前用户。服务端开启登录校验后，操作人以这里的用户为准（服务端会覆盖请求里传的名字）。
 * 没有开启时（authRequired 为 false）不区分用户，所有人都视为管理员，界面与以前一致
 */
export const useLedgerUserStore = defineStore('ledger-user', () => {
  const loaded = ref(false)
  const authRequired = ref(false)
  /** 登录方式：off 不登录 / local 内置账号 / gateway 网关 / proxy 反向代理 */
  const mode = ref<LedgerMe['mode']>('off')
  const user = ref<LedgerUser | null>(null)
  const scope = ref<LedgerScope>({ restricted: false, shops: [] })

  const isAdmin = computed(() => !authRequired.value || !!user.value?.admin)
  const name = computed(() => (authRequired.value ? user.value?.name || '' : ''))
  /** 数据范围受限：导入、建视图、定时推送等会绕过过滤的功能不可用，只能看自己范围内店铺的数据 */
  const restricted = computed(() => scope.value.restricted)
  const scopeText = computed(() => (scope.value.restricted ? (scope.value.shops.length ? scope.value.shops.join('、') : '没有任何店铺') : ''))

  /**
   * 确认登录状态；返回 false 表示需要登录（令牌缺失或已失效，已清掉本地令牌）。
   * 查询失败（如身份服务暂不可用）时不拦截，由各页面自己的请求报错
   */
  async function ensure(): Promise<boolean> {
    if (loaded.value)
      return true
    try {
      const me = await getLedgerMeApi()
      authRequired.value = me.authRequired
      mode.value = me.mode
      user.value = me.user
      scope.value = me.scope ?? { restricted: false, shops: [] }
      if (me.authRequired && !me.user) {
        removeToken()
        return false
      }
      loaded.value = true
    }
    catch {}
    return true
  }

  function reset() {
    loaded.value = false
    user.value = null
    scope.value = { restricted: false, shops: [] }
  }

  function accessLevel(w: { ownerId: string | null, visibility: Visibility }): AccessLevel {
    if (!authRequired.value || user.value?.admin || (w.ownerId && w.ownerId === user.value?.id))
      return 3
    return w.visibility === 'edit' ? 2 : w.visibility === 'view' ? 1 : 0
  }

  return { loaded, authRequired, mode, user, scope, restricted, scopeText, isAdmin, name, ensure, reset, accessLevel }
})
