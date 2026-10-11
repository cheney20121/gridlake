import type { FUniver } from '@univerjs/core/facade'

/**
 * 取 facade 没有暴露的 Univer 内部服务（在已解析的依赖里找）。
 * 服务在用到时才创建（如功能区服务在界面渲染后），找不到时返回 null，调用方可稍后重试
 */
type Match = (id: string, instance: Record<string, unknown>) => boolean

function find<T>(api: FUniver, match: Match): T | null {
  const injector = (api as unknown as { _injector?: { resolvedDependencyCollection?: { resolvedDependencies?: Map<unknown, unknown[]> } } })._injector
  const deps = injector?.resolvedDependencyCollection?.resolvedDependencies
  if (!deps)
    return null
  for (const [key, list] of deps.entries()) {
    for (const d of list) {
      const v = ((d as { value?: unknown })?.value ?? d) as Record<string, unknown> | null
      if (v && match(String(key), v))
        return v as T
    }
  }
  return null
}

/** 按方法找（只有一个服务具备这些方法时用） */
export function findUniverService<T>(api: FUniver, methods: string[]): T | null {
  return find<T>(api, (_, v) => methods.every(m => typeof v[m] === 'function'))
}

/** 按依赖注入标识找，如 univer.drawing-manager.service */
export function findUniverServiceById<T>(api: FUniver, id: string): T | null {
  return find<T>(api, key => key === id)
}
