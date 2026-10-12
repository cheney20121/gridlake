import { t } from '@/i18n'
/** 大数据计算的等待提示与错误文案（纯函数，便于单测） */

/** 服务端 /api/health 的 heavy 字段：大计算并发上限、正在进行、排队中 */
export interface QueueStatus {
  limit: number
  running: number
  waiting: number
}

/** 8 秒 / 1 分 05 秒 */
export function formatElapsed(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds))
  return s < 60 ? `${s} 秒` : `${Math.floor(s / 60)} 分 ${String(s % 60).padStart(2, '0')} 秒`
}

/**
 * 等待超过几秒后给出的说明：先判断是不是在排队，再解释为什么慢，太久了建议缩小范围
 * 3 秒以内返回空（多数查询瞬间完成，不打扰）
 */
export function busyNote(seconds: number, queue: QueueStatus | null): string {
  if (seconds < 3)
    return ''
  if (queue && queue.running >= queue.limit && queue.waiting > 0)
    return t('服务器正在处理 {0} 个大计算，另有 {1} 个在排队，会按顺序执行', [queue.running, queue.waiting])
  if (seconds >= 30)
    return t('仍在计算。可以取消后缩小筛选范围，或稍后再试')
  return t('数据量较大，首次排序 / 筛选 / 汇总需要几秒到十几秒；完成后会缓存，再次打开很快')
}

interface HttpLikeError {
  isAxiosError?: boolean
  code?: string
  message?: string
  response?: { data?: { msg?: string } }
}

/** 把请求错误转成给用户看的一句话：优先用服务端的说明（如「服务繁忙」「内存不足」） */
export function errorText(e: unknown, fallback = '操作失败，请重试'): string {
  if (typeof e === 'string')
    return e || fallback
  const err = e as HttpLikeError | null
  const server = err?.response?.data?.msg
  if (server)
    return server
  if (err?.isAxiosError && !err.response) {
    return err.code === 'ECONNABORTED' || err.code === 'ETIMEDOUT'
      ? t('等待超时。数据量较大时请缩小筛选范围后重试')
      : t('无法连接服务，请检查网络或服务是否在运行')
  }
  return err?.message || fallback
}
