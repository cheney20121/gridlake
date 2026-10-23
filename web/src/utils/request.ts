import type { AxiosInstance, AxiosRequestConfig, InternalAxiosRequestConfig } from 'axios'
import axios from 'axios'
import { lang, t } from '@/i18n'
import { getToken, removeToken } from './auth'
import { getShareProof, getShareToken } from './share-token'

/**
 * 请求表格服务：响应为信封 { code, data, msg }，code 为 200 时返回 data，否则提示 msg 并抛出。
 * 登录令牌作为 x-user-token 请求头发送；登录失效（401）时提示后回到登录页
 */
export interface CustomConfig {
  /** 调用方自己在界面里显示错误（面板内的错误条、重试按钮）：不再弹全局错误提示，避免同一条错误出现两次 */
  silent?: boolean
  /** 下载文件（配合 responseType: 'blob'）：返回完整响应，便于读取文件名等响应头 */
  type?: 'download'
  /** 不带分享令牌 / 登录令牌（分享页输入访问密码的接口本身不需要，带上反而会被当作分享请求拒绝） */
  anonymous?: boolean
}

export interface ResultData<T> {
  code: number | string
  data: T
  msg?: string
}

export interface AxiosInstanceCustom extends AxiosInstance {
  <T = any>(config?: AxiosRequestConfig<any> & CustomConfig): Promise<T>
}

export const instance = axios.create({
  baseURL: '/',
  timeout: 60000,
  headers: {
    'Content-Type': 'application/json;charset=UTF-8',
    'Accept': 'application/json',
    // 会话 cookie 登录时，服务端要求写请求带这个头（防跨站请求伪造）
    'x-requested-with': 'XMLHttpRequest',
  },
  responseType: 'json',
}) as AxiosInstanceCustom

let expiredDialogShowing = false

/** 登录失效：提示后清掉令牌并回到登录页，登录后回到当前页面 */
function showTokenExpired() {
  if (expiredDialogShowing)
    return
  expiredDialogShowing = true
  ElMessageBox.confirm(t('登录状态已失效，请重新登录'), t('提示'), {
    confirmButtonText: t('登录'),
    cancelButtonText: t('取消'),
    type: 'warning',
  }).then(() => {
    removeToken()
    const back = window.location.pathname + window.location.search
    window.location.href = `${import.meta.env.BASE_URL}login?redirect=${encodeURIComponent(back)}`
  }).catch(() => {}).finally(() => {
    expiredDialogShowing = false
  })
}

/** 错误提示停留时间：服务端的说明可能很长（如「服务繁忙…」「内存不足…」），按长度留够阅读时间 */
function toast(msg: string) {
  // 分享页里部件偶尔会请求访客不能访问的接口：不弹错误打扰访客，分享页自己处理「链接失效」
  if (getShareToken()) {
    console.warn('[share]', msg)
    return
  }
  ElMessage({ type: 'error', message: msg, duration: Math.min(8, Math.max(2, Math.ceil(msg.length / 8))) * 1000 })
}

instance.interceptors.request.use((config: InternalAxiosRequestConfig & CustomConfig) => {
  // 分享页：只带分享令牌（访客没有账号）
  const share = config.anonymous ? null : getShareToken()
  const token = share || config.anonymous ? undefined : getToken()
  if (share) {
    config.headers['x-share-token'] = share
    const proof = getShareProof()
    if (proof)
      config.headers['x-share-proof'] = proof
  }
  if (token)
    config.headers['x-user-token'] = token
  // 前后端分开部署（跨域）时 cookie 不会带过去，用请求头告诉服务端界面语言
  config.headers['x-lang'] = lang
  // FormData 上传时删除 Content-Type，让浏览器自动设置 boundary
  if (config.data instanceof FormData)
    delete config.headers['Content-Type']
  return config
})

instance.interceptors.response.use((response) => {
  const config = response.config as InternalAxiosRequestConfig & CustomConfig
  if (config.type === 'download' && config.responseType === 'blob')
    return response as any
  const res = response.data as ResultData<unknown> | undefined
  // 非信封响应（文件、裸数组 / 对象）原样返回
  if (!res || typeof res !== 'object' || !('code' in res))
    return res as any
  if (Number(res.code) === 200)
    return res.data as any
  if (Number(res.code) === 401) {
    showTokenExpired()
    return Promise.reject(new Error(res.msg || t('请先登录')))
  }
  if (!config.silent)
    toast(res.msg || t('服务错误'))
  return Promise.reject(new Error(res.msg || t('服务错误')))
}, (error) => {
  // 主动取消（换了筛选 / 关闭面板时中断上一次请求）不是错误
  if (axios.isCancel(error))
    return Promise.reject(error)
  const resp = error.response
  if (resp?.status === 401 && resp.data?.code === 401) {
    showTokenExpired()
    return Promise.reject(error)
  }
  // 服务端返回了说明（{ code, msg }）时作为错误信息，调用方显示 e.message 时是可读的中文原因而不是「status code 400」
  if (typeof resp?.data?.msg === 'string' && resp.data.msg)
    error.message = resp.data.msg
  if (!error.config?.silent)
    toast(error.message || t('请求失败'))
  return Promise.reject(error)
})

export default instance
