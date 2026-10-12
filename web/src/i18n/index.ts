/**
 * 国际化
 *
 * 做法：**中文原文就是键**。代码里写 `t('保存')`、`t('共 {0} 行', [n])`、`t('{name} 已删除', { name })`；
 * 简体中文界面直接显示原文，英文界面从 `locales/en-US.json` 查表，查不到就回退成中文原文——所以没翻译的地方
 * 只是显示中文，不会出现空白或键名，可以一个文件一个文件地逐步迁移。
 *
 * 不用 vue-i18n：我们只需要「查表 + 占位符替换」，用中文当键时 vue-i18n 的消息语法（`@` `|` `{` `$` 是特殊字符、
 * 回退时占位符不会被替换）反而碍事。语言在页面加载时确定，切换语言会刷新页面（Univer 的语言也只能在初始化时指定），
 * 所以不需要响应式。
 *
 * 新增文案：写好 `t('中文')` 后运行 `pnpm i18n:check`，它会列出缺少英文的键；在 `locales/en-US.json` 里补上即可。
 */
import type { Plugin } from 'vue'
import enUS from './locales/en-US.json' with { type: 'json' }

export type Lang = 'zh-CN' | 'en-US'

export const LANGUAGES: { value: Lang, label: string }[] = [
  { value: 'zh-CN', label: '简体中文' },
  { value: 'en-US', label: 'English' },
]

const STORAGE_KEY = 'gridlake-lang'

function normalize(v: string | null | undefined): Lang | null {
  const s = (v ?? '').toLowerCase()
  if (s.startsWith('zh'))
    return 'zh-CN'
  if (s.startsWith('en'))
    return 'en-US'
  return null
}

/** 优先级：网址里的 ?lang=（同时记住）→ 上次选的 → 浏览器语言（中文用中文，其他一律英文） */
export function detectLang(): Lang {
  // 单元测试等没有浏览器的环境：固定中文（测试断言的是中文原文）
  if (typeof window === 'undefined')
    return 'zh-CN'
  const fromUrl = normalize(new URLSearchParams(window.location.search).get('lang'))
  if (fromUrl) {
    // 记住：编辑器是整页跳转打开的，不带 ?lang= 的后续页面也要保持同一种语言
    try {
      window.localStorage.setItem(STORAGE_KEY, fromUrl)
    }
    catch {}
    return fromUrl
  }
  let saved: string | null = null
  try {
    saved = window.localStorage.getItem(STORAGE_KEY)
  }
  catch {}
  return normalize(saved) ?? normalize(window.navigator.language) ?? 'en-US'
}

export const lang: Lang = detectLang()
if (typeof document !== 'undefined')
  document.documentElement.lang = lang

const table: Record<string, string> = lang === 'en-US' ? enUS : {}

export type Params = readonly unknown[] | Record<string, unknown>

/** 占位符：`{0}` `{1}`（传数组）或 `{name}`（传对象）。找不到的占位符原样保留 */
export function format(text: string, params?: Params): string {
  if (!params)
    return text
  return text.replace(/\{(\w+)\}/g, (m, k: string) => {
    const v = (params as Record<string, unknown>)[k]
    return v === undefined ? m : String(v)
  })
}

export function t(text: string, params?: Params): string {
  return format(table[text] ?? text, params)
}

export const isEnglish = lang === 'en-US'

/** 数字 / 日期格式用的 locale */
export const numberLocale = lang === 'en-US' ? 'en-US' : 'zh-CN'

/** 记住选择并刷新页面 */
export function setLang(next: Lang) {
  try {
    window.localStorage.setItem(STORAGE_KEY, next)
  }
  catch {}
  const url = new URL(window.location.href)
  url.searchParams.delete('lang')
  window.location.href = url.href
}

export const i18nPlugin: Plugin = {
  install(app) {
    app.config.globalProperties.$t = t
  },
}

declare module 'vue' {
  interface ComponentCustomProperties {
    $t: typeof t
  }
}
