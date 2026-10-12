<script setup lang="ts">
import type { DatasetInfo, DatasetUsage } from '@/api/dataset'
import { computed, ref, shallowRef, watch } from 'vue'
import { getDatasetUsageApi } from '@/api/dataset'
import { ACCESS_LABEL, partsText, referenceCount } from '@/features/excel/dataset/usage-text'
import { t } from '@/i18n'

/** 数据集使用情况：哪些工作簿 / 推送 / 视图 / SQL / 连接 / 模板在用它，近 30 天访问次数 */
const props = defineProps<{
  visible: boolean
  dataset: DatasetInfo | null
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'openWorkbook', id: string): void
}>()

const usage = shallowRef<DatasetUsage | null>(null)
const loading = ref(false)
const error = ref('')

watch(() => [props.visible, props.dataset?.id], async () => {
  if (!props.visible || !props.dataset)
    return
  loading.value = true
  error.value = ''
  usage.value = null
  try {
    usage.value = await getDatasetUsageApi(props.dataset.id)
  }
  catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  }
  finally {
    loading.value = false
  }
}, { immediate: true })

// ── 近 30 天访问柱图 ──
const W = 520
const H = 72
const GAP = 2
const bars = computed(() => {
  const days = usage.value?.access.days ?? []
  const max = Math.max(1, ...days.map(d => d.hits))
  const bw = W / Math.max(days.length, 1)
  return days.map((d, i) => {
    const h = d.hits ? Math.max(2, (d.hits / max) * (H - 4)) : 0
    return { ...d, x: i * bw + GAP / 2, w: bw - GAP, y: H - h, h, label: t('{0}：{1} 次', [d.day.slice(5), d.hits.toLocaleString()]) }
  })
})
const peak = computed(() => Math.max(0, ...(usage.value?.access.days ?? []).map(d => d.hits)))
const hover = ref<number | null>(null)

const kinds = computed(() => Object.entries(usage.value?.access.byKind ?? {}).sort((a, b) => b[1] - a[1]))
const refs = computed(() => (usage.value ? referenceCount(usage.value) : 0))

/** 工作簿以外的引用，按影响大小排列 */
const sections = computed(() => {
  const u = usage.value
  if (!u)
    return []
  return [
    { key: 'reports', title: t('定时推送'), items: u.reports },
    { key: 'views', title: t('关联视图'), items: u.views },
    { key: 'sql', title: t('SQL 查询'), items: u.sqlQueries },
    { key: 'connectors', title: t('数据连接'), items: u.connectors },
    { key: 'templates', title: t('模板'), items: u.templates },
  ].filter(x => x.items.length)
})
</script>

