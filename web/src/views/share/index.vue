<script setup lang="ts">
import type { IWorkbookData } from '@univerjs/core'
import type { FUniver } from '@univerjs/core/facade'
import { nextTick, onMounted, onUnmounted, ref, watchEffect } from 'vue'
import { useRoute } from 'vue-router'
import { getLedgerMeApi } from '@/api/auth'
import { getDatasetsApi } from '@/api/dataset'
import { unlockShareApi } from '@/api/share'
import { getWorkbookContentApi } from '@/api/workbook'
import ChartView from '@/components/excel/chart/ChartView.vue'
import KpiView from '@/components/excel/dashboard/KpiView.vue'
import SlicerView from '@/components/excel/dashboard/SlicerView.vue'
import TableCardView from '@/components/excel/dashboard/TableCardView.vue'
import DataTableFloat from '@/components/excel/datatable/DataTableFloat.vue'
import { useBodyClass } from '@/composables/useBodyClass'
import { useUniver } from '@/composables/useUniver'
import { registerChartComponent, registerFloatView } from '@/features/excel/chart/chart-manager'
import { guardPresenting } from '@/features/excel/dashboard/present-guard'
import { setDatasetCatalog } from '@/features/excel/dashboard/slicer'
import { KPI_KEY, SLICER_KEY, TABLE_KEY } from '@/features/excel/dashboard/types'
import { DATA_TABLE_KEY } from '@/features/excel/datatable/types'
import { registerDatasetFunctions } from '@/features/excel/formula/dataset-functions'
import { setShareProof, setShareToken } from '@/utils/share-token'

/**
 * 分享链接的只读查看页：不需要登录，只能看这一个工作簿（和它用到的数据集）。
 * 复用工作簿页的表格引擎与图表 / 指标卡 / 切片器部件；编辑类操作由演示模式的拦截（present-guard）挡住，
 * 数据请求带分享令牌（request.ts），服务端只放行只读接口。切片器、图表联动照常可用
 */
defineOptions({ name: 'ShareView' })

useBodyClass('app-excel')
const route = useRoute()
const token = String(route.params.token ?? '')
// 必须先于任何请求设置
setShareToken(token)

const { univerAPI, error } = useUniver('univer-container')
const state = ref<'loading' | 'password' | 'ready' | 'invalid' | 'failed'>('loading')
const password = ref('')
const passwordError = ref('')
const unlocking = ref(false)
const title = ref('')
const message = ref('')
let started = false
let guard: ReturnType<typeof guardPresenting> | null = null

/** 隐藏编辑用的界面：只留工作表和页签 */
function hideEditingChrome(api: FUniver) {
  const ui = api as unknown as { setUIVisible?: (k: string, v: boolean) => unknown }
  for (const part of ['header', 'toolbar'])
    ui.setUIVisible?.(part, false)
}

async function open(api: FUniver) {
  try {
    // 先问一声是不是有效的链接（失效时不弹全局错误，由本页说明）
    const me = await getLedgerMeApi()
    const wb = me.share
    if (me.mode !== 'share' || !wb) {
      state.value = 'invalid'
      return
    }
    // 设了访问密码：先让访客输入，服务端不会透露工作簿名称
    if (wb.passwordRequired) {
      setShareProof(null)
      state.value = 'password'
      return
    }
    title.value = wb.workbookName
    document.title = `${wb.workbookName} · Gridlake`
    // 先读工作簿内容：服务端据此算出它用到的数据集，之后才开放这些数据集的只读查询
    const { json } = await getWorkbookContentApi(wb.workbookId)
    setDatasetCatalog(await getDatasetsApi())
    registerDatasetFunctions(api)
    // 图表组件须在打开工作簿前注册：工作簿快照里的图表按组件名重建
    registerChartComponent(api, ChartView)
    registerFloatView(api, SLICER_KEY, SlicerView)
    registerFloatView(api, KPI_KEY, KpiView)
    registerFloatView(api, TABLE_KEY, TableCardView)
    registerFloatView(api, DATA_TABLE_KEY, DataTableFloat)
    guard = guardPresenting(api, () => document.getElementById('univer-container'), () => true)
    api.createWorkbook(JSON.parse(json) as IWorkbookData)
    hideEditingChrome(api)
    state.value = 'ready'
    // 部件创建完成后记下它们的位置：拖动被拦下后据此摆回去
    setTimeout(() => guard?.start(), 800)
    api.addEvent(api.Event.ActiveSheetChanged, () => setTimeout(() => guard?.start(), 800))
  }
  catch (e) {
    const text = e instanceof Error ? e.message : String(e)
    if (text.includes('无效') || text.includes('invalid') || text.includes('expired')) {
      state.value = 'invalid'
      return
    }
    state.value = 'failed'
    message.value = text
  }
}

