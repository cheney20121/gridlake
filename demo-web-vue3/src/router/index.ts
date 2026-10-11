import type { RouteRecordRaw } from 'vue-router'
import { createRouter, createWebHistory } from 'vue-router'

const routes: RouteRecordRaw[] = [
  { path: '/', redirect: '/excel/home' },
  {
    path: '/login',
    name: 'Login',
    meta: { title: '登录', public: true },
    component: () => import('@/views/login/index.vue'),
  },
  {
    path: '/excel',
    name: 'ExcelWorkspace',
    meta: { title: '在线电子表格' },
    component: () => import('@/views/excel/index.vue'),
  },
  {
    path: '/excel/home',
    name: 'ExcelHome',
    meta: { title: '首页' },
    component: () => import('@/views/excel/home/index.vue'),
  },
  { path: '/:catchAll(.*)', redirect: '/excel/home' },
]

export const router = createRouter({
  history: createWebHistory(import.meta.env.VITE_PUBLIC_PATH),
  routes,
})
