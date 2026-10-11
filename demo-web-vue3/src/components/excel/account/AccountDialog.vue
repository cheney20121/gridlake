<script setup lang="ts">
import type { Account } from '@/api/auth'
import { reactive, ref, watch } from 'vue'
import { createAccountApi, listAccountsApi, updateAccountApi } from '@/api/auth'
import { useLedgerUserStore } from '@/stores/ledger-user'

/** 账号管理（内置账号，管理员）：新建、改显示名、设为管理员、停用 / 启用、重置密码 */
const visible = defineModel<boolean>('visible', { default: false })
const ledgerUser = useLedgerUserStore()
const accounts = ref<Account[]>([])
const loading = ref(false)
const creating = ref(false)
const form = reactive({ username: '', name: '', password: '', admin: false })

async function load() {
  loading.value = true
  try {
    accounts.value = await listAccountsApi()
  }
  finally {
    loading.value = false
  }
}

watch(visible, (v) => {
  if (v)
    load()
})

async function create() {
  creating.value = true
  try {
    await createAccountApi({ username: form.username.trim(), name: form.name.trim(), password: form.password, admin: form.admin })
    ElMessage.success(`已创建账号「${form.username.trim()}」`)
    form.username = form.name = form.password = ''
    form.admin = false
    await load()
  }
  finally {
    creating.value = false
  }
}

async function patch(a: Account, data: Parameters<typeof updateAccountApi>[1], done?: string) {
  await updateAccountApi(a.id, data)
  if (done)
    ElMessage.success(done)
  await load()
}

async function resetPassword(a: Account) {
  try {
    const { value } = await ElMessageBox.prompt(`给「${a.name}」设置新密码（至少 8 位）。设置后对方已登录的设备会退出。`, '重置密码', {
      inputType: 'password',
      inputValidator: v => (v?.length ?? 0) >= 8 || '至少 8 位',
      confirmButtonText: '重置',
    })
    await patch(a, { password: value }, '密码已重置')
  }
  catch {}
}

async function rename(a: Account) {
  try {
    const { value } = await ElMessageBox.prompt('显示名（审批、评论、审计里看到的名字）', '修改显示名', { inputValue: a.name, inputValidator: v => !!v?.trim() || '不能为空' })
    await patch(a, { name: value.trim() })
  }
  catch {}
}
</script>

<template>
  <el-dialog v-model="visible" title="账号管理" width="820px" append-to-body>
    <div class="mb-3 flex items-end gap-2 rounded-md bg-[#f9f9f7] p-3" data-testid="account-new">
      <el-input v-model="form.username" placeholder="用户名（登录用）" class="w-40" />
      <el-input v-model="form.name" placeholder="显示名（可选）" class="w-36" />
      <el-input v-model="form.password" type="password" show-password placeholder="初始密码（至少 8 位）" class="w-48" />
      <el-checkbox v-model="form.admin">
        管理员
      </el-checkbox>
      <el-button type="primary" :loading="creating" :disabled="!form.username.trim() || form.password.length < 8" @click="create">
        新建账号
      </el-button>
    </div>
    <el-table v-loading="loading" :data="accounts" size="small" max-height="420">
      <el-table-column prop="username" label="用户名" min-width="120" />
      <el-table-column label="显示名" min-width="120">
        <template #default="{ row }">
          <span class="cursor-pointer hover:text-[#107C41]" title="点击修改" @click="rename(row)">{{ row.name }}</span>
        </template>
      </el-table-column>
      <el-table-column label="管理员" width="80">
        <template #default="{ row }">
          <el-switch :model-value="row.admin" size="small" @change="(v: string | number | boolean) => patch(row, { admin: !!v })" />
        </template>
      </el-table-column>
      <el-table-column label="状态" width="80">
        <template #default="{ row }">
          <el-tag :type="row.disabled ? 'info' : 'success'" size="small">
            {{ row.disabled ? '已停用' : '正常' }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column label="最近登录" width="150">
        <template #default="{ row }">
          {{ row.lastLoginAt ?? '—' }}
        </template>
      </el-table-column>
      <el-table-column label="操作" width="150">
        <template #default="{ row }">
          <el-button link size="small" type="primary" @click="resetPassword(row)">
            重置密码
          </el-button>
          <el-button v-if="row.id !== ledgerUser.user?.id" link size="small" :type="row.disabled ? 'primary' : 'danger'" @click="patch(row, { disabled: !row.disabled }, row.disabled ? '已启用' : '已停用，对方已登录的设备会退出')">
            {{ row.disabled ? '启用' : '停用' }}
          </el-button>
        </template>
      </el-table-column>
    </el-table>
    <div class="mt-3 text-xs text-gray-400 leading-5">
      账号不能删除（它创建的工作簿、审批与审计记录要能追溯），不再使用时停用即可。至少要保留一个可用的管理员。
    </div>
  </el-dialog>
</template>
