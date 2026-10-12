<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { KpiData } from '@/features/excel/dashboard/data'
import type { KpiSpec } from '@/features/excel/dashboard/types'
import { computed, onBeforeUnmount, onMounted, ref, shallowRef } from 'vue'
import { chartBus, sheetIdOf } from '@/features/excel/chart/chart-manager'
import { crossFilterAffecting, dashboardFiltersFor } from '@/features/excel/dashboard/cross-filter'
import { compact, full, loadKpi } from '@/features/excel/dashboard/data'
import { saveWidgetImage } from '@/features/excel/dashboard/export-image'
import { t } from '@/i18n'

/**
 * 指标卡：标题、数值（自动用万 / 亿）、同比 / 环比（箭头 + 文字，颜色表示好坏）、最近 12 期趋势
 * 切片器变化、数据集更新时自动重算；刷新时保留旧值（半透明）
 */
const props = defineProps<{
  spec: KpiSpec
  api: FUniver
}>()

const spec = shallowRef<KpiSpec>(props.spec)
/** 受图表联动筛选时显示标记 */
const cross = computed(() => crossFilterAffecting(sheetIdOf(props.api, spec.value.id), spec.value.datasetId, spec.value.id))
const data = shallowRef<KpiData | null>(null)
const loading = ref(false)
const error = ref('')
let controller: AbortController | null = null
const offs: (() => void)[] = []

async function load() {
  controller?.abort()
  const c = new AbortController()
  controller = c
  loading.value = true
  try {
    const extra = dashboardFiltersFor(props.api, sheetIdOf(props.api, spec.value.id), spec.value.datasetId)
    data.value = await loadKpi(spec.value, extra, c.signal)
    error.value = ''
  }
  catch (e) {
    if (!c.signal.aborted)
      error.value = e instanceof Error ? e.message : String(e)
  }
  finally {
    if (controller === c)
      loading.value = false
  }
}

/** 变化方向 × 上升是否是好事 → 好 / 坏 / 持平 */
const tone = computed(() => {
  const d = data.value?.delta
  if (d == null || Math.abs(d) < 0.0005)
    return 'flat'
  return (d > 0) === spec.value.upIsGood ? 'good' : 'bad'
})

const deltaText = computed(() => {
  const d = data.value?.delta
  if (d == null)
    return ''
  const pct = Math.abs(d * 100)
  return `${d > 0 ? '+' : d < 0 ? '−' : ''}${pct >= 100 ? pct.toFixed(0) : pct.toFixed(1)}%`
})

