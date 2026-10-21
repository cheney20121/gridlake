<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { logoutApi } from '@/api/auth'
import { useLedgerUserStore } from '@/stores/ledger-user'
import { removeToken } from '@/utils/auth'
import AccessTokenDialog from './AccessTokenDialog.vue'
import PasswordDialog from './PasswordDialog.vue'
import TwoFactorDialog from './TwoFactorDialog.vue'

/** 登录用户菜单：内置账号可修改密码；开启登录时可退出（退出后回到登录页） */
const router = useRouter()
const ledgerUser = useLedgerUserStore()
const passwordVisible = ref(false)
const tokenVisible = ref(false)
const twoFactorVisible = ref(false)

async function logout() {
  if (ledgerUser.mode === 'local')
    await logoutApi().catch(() => {})
  removeToken()
  ledgerUser.reset()
  router.push({ name: 'Login' })
}

function onCommand(cmd: string) {
  if (cmd === 'password')
    passwordVisible.value = true
  else if (cmd === 'tokens')
    tokenVisible.value = true
  else if (cmd === 'twofactor')
    twoFactorVisible.value = true
  else if (cmd === 'logout')
    logout()
}
</script>

<template>
  <el-dropdown v-if="ledgerUser.authRequired && ledgerUser.user" trigger="click" data-testid="user-menu" @command="onCommand">
    <span class="cursor-pointer">
      <slot />
    </span>
    <template #dropdown>
      <el-dropdown-menu>
        <el-dropdown-item disabled>
          {{ ledgerUser.user.name }}{{ ledgerUser.isAdmin ? $t('（管理员）') : '' }}
        </el-dropdown-item>
        <el-dropdown-item v-if="ledgerUser.mode === 'local'" command="password" divided>
          {{ $t('修改密码') }}
        </el-dropdown-item>
        <el-dropdown-item v-if="ledgerUser.mode === 'local'" command="twofactor" data-testid="user-menu-twofactor">
          {{ $t('两步验证') }}
        </el-dropdown-item>
        <el-dropdown-item v-if="ledgerUser.mode === 'local'" command="tokens" data-testid="user-menu-tokens">
          {{ $t('访问令牌') }}
        </el-dropdown-item>
        <el-dropdown-item v-if="ledgerUser.mode === 'local' || ledgerUser.mode === 'gateway'" command="logout" :divided="ledgerUser.mode !== 'local'">
          {{ $t('退出登录') }}
        </el-dropdown-item>
      </el-dropdown-menu>
    </template>
  </el-dropdown>
  <slot v-else />
  <PasswordDialog v-model:visible="passwordVisible" />
  <AccessTokenDialog v-model:visible="tokenVisible" />
  <TwoFactorDialog v-model:visible="twoFactorVisible" />
</template>
