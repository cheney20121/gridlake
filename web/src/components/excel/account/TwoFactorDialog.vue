<script setup lang="ts">
import type { TwoFactorSetup, TwoFactorStatus } from '@/api/auth'
import { ref, watch } from 'vue'
import { disableTwoFactorApi, enableTwoFactorApi, getTwoFactorApi, setupTwoFactorApi } from '@/api/auth'
import { t } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'

/** 管理员要求必须开启：设置完成前不能关闭这个对话框 */
const props = defineProps<{ required?: boolean }>()
/**
 * 两步验证（TOTP）：登录时除了密码，还要输入验证器 App（Google Authenticator、Microsoft Authenticator、1Password 等）里的 6 位动态码。
 * 开启：生成密钥 → 在验证器里添加（点开地址或手工输入密钥）→ 输入现在显示的 6 位数字确认 → 保存一次性恢复码（手机丢了用）。
 */
const visible = defineModel<boolean>('visible', { default: false })
const ledgerUser = useLedgerUserStore()
const status = ref<TwoFactorStatus | null>(null)
const setup = ref<TwoFactorSetup | null>(null)
const recovery = ref<string[]>([])
const code = ref('')
const password = ref('')
const busy = ref(false)
const error = ref('')

async function load() {
  status.value = await getTwoFactorApi()
}

watch(visible, (v) => {
  if (v) {
    setup.value = null
    recovery.value = []
    code.value = password.value = error.value = ''
    load()
  }
}, { immediate: true })

async function run<T>(fn: () => Promise<T>): Promise<T | undefined> {
  error.value = ''
  busy.value = true
  try {
    return await fn()
  }
  catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  }
  finally {
    busy.value = false
  }
}

async function start() {
  const s = await run(setupTwoFactorApi)
  if (s)
    setup.value = s
}

async function confirm() {
  const r = await run(() => enableTwoFactorApi(code.value.trim()))
  if (r) {
    recovery.value = r.recoveryCodes
    setup.value = null
    code.value = ''
    await load()
  }
}

/** 必须开启的场景：恢复码保存好了，刷新页面让之前被拦住的数据重新加载 */
async function finishRequired() {
  ledgerUser.reset()
  await ledgerUser.ensure()
  visible.value = false
  window.location.reload()
}

async function disable() {
  const ok = await ElMessageBox.confirm(t('停用后登录只需要密码，账号安全性会降低。确定停用两步验证？'), t('停用两步验证'), { type: 'warning', confirmButtonText: t('停用'), cancelButtonText: t('取消') }).then(() => true, () => false)
  if (!ok)
    return
  const r = await run(() => disableTwoFactorApi(password.value, code.value.trim()))
  if (r) {
    ElMessage.success(t('已停用两步验证'))
    code.value = password.value = ''
    await load()
  }
}

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    ElMessage.success(t('已复制'))
  }
  catch {
    ElMessage.warning(t('浏览器不允许自动复制，请手动选中复制'))
  }
}
</script>

