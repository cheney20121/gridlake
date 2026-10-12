import type { Ref } from 'vue'
import type { QueueStatus } from '@/utils/busy-hint'
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { LEDGER_BASE } from '@/api/ledger-base'
import { busyNote, formatElapsed } from '@/utils/busy-hint'

/**
 * 长时间等待的计时与说明：active 变为 true 起每秒累计；等满 3 秒后开始（每 2 秒）问一次服务端是否在排队，
 * 给出「已用 N 秒」与原因说明。健康检查用 fetch，失败时静默（不弹错误提示）
 */
export function useBusy(active: Ref<boolean>) {
  const seconds = ref(0)
  const queue = ref<QueueStatus | null>(null)
  let timer: ReturnType<typeof setInterval> | undefined
  let started = 0

  async function pollQueue() {
    try {
      const r = await fetch(`${LEDGER_BASE}/api/health`)
      const body = await r.json()
      if (active.value)
        queue.value = body?.data?.heavy ?? null
    }
    catch {
      queue.value = null
    }
  }

  function stop() {
    if (timer)
      clearInterval(timer)
    timer = undefined
  }

  watch(active, (on) => {
    stop()
    seconds.value = 0
    queue.value = null
    if (!on)
      return
    started = Date.now()
    timer = setInterval(() => {
      seconds.value = Math.floor((Date.now() - started) / 1000)
      if (seconds.value >= 3 && seconds.value % 2 === 1)
        pollQueue()
    }, 1000)
  }, { immediate: true })

  onBeforeUnmount(stop)

  return {
    seconds,
    elapsed: computed(() => formatElapsed(seconds.value)),
    note: computed(() => busyNote(seconds.value, queue.value)),
  }
}
