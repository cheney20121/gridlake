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
          transform: async contents => `window.__gridlake_config = ${contents.toString()}`,
        }],
      }),
      createHtmlPlugin({
        minify: !isDev,
        inject: {
          data: {
            title: packageJson.title || 'Gridlake',
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
    // 开发服务器启动时一次性预构建全部依赖：Element Plus 的组件与样式是按需自动引入的，扫描源码时看不到，
    // 否则第一次打开某个页面才发现新依赖、重新预构建，已加载的模块返回 504（Outdated Optimize Dep）并整页刷新；
    // 缓存为空的全新环境（如 CI）上界面测试因此成片失败
    optimizeDeps: {
      entries: ['index.html', 'src/**/*.vue'],
      include: ['element-plus/es', 'element-plus/es/components/*/style/index'],
    },
    server: {
      proxy: {
        // 店铺资金流水服务（gridlake，Rust + DuckDB）
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
