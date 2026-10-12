<script setup lang="ts">
import type { Lang } from '@/i18n'
import { lang, LANGUAGES, setLang } from '@/i18n'

/** 语言切换：选中后记住选择并刷新页面 */
defineOptions({ name: 'LanguageSwitch' })

function onCommand(next: Lang) {
  if (next !== lang)
    setLang(next)
}
</script>

<template>
  <el-dropdown trigger="click" data-testid="language-switch" @command="onCommand">
    <span class="lang-switch" title="Language / 语言">
      <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3"><circle cx="8" cy="8" r="6" /><path d="M2 8h12M8 2c2 2 2 10 0 12M8 2c-2 2-2 10 0 12" /></svg>
      {{ lang === 'en-US' ? 'EN' : '中文' }}
    </span>
    <template #dropdown>
      <el-dropdown-menu>
        <el-dropdown-item v-for="l in LANGUAGES" :key="l.value" :command="l.value" :disabled="l.value === lang" :data-testid="`language-${l.value}`">
          {{ l.label }}
        </el-dropdown-item>
      </el-dropdown-menu>
    </template>
  </el-dropdown>
</template>

<style scoped>
.lang-switch {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
  font-size: 12px;
  color: #6b7280;
  white-space: nowrap;
}
.lang-switch:hover {
  color: #111827;
}
</style>