<template>
  <el-dialog v-model="visible" :title="$t('两步验证')" width="520px" append-to-body :show-close="!props.required" :close-on-click-modal="!props.required" :close-on-press-escape="!props.required" data-testid="twofactor-dialog">
    <div v-if="props.required && !recovery.length" class="mb-3 rounded bg-[#FEF3C7] px-3 py-2 text-[13px] text-[#92400E] leading-6" data-testid="twofactor-required">
      {{ $t('管理员要求所有账号开启两步验证：设置完成前其他功能暂时不能使用。') }}
    </div>
    <div v-if="status" class="text-[13px] leading-6">
      <!-- 刚启用：展示恢复码（只此一次） -->
      <div v-if="recovery.length" class="mb-3 border border-[#a7d7b8] rounded-lg bg-[#f1faf4] p-3" data-testid="twofactor-recovery">
        <div class="mb-1 font-semibold">
          {{ $t('两步验证已启用。请保存这些一次性恢复码——手机丢了时可以代替验证码登录，每个只能用一次，关闭后无法再次查看') }}
        </div>
        <div class="grid grid-cols-2 gap-1 text-xs font-mono">
          <code v-for="c in recovery" :key="c" data-testid="twofactor-recovery-code">{{ c }}</code>
        </div>
        <el-button class="mt-2" size="small" @click="copy(recovery.join('\n'))">
          {{ $t('复制全部') }}
        </el-button>
      </div>

      <template v-if="status.enabled">
        <div class="mb-3">
          <el-tag type="success" size="small">
            {{ $t('已启用') }}
          </el-tag>
          <span class="ml-2 text-gray-500">{{ $t('还剩 {0} 个恢复码', [status.recoveryLeft]) }}</span>
        </div>
        <div class="mb-2 text-gray-500">
          {{ $t('停用需要再输入一次密码和一个验证码（或恢复码）。') }}
        </div>
        <div class="flex flex-col gap-2">
          <el-input v-model="password" type="password" show-password :placeholder="$t('当前密码')" autocomplete="current-password" data-testid="twofactor-disable-password" />
          <el-input v-model="code" :placeholder="$t('验证码或恢复码')" autocomplete="one-time-code" data-testid="twofactor-disable-code" />
          <div>
            <el-button type="danger" :loading="busy" :disabled="!password || !code.trim()" data-testid="twofactor-disable" @click="disable">
              {{ $t('停用两步验证') }}
            </el-button>
          </div>
        </div>
      </template>

      <template v-else-if="setup">
        <ol class="mb-3 list-decimal pl-5 text-gray-600">
          <li>{{ $t('在验证器 App 里添加账号：手机上可以直接点开下面的地址，或在 App 里选「手动输入密钥」。') }}</li>
          <li>{{ $t('输入 App 里现在显示的 6 位数字，确认设置。') }}</li>
        </ol>
        <div class="mb-2">
          <div class="text-gray-500">
            {{ $t('密钥') }}
          </div>
          <code class="break-all rounded bg-[#F7F8FA] px-2 py-1 text-sm" data-testid="twofactor-secret">{{ setup.secret }}</code>
          <el-button class="ml-2" size="small" @click="copy(setup.secret)">
            {{ $t('复制') }}
          </el-button>
        </div>
        <div class="mb-3">
          <a :href="setup.uri" class="break-all text-xs text-[#107C41]">{{ setup.uri }}</a>
        </div>
        <div class="flex items-center gap-2">
          <el-input v-model="code" class="!w-44" maxlength="7" :placeholder="$t('6 位验证码')" autocomplete="one-time-code" data-testid="twofactor-code" @keyup.enter="confirm" />
          <el-button type="primary" :loading="busy" :disabled="!code.trim()" data-testid="twofactor-enable" @click="confirm">
            {{ $t('确认并启用') }}
          </el-button>
        </div>
      </template>

      <template v-else-if="!recovery.length">
        <div class="mb-3 text-gray-600">
          {{ $t('开启后，登录时除了密码还要输入验证器 App 里的 6 位动态码，密码泄露也不会被直接登录。') }}
        </div>
        <el-button type="primary" :loading="busy" data-testid="twofactor-start" @click="start">
          {{ $t('开始设置') }}
        </el-button>
      </template>

      <div v-if="error" class="mt-3 text-[#B91C1C]" data-testid="twofactor-error">
        {{ error }}
      </div>
    </div>
    <template #footer>
      <el-button v-if="props.required && recovery.length" type="primary" data-testid="twofactor-continue" @click="finishRequired">
        {{ $t('我已保存恢复码，继续') }}
      </el-button>
      <el-button v-else-if="!props.required" @click="visible = false">
        {{ $t('关闭') }}
      </el-button>
    </template>
  </el-dialog>
</template>
