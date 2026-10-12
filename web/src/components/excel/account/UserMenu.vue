<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { logoutApi } from '@/api/auth'
import { useLedgerUserStore } from '@/stores/ledger-user'
import { removeToken } from '@/utils/auth'
import PasswordDialog from './PasswordDialog.vue'

/** 登录用户菜单：内置账号可修改密码；开启登录时可退出（退出后回到登录页） */
const router = useRouter()
const ledgerUser = useLedgerUserStore()
const passwordVisible = ref(false)

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
        <el-dropdown-item v-if="ledgerUser.mode === 'local' || ledgerUser.mode === 'gateway'" command="logout" :divided="ledgerUser.mode !== 'local'">
          {{ $t('退出登录') }}
        </el-dropdown-item>
      </el-dropdown-menu>
    </template>
  </el-dropdown>
  <slot v-else />
  <PasswordDialog v-model:visible="passwordVisible" />
</template>
