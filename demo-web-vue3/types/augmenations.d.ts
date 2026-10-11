import 'vue-router'

export {}

declare module 'vue-router' {
  interface RouteMeta {
    /** 页面标题 */
    title?: string
    /** 不需要登录即可访问 */
    public?: boolean
  }
}
