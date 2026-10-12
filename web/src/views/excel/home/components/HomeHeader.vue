<script setup lang="ts">
import { Plus } from '@element-plus/icons-vue'
import { computed } from 'vue'
import LanguageSwitch from '@/components/common/LanguageSwitch.vue'
import UserMenu from '@/components/excel/account/UserMenu.vue'

/** 首页顶栏：品牌、登录用户、新建入口 */
const props = defineProps<{
  userName?: string
  isAdmin?: boolean
  creating?: boolean
}>()

const emit = defineEmits<{
  (e: 'template'): void
  (e: 'create'): void
}>()

const initial = computed(() => props.userName?.trim().charAt(0).toUpperCase() ?? '')
</script>

<template>
  <header class="home-header">
    <div class="logo">
      <svg viewBox="0 0 16 16" width="16" height="16"><path d="M3 3h10v10H3z" fill="none" stroke="#fff" stroke-width="1.2" /><path d="M3 6.5h10M3 9.8h10M7 3v10" stroke="#fff" stroke-width="1" /></svg>
    </div>
    <span class="whitespace-nowrap text-[15px] font-semibold tracking-wide">Gridlake</span>
    <span class="tagline">{{ $t('亿级数据表格') }}</span>
    <div class="flex-1" />
    <LanguageSwitch />
    <UserMenu v-if="userName">
      <span class="user-chip" :title="$t('登录账号：{0}', [userName])">
        <span class="avatar">{{ initial }}</span>
        <span class="max-w-32 truncate">{{ userName }}</span>
        <span v-if="isAdmin" class="admin-tag">{{ $t('管理员') }}</span>
      </span>
    </UserMenu>
    <el-button @click="emit('template')">
      {{ $t('从模板新建') }}
    </el-button>
    <el-button type="primary" :icon="Plus" :loading="creating" @click="emit('create')">
      {{ $t('新建工作簿') }}
    </el-button>
  </header>
</template>

<style scoped lang="scss">
.home-header {
  @apply z-10 h-14 flex shrink-0 items-center gap-3 border-b border-[#e8ebe9] border-b-solid bg-white px-6;

  box-shadow: 0 1px 2px rgb(16 24 20 / 4%);
}

.logo {
  @apply h-7 w-7 flex items-center justify-center rounded-lg;

  background: linear-gradient(135deg, #1fa35f, #0b6b38);
  box-shadow: 0 2px 6px rgb(16 124 65 / 30%);
}

.tagline {
  @apply whitespace-nowrap border-l border-[#e3e6e4] border-l-solid pl-3 text-xs text-gray-400;
}

.user-chip {
  @apply mr-2 flex items-center gap-2 text-[13px] text-gray-600;
}

.avatar {
  @apply h-6 w-6 flex items-center justify-center rounded-full bg-[#E7F4EC] text-xs font-semibold text-[#0E5A31];
}

.admin-tag {
  @apply rounded bg-[#E7F4EC] px-1.5 py-0.5 text-[11px] text-[#0E5A31];
}
</style>
