<script setup lang="ts">
import { useStorage } from '@vueuse/core'
import en from 'element-plus/es/locale/lang/en.mjs'
import zh from 'element-plus/es/locale/lang/zh-cn.mjs'
import { RouterView } from 'vue-router'
import { isEnglish } from './i18n'
import { useElementPlusTheme } from './utils/colorUtils'

defineOptions({ name: 'App' })

const themeColor = useStorage('theme-color', import.meta.env.VITE_INIT_THEME)
useElementPlusTheme(themeColor.value)
const namespace = import.meta.env.VITE_ELEMENT_NAMESPACE
const elLocale = isEnglish ? en : zh
</script>

<template>
  <el-config-provider :namespace="namespace" :locale="elLocale">
    <div id="app-container">
      <RouterView v-slot="{ Component }">
        <keep-alive :max="5">
          <component :is="Component" />
        </keep-alive>
      </RouterView>
    </div>
  </el-config-provider>
</template>

<style lang="scss">
  #app-container {
    @apply h-full overflow-hidden bg-#F2F4F7;
  }
</style>
