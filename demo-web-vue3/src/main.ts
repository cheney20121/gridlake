import { createPinia } from 'pinia'
import { createApp } from 'vue'
import AppVue from './App.vue'
import { router } from './router'
import { setupRouterGuard } from './router/guard'
import '@unocss/reset/sanitize/sanitize.css'
import '@unocss/reset/sanitize/assets.css'
import 'virtual:uno.css'
import '@/styles/index.scss'

const app = createApp(AppVue)
app.use(createPinia())
app.use(router)
setupRouterGuard(router)
app.mount('#app')
