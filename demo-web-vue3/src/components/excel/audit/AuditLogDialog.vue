<script setup lang="ts">
import type { AuditItem } from '@/api/auth'
import { ref, watch } from 'vue'
import { getAuditApi } from '@/api/auth'

/** 操作审计（仅管理员）：登录用户的写操作与导出，含被拒绝的；读取类请求不记 */
const props = defineProps<{
  visible: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
}>()

const items = ref<AuditItem[]>([])
const total = ref(0)
const days = ref(0)
const loading = ref(false)
const page = ref(1)
const size = 50
const user = ref('')
const action = ref('')
const range = ref<[string, string] | null>(null)
const failed = ref(false)

async function load() {
  loading.value = true
  try {
    const r = await getAuditApi({
      user: user.value.trim() || undefined,
      action: action.value.trim() || undefined,
      from: range.value?.[0],
      to: range.value?.[1],
      failed: failed.value || undefined,
      page: page.value,
      size,
    })
    items.value = r.items
    total.value = r.total
    days.value = r.days
  }
  finally {
    loading.value = false
  }
}

function search() {
  page.value = 1
  load()
}

watch(() => props.visible, (v) => {
  if (v)
    search()
})

function target(row: AuditItem) {
  if (!row.targetName && !row.targetId)
    return ''
  return row.targetName ? `${row.targetKind ? `${row.targetKind}：` : ''}${row.targetName}` : row.targetId
}
</script>

<template>
  <el-dialog :model-value="visible" title="操作审计" width="980px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div class="mb-3 flex flex-wrap items-center gap-2">
      <el-input v-model="user" size="small" clearable placeholder="用户名 / 用户 ID" class="!w-40" @keyup.enter="search" @clear="search" />
      <el-input v-model="action" size="small" clearable placeholder="操作，如 删除" class="!w-40" @keyup.enter="search" @clear="search" />
      <el-date-picker v-model="range" type="daterange" size="small" value-format="YYYY-MM-DD" range-separator="至" start-placeholder="开始日期" end-placeholder="结束日期" class="!w-64" @change="search" />
      <el-checkbox v-model="failed" size="small" @change="search">
        只看失败 / 被拒绝
      </el-checkbox>
      <el-button size="small" type="primary" @click="search">
        查询
      </el-button>
      <span class="ml-auto text-xs text-gray-400">共 {{ total.toLocaleString() }} 条{{ days ? `，保留 ${days} 天` : '' }}</span>
    </div>
    <el-table v-loading="loading" :data="items" size="small" max-height="460" empty-text="没有符合条件的记录">
      <el-table-column label="时间" prop="at" width="150" />
      <el-table-column label="用户" width="130">
        <template #default="{ row }">
          <span :title="`ID：${row.userId}`">{{ row.userName }}</span>
        </template>
      </el-table-column>
      <el-table-column label="操作" prop="action" min-width="150" show-overflow-tooltip />
      <el-table-column label="对象" min-width="160" show-overflow-tooltip>
        <template #default="{ row }">
          {{ target(row) }}
        </template>
      </el-table-column>
      <el-table-column label="结果" width="90">
        <template #default="{ row }">
          <el-tag v-if="row.status < 400" size="small" type="success">
            成功
          </el-tag>
          <el-tag v-else-if="row.status === 403" size="small" type="warning" title="权限不足被拒绝">
            被拒绝
          </el-tag>
          <el-tag v-else size="small" type="danger" :title="`HTTP ${row.status}`">
            失败
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column label="来源 IP" prop="ip" width="120" />
      <el-table-column label="耗时" width="70" align="right">
        <template #default="{ row }">
          {{ row.ms >= 1000 ? `${(row.ms / 1000).toFixed(1)} s` : `${row.ms} ms` }}
        </template>
      </el-table-column>
    </el-table>
    <el-pagination
      v-if="total > size"
      v-model:current-page="page"
      class="mt-3 justify-end"
      small
      background
      layout="prev, pager, next"
      :page-size="size"
      :total="total"
      @current-change="load"
    />
  </el-dialog>
</template>
