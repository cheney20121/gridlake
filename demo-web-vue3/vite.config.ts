import type { ConfigEnv, UserConfig } from 'vite'
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'

import { fileURLToPath, URL } from 'node:url'
import vue from '@vitejs/plugin-vue'
import vueJsx from '@vitejs/plugin-vue-jsx'
import UnoCSS from 'unocss/vite'
import AutoImport from 'unplugin-auto-import/vite'
import { ElementPlusResolver } from 'unplugin-vue-components/resolvers'
import Components from 'unplugin-vue-components/vite'
import { defineConfig, loadEnv } from 'vite'
import { createHtmlPlugin } from 'vite-plugin-html'
import { viteStaticCopy } from 'vite-plugin-static-copy'

// 读取package.json获取项目名称
const packageJsonPath = path.resolve(process.cwd(), 'package.json')
const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, 'utf-8'))

// https://vite.dev/config/
export default defineConfig(({ mode }: ConfigEnv): UserConfig => {
  const env = loadEnv(mode, process.cwd())
  const isDev = mode === 'development'
  const appBase = `${(env.VITE_PUBLIC_PATH || '/').replace(/\/$/, '')}/`
  return {
    plugins: [
      vue(),
      vueJsx(),
      UnoCSS(),
      AutoImport({
        resolvers: [ElementPlusResolver({ importStyle: 'sass' })],
        eslintrc: {
          enabled: true,
          globalsPropValue: true,
        },
      }),
      Components({
        resolvers: [ElementPlusResolver({ importStyle: 'sass' })],
      }),
      // 运行时配置：config/index.json → web/config/index.js（部署后可直接改，不用重新构建）
      viteStaticCopy({
        targets: [{
          src: 'config/index.json',
          dest: 'config',
          rename: () => `index.js`,
          transform: async contents => `window.__webexcel_config = ${contents.toString()}`,
        }],
      }),
      createHtmlPlugin({
        minify: !isDev,
        inject: {
          data: {
            title: packageJson.title || 'WebExcel',
            injectScript: `
            <script src="${appBase}config/index.js?t=${new Date().getTime()}"></script>`,
          },
        },
      }),
    ],
    define: {
      __BUILD_TIME__: JSON.stringify(new Date(new Date().getTime() + 480 * 60 * 1000).toISOString().replace('T', ' ').slice(0, 19)),
    },
    base: appBase,
    server: {
      proxy: {
        // 店铺资金流水服务（ledger-server，Rust + DuckDB）
        '/ledger-api': {
          target: env.VITE_LEDGER_SERVER || 'http://127.0.0.1:8787',
          changeOrigin: true,
          rewrite: p => p.replace(/^\/ledger-api/, ''),
        },
      },
    },
    resolve: {
      alias: {
        '@': fileURLToPath(new URL('./src', import.meta.url)),
      },
    },
    css: {
      preprocessorOptions: {
        scss: {
          additionalData: `
            @use "@/styles/element/index.scss" as *;
          `,
        },
      },
    },
  }
})
