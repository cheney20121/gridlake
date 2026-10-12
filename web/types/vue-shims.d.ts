declare module '*.vue' {
  import type { Component } from 'vue'

  const component: Component
  export default component
}

declare module '*.svg' {
  import type { VueConstructor } from 'vue'
  import type Vue from 'vue'

  const content: VueConstructor<Vue>
  export default content
}
declare global {
  interface Window {
    [key: string]: any
  }
  interface ImportMetaEnv extends ViteEnv {
    readonly VITE_APP_PAGE_CONFIG: string
    /** 店铺资金流水服务地址，默认走 Vite 代理 /ledger-api */
    readonly VITE_LEDGER_API?: string
  }

  interface ImportMeta {
    readonly env: ImportMetaEnv
  }

  declare interface ViteEnv {
    VITE_PORT: number
    VITE_GLOB_APP_TITLE: string
    VITE_BASEURL: string
    VITE_PRIMARY_COLOR: string
  }
}

declare module 'node-rsa-build'
