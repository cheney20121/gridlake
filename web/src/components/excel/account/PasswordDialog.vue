<script setup lang="ts">
import { reactive, ref, watch } from 'vue'
import { changePasswordApi } from '@/api/auth'
import { t } from '@/i18n'

/** 修改自己的密码（内置账号）：核对原密码；其他设备上的登录随之失效 */
const visible = defineModel<boolean>('visible', { default: false })
const form = reactive({ old: '', next: '', confirm: '' })
const saving = ref(false)
const error = ref('')

watch(visible, (v) => {
  if (v) {
    form.old = form.next = form.confirm = ''
    error.value = ''
  }
})

async function save() {
  error.value = ''
  if (form.next.length < 8)
    return (error.value = '新密码至少 8 位')
  if (form.next !== form.confirm)
    return (error.value = '两次输入的新密码不一致')
  saving.value = true
  try {
    await changePasswordApi(form.old, form.next)
    ElMessage.success(t('密码已修改，其他设备上的登录已失效'))
    visible.value = false
  }
  catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  }
  finally {
    saving.value = false
  }
}
</script>

<template>
  <el-dialog v-model="visible" :title="$t('修改密码')" width="380px" append-to-body>
    <el-form label-width="80px">
      <el-form-item :label="$t('原密码')">
        <el-input v-model="form.old" type="password" show-password autocomplete="current-password" />
      </el-form-item>
      <el-form-item :label="$t('新密码')">
        <el-input v-model="form.next" type="password" show-password autocomplete="new-password" :placeholder="$t('至少 8 位')" />
      </el-form-item>
      <el-form-item :label="$t('确认')">
        <el-input v-model="form.confirm" type="password" show-password autocomplete="new-password" @keyup.enter="save" />
      </el-form-item>
    </el-form>
    <div v-if="error" class="text-[13px] text-[#B91C1C]">
      {{ error }}
    </div>
    <template #footer>
      <el-button @click="visible = false">
        {{ $t('取消') }}
      </el-button>
      <el-button type="primary" :loading="saving" @click="save">
        {{ $t('保存') }}
      </el-button>
    </template>
  </el-dialog>
</template>
