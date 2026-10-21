<script setup lang="ts">
import type { LedgerMe } from '@/api/auth'
import { onMounted, reactive, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getLedgerMeApi, loginApi, ssoLoginUrl } from '@/api/auth'
import { LEDGER_SAME_ORIGIN } from '@/api/ledger-base'
import LanguageSwitch from '@/components/common/LanguageSwitch.vue'
import { t } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'
import { setToken } from '@/utils/auth'
import { runtimeConfig } from '@/utils/runtime-config'

/**
 * 登录页：按服务端的登录方式（LEDGER_AUTH_MODE）显示
 * - local：用户名 + 密码（内置账号）
 * - gateway：跳转运行时配置里的统一登录地址（loginUrl），没有配置时提示
 * - off / proxy：不需要在这里登录，直接进入
 */
defineOptions({ name: 'LoginPage' })

const route = useRoute()
const router = useRouter()
const userStore = useLedgerUserStore()
const me = ref<LedgerMe | null>(null)
const failed = ref('')
const form = reactive({ username: '', password: '', code: '' })
/** 密码对了、这个账号启用了两步验证：等用户输入验证码 */
const needCode = ref(false)
const submitting = ref(false)
const error = ref(ssoErrorText(route.query.sso_error))

/** 单点登录失败时服务端带回来的错误代码 → 给用户看的说明（细节只在服务日志里） */
function ssoErrorText(code: unknown) {
  if (typeof code !== 'string' || !code)
    return ''
  switch (code) {
    case 'denied': return t('你取消了授权，或没有被授予访问权限。')
    case 'expired': return t('登录请求已过期或不完整，请重新点击登录。')
    case 'token': return t('身份提供方返回的登录凭证没有通过校验，请联系管理员。')
    case 'disabled': return t('账号已停用，请联系管理员。')
    case 'not_provisioned': return t('你的账号还没有开通，请联系管理员。')
    case 'unreachable': return t('连不上身份提供方，请稍后再试或联系管理员。')
    case 'config': return t('单点登录的配置有误，请联系管理员。')
    case 'busy': return t('正在登录的人太多了，请稍后再试。')
    case 'idp_error': return t('身份提供方返回了错误，请稍后再试。')
    default: return t('单点登录失败，请重试；仍然不行请联系管理员。')
  }
}

/** 单点登录入口：只在页面与服务同源时可用（登录结果靠服务端写的 cookie 带回来） */
function ssoUrl() {
  const sso = me.value?.sso
  if (!sso || !LEDGER_SAME_ORIGIN)
    return ''
  // 服务端登录后用 302 把浏览器送回这个路径，要带上前端路由的 base
  return ssoLoginUrl(sso.loginPath, router.resolve(redirect()).href)
}

function redirect() {
  const r = route.query.redirect
  return typeof r === 'string' && r.startsWith('/') && !r.startsWith('//') ? r : '/excel/home'
}

onMounted(async () => {
  try {
    me.value = await getLedgerMeApi()
  }
  catch (e) {
    failed.value = e instanceof Error ? e.message : String(e)
    return
  }
  if (!me.value.authRequired || me.value.user || me.value.mode === 'proxy') {
    router.replace(redirect())
    return
  }
  const loginUrl = runtimeConfig().loginUrl
  if (me.value.mode === 'gateway' && loginUrl) {
    const target = new URL(loginUrl, window.location.origin)
    target.searchParams.set('redirect', new URL(redirect(), window.location.origin).href)
    window.location.href = target.href
  }
})

async function submit() {
  error.value = ''
  if (!form.username.trim() || !form.password) {
    error.value = t('请输入用户名和密码')
    return
  }
  if (needCode.value && !form.code.trim()) {
    error.value = t('请输入验证码')
    return
  }
  submitting.value = true
  try {
    const r = await loginApi(form.username.trim(), form.password, needCode.value ? form.code.trim() : undefined)
    if (r.needCode) {
      needCode.value = true
      return
    }
    // 同源部署：服务端已写入 HttpOnly 会话 cookie，页面不保存令牌（脚本读不到，XSS 偷不走）；
    // 跨域部署：浏览器不会向另一个来源带 cookie，只能把令牌放在 cookie / 请求头里自己带
    if (!LEDGER_SAME_ORIGIN)
      setToken(r.token, r.days)
    userStore.reset()
    router.replace(redirect())
  }
  catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  }
  finally {
    submitting.value = false
  }
}
</script>

<template>
  <div class="relative h-full flex items-center justify-center bg-[#F4F6F5]">
    <div class="absolute right-5 top-4">
      <LanguageSwitch />
    </div>
    <div class="w-[360px] rounded-xl bg-white p-8 shadow-[0_8px_30px_rgba(0,0,0,0.06)]">
      <div class="mb-6 flex items-center gap-2">
        <span class="h-7 w-7 flex items-center justify-center rounded-md bg-[#107C41] text-sm text-white font-bold">W</span>
        <span class="text-lg text-gray-900 font-semibold">Gridlake</span>
      </div>
      <div v-if="failed" class="text-[13px] text-[#B91C1C] leading-6">
        {{ $t('连不上表格服务：{0}', [failed]) }}
      </div>
      <template v-else-if="me?.mode === 'local'">
        <el-form label-position="top" @submit.prevent="submit">
          <template v-if="needCode">
            <div class="mb-3 text-[13px] text-gray-600 leading-6">
              {{ $t('这个账号启用了两步验证：请输入验证器 App 里现在显示的 6 位数字。手机不在身边可以输入一次性恢复码。') }}
            </div>
            <el-form-item :label="$t('验证码')">
              <el-input v-model="form.code" autocomplete="one-time-code" data-testid="login-code" @keyup.enter="submit" />
            </el-form-item>
          </template>
          <el-form-item v-else :label="$t('用户名')">
            <el-input v-model="form.username" autocomplete="username" data-testid="login-username" />
          </el-form-item>
          <el-form-item v-if="!needCode" :label="$t('密码')">
            <el-input v-model="form.password" type="password" show-password autocomplete="current-password" data-testid="login-password" @keyup.enter="submit" />
          </el-form-item>
          <div v-if="error" class="mb-3 text-[13px] text-[#B91C1C]" data-testid="login-error">
            {{ error }}
          </div>
          <el-button type="primary" class="w-full" :loading="submitting" native-type="submit" data-testid="login-submit">
            {{ $t('登录') }}
          </el-button>
        </el-form>
        <template v-if="ssoUrl()">
          <div class="my-4 flex items-center gap-3 text-xs text-gray-400">
            <span class="h-px flex-1 bg-gray-200" />
            {{ $t('或') }}
            <span class="h-px flex-1 bg-gray-200" />
          </div>
          <a :href="ssoUrl()" class="block" data-testid="login-sso">
            <el-button class="w-full">{{ $t('用 {0} 登录', [me?.sso?.name ?? '']) }}</el-button>
          </a>
        </template>
        <div class="mt-4 text-xs text-gray-400 leading-5">
          {{ $t('账号由管理员在首页「自动化 → 账号管理」里创建；忘记密码请联系管理员重置。') }}
        </div>
      </template>
      <div v-else-if="me?.mode === 'gateway'" class="text-[13px] text-gray-600 leading-6">
        {{ $t('请从统一登录入口登录后再打开本页面。（管理员可在运行时配置') }} <code>config/index.js</code> {{ $t('的') }} <code>loginUrl</code> {{ $t('里填写登录地址，未登录时会自动跳转。）') }}
      </div>
      <div v-else class="text-[13px] text-gray-500">
        {{ $t('正在进入…') }}
      </div>
    </div>
  </div>
</template>