// 趋势小图（SVG）：弱化的灰线，当前所在期用强调色点出
const W = 120
const H = 30
const spark = computed(() => {
  const s = data.value?.spark
  if (!s || s.values.filter(v => v != null).length < 2)
    return null
  const nums = s.values.map(v => v ?? 0)
  const min = Math.min(...nums)
  const max = Math.max(...nums)
  const span = max - min || 1
  const x = (i: number) => (s.values.length === 1 ? W / 2 : (i / (s.values.length - 1)) * (W - 8) + 4)
  const y = (v: number) => H - 4 - ((v - min) / span) * (H - 8)
  const points = s.values.map((v, i) => ({ x: x(i), y: y(v ?? 0), key: s.keys[i]!, v, current: s.current.includes(s.keys[i]!) }))
  return { path: points.map((p, i) => `${i ? 'L' : 'M'}${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' '), points }
})

async function saveImage(e: MouseEvent) {
  e.stopPropagation()
  try {
    ElMessage.success(t('已保存图片：{0}', [await saveWidgetImage(e, '.kpi', spec.value.title || '指标卡')]))
  }
  catch (err) {
    ElMessage.error(t('保存图片失败：{0}', [err instanceof Error ? err.message : String(err)]))
  }
}

function edit(e: MouseEvent) {
  e.stopPropagation()
  chartBus.emit('select', { id: spec.value.id, kind: 'kpi' })
}

onMounted(() => {
  load()
  offs.push(
    chartBus.on('spec', (s: KpiSpec) => {
      if (s.id === spec.value.id) {
        spec.value = s
        load()
      }
    }),
    chartBus.on('slicer', (sheetId: string) => {
      if (sheetId === sheetIdOf(props.api, spec.value.id))
        load()
    }),
    chartBus.on('refresh-dataset', (id?: string) => {
      if (!id || id === spec.value.datasetId)
        load()
    }),
  )
})

onBeforeUnmount(() => {
  offs.forEach(off => off())
  controller?.abort()
})
</script>

<template>
  <div class="kpi" @dblclick="edit">
    <div class="head">
      <span class="label" :title="spec.title">{{ spec.title || $t('指标') }}</span>
      <span v-if="data?.period" class="period">{{ data.period }}</span>
      <span v-if="cross" class="cross" :title="$t('按图表点选的「{0}」筛选；在图表上再点一次或点空白处取消', [cross.label])">{{ $t('联动') }}</span>
      <span class="edit" :title="$t('保存为图片')" @pointerdown.stop @mousedown.stop @click="saveImage">📷</span>
      <span class="edit" :title="$t('编辑指标卡')" @pointerdown.stop @mousedown.stop @click="edit">⚙</span>
    </div>
    <div v-if="error" class="error">
      {{ error }}
    </div>
    <template v-else>
      <div class="value" :class="{ dim: loading && data }" :title="data ? full(data.value, data.money) : ''">
        {{ data ? compact(data.value, data.money) : '…' }}
      </div>
      <div class="foot">
        <span v-if="data?.delta != null" class="delta" :class="tone">
          <span class="arrow">{{ data.delta > 0 ? '▲' : data.delta < 0 ? '▼' : '—' }}</span>
          {{ deltaText }}
          <span class="vs">{{ data.deltaLabel }}</span>
        </span>
        <span v-else-if="spec.compare !== 'none' && data" class="vs">{{ $t('{0}：无基期数据', [data.deltaLabel]) }}</span>
        <svg v-if="spark" class="spark" :viewBox="`0 0 ${W} ${H}`" :width="W" :height="H">
          <path :d="spark.path" fill="none" stroke="#b5b3ab" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round" />
          <g v-for="p in spark.points" :key="p.key">
            <circle :cx="p.x" :cy="p.y" :r="p.current ? 3 : 6" :fill="p.current ? '#2a78d6' : 'transparent'" :stroke="p.current ? '#fff' : 'none'" :stroke-width="p.current ? 1.5 : 0">
              <title>{{ p.key }}：{{ full(p.v, data?.money) }}</title>
            </circle>
          </g>
        </svg>
      </div>
    </template>
  </div>
</template>

<style scoped>
.cross {
  flex-shrink: 0;
  padding: 0 5px;
  border-radius: 3px;
  background: #e7f4ec;
  color: #0b5a2e;
  font-size: 11px;
  line-height: 16px;
}

.kpi {
  position: relative;
  width: 100%;
  height: 100%;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  gap: 4px;
  padding: 10px 14px;
  background: #fff;
  border: 1px solid rgba(11, 11, 11, 0.1);
  border-radius: 6px;
  font-family: system-ui, -apple-system, 'PingFang SC', 'Microsoft YaHei', sans-serif;
  color: #0b0b0b;
  overflow: hidden;
}

.head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
}

.label {
  flex: 1;
  min-width: 0;
  color: #52514e;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.period {
  color: #898781;
  font-size: 12px;
}

.edit {
  color: #898781;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.15s;
}

.kpi:hover .edit {
  opacity: 1;
}

.value {
  font-size: 30px;
  font-weight: 600;
  line-height: 1.15;
  white-space: nowrap;
  transition: opacity 0.15s;
}

.value.dim {
  opacity: 0.5;
}

.foot {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 8px;
  min-height: 30px;
  font-size: 12px;
}

.delta {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  font-weight: 600;
  white-space: nowrap;
}

.delta.good {
  color: #006300;
}

.delta.bad {
  color: #d03b3b;
}

.delta.flat {
  color: #52514e;
}

.arrow {
  font-size: 10px;
}

.vs {
  margin-left: 2px;
  font-weight: 400;
  color: #898781;
}

.spark {
  flex: none;
}

.error {
  font-size: 12px;
  color: #b91c1c;
}
</style>
