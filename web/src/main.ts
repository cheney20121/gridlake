import { createPinia } from 'pinia'
import { createApp } from 'vue'
import AppVue from './App.vue'
import { i18nPlugin, lang } from './i18n'
import { loadUniverLocales } from './i18n/univer-locale'
import { router } from './router'
import { setupRouterGuard } from './router/guard'
import '@unocss/reset/sanitize/sanitize.css'
import '@unocss/reset/sanitize/assets.css'
import 'virtual:uno.css'
import '@/styles/index.scss'

async function bootstrap() {
  // 表格内核的界面语言包要在编辑器创建前加载好
  await loadUniverLocales(lang)

  const app = createApp(AppVue)
  app.use(i18nPlugin)
  app.use(createPinia())
  app.use(router)
  setupRouterGuard(router)
  app.mount('#app')
}

bootstrap()
