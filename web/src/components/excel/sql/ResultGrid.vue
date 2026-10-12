<script setup lang="ts">
import type { SqlResult } from '@/api/sql'
import BusyInline from '@/components/excel/common/BusyInline.vue'
import { t as tr } from '@/i18n'
import { fmt, KIND_LABEL } from './format'

/** 查询结果：错误提示 / 空状态 / 结果表格与底部状态栏 */
const props = defineProps<{
  result: SqlResult | null
  error: string
  running: boolean
}>()

function tsv() {
  const r = props.result!
  const line = (cells: unknown[]) => cells.map(v => (v == null ? '' : String(v).replace(/[\t\r\n]/g, ' '))).join('\t')
  return [line(r.columns.map(c => c.name)), ...r.rows.map(line)].join('\n')
}

async function copyResult() {
  try {
    await navigator.clipboard.writeText(tsv())
    ElMessage.success(tr('已复制 {0} 行（含表头），可直接粘贴到 Excel', [props.result!.rows.length.toLocaleString()]))
  }
  catch {
    ElMessage.error(tr('复制失败：浏览器不允许访问剪贴板'))
  }
}

/** 下载当前显示的结果（带 BOM，Excel 直接打开不乱码） */
function downloadCsv() {
  const r = props.result!
  const cell = (v: unknown) => {
    const t = v == null ? '' : String(v)
    return /[",\r\n]/.test(t) ? `"${t.replace(/"/g, '""')}"` : t
  }
  const text = [r.columns.map(c => cell(c.name)).join(','), ...r.rows.map(row => row.map(cell).join(','))].join('\r\n')
  const a = document.createElement('a')
  a.href = URL.createObjectURL(new Blob([`\uFEFF${text}`], { type: 'text/csv;charset=utf-8' }))
  a.download = `查询结果_${new Date().toISOString().slice(0, 10)}.csv`
  a.click()
  setTimeout(() => URL.revokeObjectURL(a.href), 1000)
}
</script>

<template>
  <div class="result">
    <div v-if="error" class="m-3 flex gap-2 rounded-md bg-[#FEF2F2] px-3 py-2.5 text-[13px] text-[#B91C1C] leading-6" role="alert">
      <span class="shrink-0 font-medium">{{ $t('出错了') }}</span>
      <span class="min-w-0 break-all">{{ error }}</span>
    </div>
    <div v-else-if="!result" class="p-3 text-[13px] text-gray-400">
      <BusyInline v-if="running" :loading="running" :text="$t('正在查询')" />
      <template v-else>
        {{ $t('运行后在这里查看结果') }}
      </template>
    </div>
    <template v-else>
      <div class="grid-wrap" @wheel.stop>
        <table>
          <thead>
            <tr>
              <th class="rownum">
                #
              </th>
              <th v-for="c in result.columns" :key="c.name" :class="{ num: c.kind === 'money' || c.kind === 'number' }" :title="`${c.name}（${KIND_LABEL[c.kind] ?? c.kind}，${c.sqlType}）`">
                {{ c.name }}
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(r, ri) in result.rows" :key="ri">
              <td class="rownum">
                {{ ri + 1 }}
              </td>
              <td v-for="(v, ci) in r" :key="ci" :class="{ num: result.columns[ci]!.kind === 'money' || result.columns[ci]!.kind === 'number', nil: v == null }">
                {{ fmt(v, result.columns[ci]!.kind) }}
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="status">
        {{ $t('{0} 行{1} · {2} ms', [result.rows.length.toLocaleString(), result.truncated ? $t('（结果更多，只显示前 1,000 行；另存为数据集可保存全部结果）') : '', result.elapsedMs]) }}
        <span v-if="result.datasets.length"> {{ $t('· 用到：{0}', [result.datasets.join('、')]) }}</span>
        <span class="ml-auto flex shrink-0 gap-1">
          <el-button link size="small" type="primary" :disabled="!result.rows.length" @click="copyResult">{{ $t('复制') }}</el-button>
          <el-button link size="small" type="primary" :disabled="!result.rows.length" @click="downloadCsv">{{ $t('下载 CSV') }}</el-button>
        </span>
      </div>
    </template>
  </div>
</template>

<style scoped lang="scss">
.result {
  @apply min-h-0 flex flex-1 flex-col overflow-hidden rounded-md border border-[#e5e7eb] border-solid;
}

.status {
  @apply flex shrink-0 items-center gap-1 border-0 border-t border-t-solid border-t-[#ebeae6] bg-[#fafaf9] px-3 py-1.5 text-xs text-gray-500;
}

.grid-wrap {
  @apply min-h-0 flex-1 overflow-auto;

  table {
    @apply border-collapse text-xs;
  }

  // 项目没有引入 CSS 重置：只写某一侧的 border-solid，否则其余三侧会按浏览器默认的 3px 画出来
  th {
    @apply sticky top-0 z-1 whitespace-nowrap border-0 border-b border-r border-b-solid border-r-solid border-b-[#e5e7eb] border-r-[#f0efec] bg-[#f9f9f7] px-2.5 py-1.5 text-left font-medium text-gray-600;
  }

  td {
    @apply max-w-80 truncate whitespace-nowrap border-0 border-b border-r border-b-solid border-r-solid border-b-[#f0efec] border-r-[#f0efec] px-2.5 py-1;
  }

  tbody tr:hover td {
    @apply bg-[#F4F9F6];
  }

  .num {
    @apply text-right tabular-nums;
  }

  .nil {
    @apply bg-[#fcfcfb];
  }

  .rownum {
    @apply sticky left-0 bg-[#f9f9f7] text-right text-gray-400;
  }
}
</style>