/** 输入访问密码：对了换到通行证，再走一遍打开流程 */
async function unlock() {
  if (!password.value || unlocking.value || !univerAPI.value)
    return
  unlocking.value = true
  passwordError.value = ''
  try {
    const { proof } = await unlockShareApi(token, password.value)
    setShareProof(proof)
    password.value = ''
    state.value = 'loading'
    await open(univerAPI.value)
  }
  catch (e) {
    const text = e instanceof Error ? e.message : String(e)
    if (text.includes('无效') || text.includes('invalid'))
      state.value = 'invalid'
    else
      passwordError.value = text
  }
  finally {
    unlocking.value = false
  }
}

onMounted(() => {
  watchEffect(async () => {
    if (univerAPI.value && !started) {
      started = true
      await nextTick()
      await open(univerAPI.value)
    }
  })
})

onUnmounted(() => {
  guard?.dispose()
  setShareToken(null)
})
</script>

<template>
  <div class="share-view">
    <header class="share-bar">
      <strong class="truncate">{{ title || $t('分享的工作簿') }}</strong>
      <span class="badge">{{ $t('只读') }}</span>
      <span class="flex-1" />
      <span class="powered">Gridlake</span>
    </header>
    <div class="share-body">
      <div id="univer-container" class="univer-container" />
      <div v-if="state !== 'ready'" class="share-state">
        <template v-if="state === 'loading' && !error">
          {{ $t('正在加载…') }}
        </template>
        <form v-else-if="state === 'password'" class="password-box" @submit.prevent="unlock">
          <div class="text-base text-gray-800 font-medium">
            {{ $t('这个分享链接需要访问密码') }}
          </div>
          <div class="mt-1 text-xs text-gray-500">
            {{ $t('请向分享人要密码。') }}
          </div>
          <el-input
            v-model="password"
            class="mt-3"
            type="password"
            show-password
            maxlength="128"
            autofocus
            data-testid="share-password"
            :placeholder="$t('访问密码')"
          />
          <div v-if="passwordError" class="mt-2 text-xs text-[#B91C1C]" data-testid="share-password-error">
            {{ passwordError }}
          </div>
          <el-button class="mt-3 w-full" type="primary" native-type="submit" :loading="unlocking" :disabled="!password" data-testid="share-unlock">
            {{ $t('打开') }}
          </el-button>
        </form>
        <template v-else-if="state === 'invalid'">
          <div class="text-base text-gray-800 font-medium">
            {{ $t('这个分享链接已失效或不存在') }}
          </div>
          <div class="mt-1">
            {{ $t('可能已过期、被分享人撤销，或者工作簿已被删除。请向分享人要一个新的链接。') }}
          </div>
        </template>
        <template v-else>
          <div class="text-base text-gray-800 font-medium">
            {{ $t('打开失败') }}
          </div>
          <div class="mt-1">
            {{ error || message }}
          </div>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.share-view {
  @apply flex h-100vh flex-col bg-white;
}

.share-bar {
  @apply flex h-10 shrink-0 items-center gap-2 border-0 border-b border-[#e1e0d9] border-solid bg-[#f7f7f4] px-4 text-sm;

  .badge {
    @apply rounded bg-[#EEF2EF] px-1.5 py-0.5 text-xs text-[#0E5A31];
  }

  .powered {
    @apply text-xs text-gray-400;
  }
}

.share-body {
  @apply relative min-h-0 flex-1;

  .univer-container {
    width: 100%;
    height: 100%;
  }
}

.password-box {
  @apply w-72 text-center;
}

.share-state {
  @apply absolute inset-0 z-2000 flex flex-col items-center justify-center bg-white text-sm text-gray-500;
}
</style>
