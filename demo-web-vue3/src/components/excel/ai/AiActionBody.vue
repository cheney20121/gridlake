<script setup lang="ts">
import { computed } from 'vue'

/**
 * AI 建议的内容：公式文本、一句话说明、结果预览表。单个建议卡与方案卡里的每一步共用。
 * kind / payload 与服务端建议一致（方案里每一步的 payload 自带 kind）
 */
const props = defineProps<{
  kind: string
  payload: Record<string, any>
}>()

const CHART_LABEL: Record<string, string> = { column: '柱形图', bar: '条形图', line: '折线图', area: '面积图', pie: '饼图', donut: '环形图', waterfall: '瀑布图' }

const detail = computed(() => {
  const p = props.payload
  switch (props.kind) {
    case 'pivot': {
      const r = p.request ?? {}
      const f = (xs: { column: string, granularity?: string | null }[] = []) => xs.map(x => x.column + (x.granularity ? `（${{ year: '年', month: '月', day: '日' }[x.granularity] ?? ''}）` : '')).join('、')
      return [`数据集：${p.datasetName}`, r.rows?.length ? `行：${f(r.rows)}` : '', r.columns?.length ? `列：${f(r.columns)}` : '', r.filters?.length ? `筛选：${r.filters.length} 个条件` : ''].filter(Boolean).join('；')
    }
    case 'chart':
      return `${CHART_LABEL[p.spec?.type] ?? '图表'} · 数据集「${p.datasetName}」· 插入当前工作表`
    case 'formula':
      return p.explanation || ''
    case 'calc':
      return `数据集「${p.datasetName}」· ${p.preview?.totalRows?.toLocaleString?.() ?? ''} 行中 ${p.preview?.nullRows ?? 0} 行结果为空`
  }
  return ''
})
</script>

<template>
  <div>
    <div v-if="kind === 'formula' || kind === 'calc'" class="formula">
      {{ payload.formula }}
    </div>
    <div v-if="detail" class="mt-1 text-xs text-gray-500">
      {{ detail }}
    </div>
    <div v-if="payload.preview?.header" class="preview">
      <table>
        <thead>
          <tr>
            <th v-for="(h, i) in payload.preview.header" :key="i">
              {{ h }}
            </th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(r, ri) in payload.preview.rows.slice(0, 6)" :key="ri">
            <td v-for="(c, ci) in r" :key="ci" :class="{ num: Number(ci) > 0 }">
              {{ c }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <div v-if="kind === 'calc' && payload.preview?.samples?.length" class="preview">
      <div v-for="(s, si) in payload.preview.samples.slice(0, 3)" :key="si" class="truncate text-xs text-gray-500">
        {{ Object.entries(s).map(([k, v]) => `${k}=${v}`).join('，') }}
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.formula {
  @apply mt-1 break-all rounded bg-[#f4f4f1] px-1.5 py-1 text-xs font-mono;
}

.preview {
  @apply mt-1.5 overflow-x-auto;

  table {
    @apply w-full border-collapse text-xs;
  }

  th {
    @apply whitespace-nowrap bg-[#f4f4f1] px-1.5 py-0.5 text-left font-normal text-gray-500;
  }

  td {
    @apply whitespace-nowrap border-b border-b-[#f0efec] border-b-solid px-1.5 py-0.5;
  }

  td.num {
    @apply text-right tabular-nums;
  }
}
</style>
