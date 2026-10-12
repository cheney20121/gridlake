<script setup lang="ts">
import type { AnomalyFinding, AnomalyScan, DatasetInfo } from '@/api/dataset'
import { computed, ref, watch } from 'vue'
import { getAnomaliesApi } from '@/api/dataset'
import { t } from '@/i18n'

/**
 * 异常发现：逐个扫描数据集，列出最近 7 天相对此前 28 天明显偏离的地方（某天行数 / 金额骤降骤升、某个分组的日均大幅变化或消失）。
 * 只做可解释的统计，不依赖 AI；点「查看明细」直接打开对应数据集并带上筛选。
 */
const props = defineProps<{
  visible: boolean
  datasets: DatasetInfo[]
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'drill', finding: AnomalyFinding): void
  /** 请 AI 助手进一步分析 */
  (e: 'ask', finding: AnomalyFinding): void
}>()

interface Entry {
  dataset: DatasetInfo
  state: 'pending' | 'running' | 'done' | 'error'
  scan?: AnomalyScan
  error?: string
}

const entries = ref<Entry[]>([])
/** 每次打开重新扫描；关闭时作废还在途的扫描 */
let token = 0

async function run() {
  const mine = ++token
  entries.value = props.datasets.map(dataset => ({ dataset, state: 'pending' as const }))
  for (const e of entries.value) {
    if (mine !== token)
      return
    e.state = 'running'
    try {
      e.scan = await getAnomaliesApi(e.dataset.id)
      e.state = 'done'
    }
    catch (err) {
      e.state = 'error'
      e.error = err instanceof Error ? err.message : t('扫描失败')
    }
  }
}

watch(() => props.visible, (v) => {
  if (v)
    run()
  else
    token++
})

const progress = computed(() => entries.value.filter(e => e.state === 'done' || e.state === 'error').length)
const scanning = computed(() => entries.value.some(e => e.state === 'pending' || e.state === 'running'))
const withFindings = computed(() => entries.value.filter(e => e.scan?.findings.length))
const clean = computed(() => entries.value.filter(e => e.state === 'done' && !e.scan?.findings.length && !e.scan?.note))
const skipped = computed(() => entries.value.filter(e => (e.state === 'done' && e.scan?.note && !e.scan.findings.length) || e.state === 'error'))
const total = computed(() => withFindings.value.reduce((n, e) => n + (e.scan?.findings.length ?? 0), 0))

const SEVERITY: Record<string, { label: string, type: 'danger' | 'warning' | 'info' }> = {
  high: { label: t('高'), type: 'danger' },
  medium: { label: t('中'), type: 'warning' },
  low: { label: t('低'), type: 'info' },
}
</script>

<template>
  <el-dialog :model-value="visible" :title="$t('异常发现')" width="860px" append-to-body top="6vh" @update:model-value="emit('update:visible', $event)">
    <div class="mb-3 flex items-center gap-3 text-xs text-gray-500">
      <span>{{ $t('以数据里最新的一天为准，比较最近 7 天与此前 28 天：某天行数或金额明显偏离（已按星期折算，周末低谷不算）、某个店铺 / 渠道 / 状态的日均大幅变化或消失。') }}</span>
      <el-button class="ml-auto shrink-0" size="small" :loading="scanning" @click="run">
        {{ $t('重新扫描') }}
      </el-button>
    </div>
    <el-progress v-if="scanning" :percentage="entries.length ? Math.round(progress / entries.length * 100) : 0" :stroke-width="4" :show-text="false" class="mb-3" />

    <div class="anomaly-body max-h-[60vh] overflow-auto pr-1">
      <div v-for="e in withFindings" :key="e.dataset.id" class="mb-4">
        <div class="mb-1.5 flex items-baseline gap-2">
          <span class="font-medium">{{ e.dataset.name }}</span>
          <span class="text-xs text-gray-400">{{ $t('最新数据 {0} · 按「{1}」', [e.scan?.latestDate, e.scan?.timeColumn]) }}</span>
        </div>
        <div v-for="f in e.scan?.findings" :key="f.id" class="finding">
          <el-tag size="small" :type="SEVERITY[f.severity]?.type" effect="dark" class="shrink-0">
            {{ SEVERITY[f.severity]?.label }}
          </el-tag>
          <div class="min-w-0 flex-1">
            <div class="finding-title">
              {{ f.title }}
            </div>
            <div class="text-xs text-gray-500">
              {{ f.detail }}
            </div>
          </div>
          <el-button size="small" link type="primary" @click="emit('drill', f)">
            {{ $t('查看明细') }}
          </el-button>
          <el-button size="small" link @click="emit('ask', f)">
            {{ $t('问 AI') }}
          </el-button>
        </div>
      </div>

      <el-empty v-if="!scanning && !withFindings.length" :image-size="64" :description="$t('最近没有发现明显偏离')" />

      <div v-if="!scanning && entries.length" class="mt-2 border-t border-gray-100 pt-2 text-xs text-gray-400">
        <div v-if="total">
          {{ $t('共 {0} 处。', [total]) }}
        </div>
        <div v-if="clean.length">
          {{ $t('没有发现异常：{0}', [clean.map(e => e.dataset.name).join('、')]) }}
        </div>
        <div v-for="e in skipped" :key="e.dataset.id">
          {{ $t('未能比较「{0}」：{1}', [e.dataset.name, e.error ?? e.scan?.note]) }}
        </div>
      </div>
    </div>
  </el-dialog>
</template>

<style scoped lang="scss">
.finding {
  @apply flex items-center gap-2 rounded px-2 py-1.5;

  &:hover {
    background: #f6f8fa;
  }
}

.finding-title {
  @apply text-sm;
}
</style>
