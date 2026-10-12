import type { Lang } from './index'

/**
 * Univer 自带界面（工具栏、右键菜单、查找替换等）的语言包：按需加载当前语言的，另一种不进首屏
 * 在应用挂载之前 await 一次，之后 initUniver 同步取用
 */
type Pack = Record<string, unknown>

async function load(lang: Lang): Promise<Pack[]> {
  const mods = lang === 'en-US'
    ? await Promise.all([
        import('@univerjs/preset-sheets-core/locales/en-US'),
        import('@univerjs/preset-sheets-drawing/locales/en-US'),
        import('@univerjs/preset-sheets-conditional-formatting/locales/en-US'),
        import('@univerjs/preset-sheets-data-validation/locales/en-US'),
        import('@univerjs/preset-sheets-filter/locales/en-US'),
        import('@univerjs/preset-sheets-sort/locales/en-US'),
        import('@univerjs/preset-sheets-find-replace/locales/en-US'),
        import('@univerjs/preset-sheets-hyper-link/locales/en-US'),
        import('@univerjs/preset-sheets-note/locales/en-US'),
        import('@univerjs/preset-sheets-table/locales/en-US'),
      ])
    : await Promise.all([
        import('@univerjs/preset-sheets-core/locales/zh-CN'),
        import('@univerjs/preset-sheets-drawing/locales/zh-CN'),
        import('@univerjs/preset-sheets-conditional-formatting/locales/zh-CN'),
        import('@univerjs/preset-sheets-data-validation/locales/zh-CN'),
        import('@univerjs/preset-sheets-filter/locales/zh-CN'),
        import('@univerjs/preset-sheets-sort/locales/zh-CN'),
        import('@univerjs/preset-sheets-find-replace/locales/zh-CN'),
        import('@univerjs/preset-sheets-hyper-link/locales/zh-CN'),
        import('@univerjs/preset-sheets-note/locales/zh-CN'),
        import('@univerjs/preset-sheets-table/locales/zh-CN'),
      ])
  return mods.map(m => m.default as Pack)
}

let packs: Pack[] = []

export async function loadUniverLocales(lang: Lang) {
  packs = await load(lang)
}

export function univerLocalePacks(): Pack[] {
  return packs
}
