import type { Univer } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'
import type { InjectionKey } from 'vue'
import type { FloatSpec } from '@/features/excel/chart/chart-manager'
import type { ChartSpec } from '@/features/excel/chart/types'
import type { UniverContext } from '@/lib/univer'
import { inject, onMounted, onUnmounted, ref, shallowRef } from 'vue'
import { chartInstance } from '@/features/excel/chart/chart-images'
import { insertChart, insertFloat } from '@/features/excel/chart/chart-manager'
import { disposeUniver, initUniver } from '@/lib/univer'

/** Univer 实例的 provide/inject key */
export const UNIVER_KEY: InjectionKey<UniverContext> = Symbol('univer')

/**
 * Univer 实例组合式函数
 * 在组件中初始化 Univer，挂载到指定容器 ID
 */
export function useUniver(containerId: string = 'univer-container') {
  const univer = shallowRef<Univer | null>(null)
  // 必须用 shallowRef：ref 会深度代理 FUniver 并解包其类型，导致类型不再是 FUniver
  const univerAPI = shallowRef<FUniver | null>(null)
  const ready = ref(false)
  const error = ref<string | null>(null)

  onMounted(() => {
    try {
      const ctx = initUniver(containerId)
      univer.value = ctx.univer
      univerAPI.value = ctx.univerAPI
      ready.value = true
      // 仅开发环境：便于在控制台 / 端到端测试中检查工作簿
      if (import.meta.env.DEV) {
        const w = window as unknown as { __univerAPI?: FUniver, __gridlake?: unknown }
        w.__univerAPI = ctx.univerAPI
        // 端到端测试用：插入图表 / 看板部件、取页面上的图表实例
        w.__gridlake = {
          insertChart: (spec: ChartSpec) => insertChart(ctx.univerAPI, spec),
          insertFloat: (key: string, spec: FloatSpec, size: { width: number, height: number }) => insertFloat(ctx.univerAPI, key, spec, size),
          chart: chartInstance,
        }
      }
    }
    catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
      console.error('[useUniver] 初始化失败:', e)
    }
  })

  onUnmounted(() => {
    disposeUniver()
    ready.value = false
  })

  return {
    univer,
    univerAPI,
    ready,
    error,
  }
}

/**
 * 从父组件注入 Univer 上下文
 */
export function useUniverInject() {
  return inject(UNIVER_KEY, null)
}
