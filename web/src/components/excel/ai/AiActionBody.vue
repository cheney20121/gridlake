<script setup lang="ts">
import { computed } from 'vue'
import { t } from '@/i18n'

/**
 * AI 建议的内容：公式文本、一句话说明、结果预览表。单个建议卡与方案卡里的每一步共用。
 * kind / payload 与服务端建议一致（方案里每一步的 payload 自带 kind）
 */
const props = defineProps<{
  kind: string
  payload: Record<string, any>
}>()

const CHART_LABEL: Record<string, string> = { column: '柱形图', bar: t('条形图'), line: t('折线图'), area: t('面积图'), pie: t('饼图'), donut: t('环形图'), waterfall: t('瀑布图') }

const detail = computed(() => {
  const p = props.payload
  switch (props.kind) {
    case 'pivot': {
      const r = p.request ?? {}
      const f = (xs: { column: string, granularity?: string | null }[] = []) => xs.map(x => x.column + (x.granularity ? `（${{ year: t('年'), month: t('月'), day: t('日') }[x.granularity] ?? ''}）` : '')).join('、')
      return [t('数据集：{0}', [p.datasetName]), r.rows?.length ? t('行：{0}', [f(r.rows)]) : '', r.columns?.length ? t('列：{0}', [f(r.columns)]) : '', r.filters?.length ? t('筛选：{0} 个条件', [r.filters.length]) : ''].filter(Boolean).join('；')
    }
    case 'chart':
      return t('{0} · 数据集「{1}」· 插入当前工作表', [CHART_LABEL[p.spec?.type] ?? t('图表'), p.datasetName])
    case 'formula':
      return p.explanation || ''
    case 'calc':
      return t('数据集「{0}」· {1} 行中 {2} 行结果为空', [p.datasetName, p.preview?.totalRows?.toLocaleString?.() ?? '', p.preview?.nullRows ?? 0])
    case 'table_view':
      return t('数据表「{0}」· {1}', [p.table, p.preview?.note ?? ''])
    case 'column_format':
      return t('数据表「{0}」{1}列 · {2}', [p.table, p.column, p.preview?.note ?? ''])
    case 'table_create':
      return t('数据集「{0}」· {1}', [p.datasetName, p.preview?.note ?? ''])
    case 'dataset_save':
      return [p.purpose, p.preview?.note].filter(Boolean).join(' · ')
    case 'clean':
    case 'validation':
      return t('数据集「{0}」· {1}', [p.datasetName, p.preview?.note ?? ''])
    case 'window':
      return p.preview?.note ?? ''
  }
  return ''
})
</script>

<template>
  <div>
    <div v-if="kind === 'formula' || kind === 'calc' || kind === 'dataset_save'" class="formula">
      {{ kind === 'dataset_save' ? payload.sql : payload.formula }}
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