<template>
  <el-dialog :model-value="visible" :title="$t('数据集使用情况 · {0}', [dataset?.name ?? ''])" width="min(680px, 94vw)" append-to-body @update:model-value="emit('update:visible', $event)">
    <div v-loading="loading" class="min-h-40 flex flex-col gap-4 text-[13px]">
      <div v-if="error" class="text-[#B91C1C]">
        {{ error }}
      </div>
      <template v-else-if="usage">
        <!-- 访问 -->
        <section class="flex flex-col gap-2">
          <div class="sec-title">
            {{ $t('近 30 天访问') }}
            <span class="font-normal text-gray-500">{{ $t('共 {0} 次', [usage.access.total.toLocaleString()]) }}<template v-if="peak">{{ $t('，单日最多 {0} 次', [peak.toLocaleString()]) }}</template><template v-if="usage.access.lastDay">{{ $t('，最近一次 {0}', [usage.access.lastDay]) }}</template></span>
          </div>
          <div v-if="usage.access.total" class="relative">
            <svg :viewBox="`0 0 ${W} ${H}`" class="block h-18 w-full" preserveAspectRatio="none">
              <line :x1="0" :x2="W" :y1="H - 0.5" :y2="H - 0.5" stroke="#e5e7eb" />
              <g v-for="(b, i) in bars" :key="b.day">
                <!-- 命中区比柱子大：整列都可悬停 -->
                <rect :x="b.x - 1" y="0" :width="b.w + 2" :height="H" fill="transparent" @mouseenter="hover = i" @mouseleave="hover = null" />
                <rect v-if="b.h" :x="b.x" :y="b.y" :width="b.w" :height="b.h" rx="2" :fill="hover === i ? '#1d5fae' : '#2a78d6'" pointer-events="none" />
              </g>
            </svg>
            <div v-if="hover != null && bars[hover]" class="tip" :style="{ left: `${((bars[hover]!.x + bars[hover]!.w / 2) / W) * 100}%` }">
              {{ bars[hover]!.label }}
            </div>
            <div class="flex justify-between text-[11px] text-gray-400">
              <span>{{ bars[0]?.day.slice(5) }}</span>
              <span>{{ $t('今天') }}</span>
            </div>
          </div>
          <div v-else class="text-gray-400">
            {{ $t('近 30 天没有被查询过（统计从升级到此版本后开始）') }}
          </div>
          <div v-if="kinds.length" class="flex flex-wrap gap-x-4 gap-y-1 text-xs text-gray-600">
            <span v-for="[k, n] in kinds" :key="k">{{ ACCESS_LABEL[k] ?? k }} <b class="tabular-nums">{{ n.toLocaleString() }}</b></span>
          </div>
        </section>

        <div v-if="!refs && !usage.writebacks" class="rounded bg-[#F9FAFB] p-3 text-gray-500">
          {{ $t('没有工作簿、推送、视图、SQL 查询、数据连接或模板引用这个数据集') }}
        </div>

        <!-- 工作簿 -->
        <section v-if="usage.workbooks.length" class="flex flex-col gap-1">
          <div class="sec-title">
            {{ $t('工作簿 {0}', [usage.workbooks.length]) }}
          </div>
          <div v-for="w in usage.workbooks" :key="w.id" class="row">
            <a class="link" :title="$t('打开这个工作簿')" @click="emit('openWorkbook', w.id)">{{ w.name }}</a>
            <span class="text-gray-600">{{ partsText(w.parts) }}</span>
            <span class="ml-auto shrink-0 text-xs text-gray-400">{{ w.updatedAt }} {{ w.updatedBy ?? '' }}</span>
          </div>
          <div class="text-xs text-gray-400">
            {{ $t('「公式」为按名称引用的 DSUMIFS 等，改名后需同步修改；其余部件按数据集 ID 引用，改名不受影响') }}
          </div>
        </section>

        <section v-for="sec in sections" :key="sec.key" class="flex flex-col gap-1">
          <div class="sec-title">
            {{ sec.title }} {{ sec.items.length }}
          </div>
          <div v-for="it in sec.items" :key="it.id" class="row">
            <span>{{ it.name }}</span>
            <span class="text-gray-600">{{ it.role }}</span>
            <span v-if="it.enabled === false" class="text-xs text-gray-400">{{ $t('（已停用）') }}</span>
          </div>
        </section>

        <div v-if="usage.writebacks" class="text-xs text-gray-500">
          {{ $t('填报提交 {0} 次（工作表里的填报区写入这个数据集）', [usage.writebacks]) }}
        </div>
      </template>
    </div>
  </el-dialog>
</template>

<style scoped lang="scss">
.sec-title {
  @apply flex items-center gap-2 text-xs font-semibold text-gray-700;
}

.row {
  @apply flex items-center gap-3 border-b border-[#f3f4f6] border-b-solid py-1 last:border-b-0;
}

.link {
  @apply cursor-pointer text-[#217346] hover:underline;
}

.tip {
  @apply pointer-events-none absolute top-0 -translate-x-1/2 -translate-y-full whitespace-nowrap rounded bg-[#1f2937] px-2 py-0.5 text-xs text-white;
}
</style>
