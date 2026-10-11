import type { FUniver } from '@univerjs/core/facade'
import { findUniverService } from '@/features/excel/univer-internal'

/**
 * 把平台功能放进 Univer 功能区：新增「分析」等页签，或往已有页签（如「插入」）的分组里追加按钮。
 *
 * - 按钮只有文字（不传图标时功能区显示标题），点击执行 run
 * - contextual 页签默认隐藏，用 setTabVisible 按上下文显示（如台账工作簿才显示「台账」）
 * - facade 的 createMenu().appendTo() 不能给新页签设标题和顺序，这里取它生成的菜单项，
 *   再通过同一个菜单服务合并完整的页签结构
 */

export interface RibbonAction {
  id: string
  title: string
  tooltip?: string
  /** 命令面板的额外搜索词（功能区不用） */
  keywords?: string
  run: () => void
}

export interface RibbonGroup {
  key: string
  items: RibbonAction[]
}

export interface RibbonTab {
  /** 页签 key，如 ribbon.analysis；已有页签用 Univer 的 key（ribbon.insert） */
  key: string
  /** 新页签必填；追加到已有页签时省略 */
  title?: string
  /** 开始 0、插入 1、公式 2、数据 3、视图 4 */
  order?: number
  contextual?: boolean
  groups: RibbonGroup[]
}

export interface AppRibbon {
  setTabVisible: (key: string, visible: boolean) => void
}

type MenuSchema = Record<string, unknown>

interface FMenuInternal {
  __getSchema: () => MenuSchema
  _menuManagerService: { mergeMenu: (source: MenuSchema) => void }
}

interface RibbonServiceLike {
  showContextualTab: (tab: string) => void
  hideContextualTab: (tab: string) => void
}

export function registerAppRibbon(api: FUniver, tabs: RibbonTab[]): AppRibbon {
  let merge: FMenuInternal['_menuManagerService'] | null = null
  const ribbon: Record<string, MenuSchema> = {}
  for (const tab of tabs) {
    const tabSchema: MenuSchema = {}
    if (tab.title)
      tabSchema.title = tab.title
    if (tab.order != null)
      tabSchema.order = tab.order
    if (tab.contextual)
      tabSchema.contextual = true
    tab.groups.forEach((group, gi) => {
      const groupSchema: MenuSchema = { order: 10 + gi }
      group.items.forEach((item, ii) => {
        const menu = api.createMenu({ id: item.id, title: item.title, tooltip: item.tooltip ?? item.title, order: ii, action: item.run }) as unknown as FMenuInternal
        merge ??= menu._menuManagerService
        Object.assign(groupSchema, menu.__getSchema())
      })
      tabSchema[group.key] = groupSchema
    })
    ribbon[tab.key] = tabSchema
  }
  ;(merge as FMenuInternal['_menuManagerService'] | null)?.mergeMenu({ ribbon })

  // 功能区服务在界面渲染后才创建：用到时再找，还没有就稍后重试
  let service: RibbonServiceLike | null = null
  const apply = (key: string, visible: boolean, retries = 50) => {
    service ??= findUniverService<RibbonServiceLike>(api, ['showContextualTab', 'hideContextualTab'])
    if (!service) {
      if (retries > 0)
        setTimeout(() => apply(key, visible, retries - 1), 100)
      return
    }
    if (visible)
      service.showContextualTab(key)
    else
      service.hideContextualTab(key)
  }
  return { setTabVisible: (key, visible) => apply(key, visible) }
}
