<script setup lang="ts">
import type { ConnectorTestResult } from '@/api/connector'

/** 「测试连接」的结果：匹配到的文件 / 查询结果的列 / 接口返回的字段与响应开头 */
defineProps<{
  result: ConnectorTestResult | null
}>()

const sizeText = (n: number) => n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1e3))} KB`
</script>

<template>
  <div v-if="result">
    <div v-if="result?.files?.length" class="ml-[92px] max-h-40 overflow-auto rounded bg-[#F7F8FA] px-3 py-2 text-xs leading-6">
      <div v-for="f in result.files" :key="f.path" class="flex gap-3">
        <span class="flex-1 truncate">{{ f.name }}</span>
        <span class="text-gray-400">{{ sizeText(f.size) }}</span>
        <span class="text-gray-400">{{ f.mtime }}</span>
        <span :class="f.imported ? 'text-gray-400' : f.settling ? 'text-[#92400E]' : 'text-[#0B5A2E]'">
          {{ f.imported ? $t('已导入') : f.settling ? $t('写入中') : $t('待导入') }}
        </span>
      </div>
    </div>
    <div v-if="result?.database?.columns.length" class="ml-[92px] text-xs text-gray-500">
      {{ $t('查询结果的列：{0}', [result.database.columns.join('、')]) }}
    </div>
    <div v-if="result?.http" class="ml-[92px] flex flex-col gap-1 text-xs text-gray-500">
      <span v-if="result.http.fields.length">{{ $t('字段：{0}', [result.http.fields.join('、')]) }}</span>
      <details>
        <summary class="cursor-pointer">
          {{ $t('查看响应开头') }}
        </summary>
        <pre class="mt-1 max-h-48 overflow-auto rounded bg-[#F7F8FA] p-2 text-[11px] leading-4">{{ result.http.sample }}</pre>
      </details>
    </div>
  </div>
</template>
