<script setup lang="ts">
import type { DbForm, SourceKind } from './form-state'

/** 数据库来源（MySQL / PostgreSQL / ClickHouse）：地址、账号、只读查询语句 */
defineProps<{
  engine: SourceKind
  /** 修改已有连接器且已保存过密码（密码框留空表示沿用） */
  editing: boolean
  hasPassword: boolean
}>()

const db = defineModel<DbForm>({ required: true })
</script>

<template>
  <div class="row">
    <span class="label">{{ $t('地址') }}</span>
    <el-input v-model="db.host" class="!w-64" :placeholder="$t('主机名或 IP')" />
    <span class="text-gray-500">{{ $t('端口') }}</span>
    <el-input-number v-model="db.port" :min="1" :max="65535" :controls="false" class="!w-24" />
    <span class="text-gray-500">{{ $t('库名') }}</span>
    <el-input v-model="db.database" class="!w-44" />
  </div>
  <div class="row">
    <span class="label">{{ $t('账号') }}</span>
    <el-input v-model="db.user" class="!w-44" autocomplete="off" />
    <span class="text-gray-500">{{ $t('密码') }}</span>
    <el-input
      v-model="db.password"
      type="password"
      show-password
      class="!w-52"
      autocomplete="new-password"
      :placeholder="editing && hasPassword ? $t('已保存，留空不修改') : ''"
    />
    <template v-if="engine === 'clickhouse'">
      <span class="text-gray-500">{{ $t('协议') }}</span>
      <el-select v-model="db.ssl" class="!w-28">
        <el-option label="HTTP" value="disable" />
        <el-option label="HTTPS" value="require" />
      </el-select>
    </template>
    <template v-else>
      <span class="text-gray-500">{{ $t('加密连接') }}</span>
      <el-select v-model="db.ssl" class="!w-28">
        <el-option :label="$t('优先加密')" value="prefer" />
        <el-option :label="$t('必须加密')" value="require" />
        <el-option :label="$t('不加密')" value="disable" />
      </el-select>
    </template>
  </div>
  <div class="flex gap-3">
    <span class="label pt-1">{{ $t('查询语句') }}</span>
    <div class="flex flex-1 flex-col gap-1">
      <el-input
        v-model="db.sql"
        type="textarea"
        :rows="4"
        class="sql"
        placeholder="SELECT order_id, shop, amount, update_time FROM orders WHERE create_time >= '2026-01-01'"
      />
      <span v-if="engine === 'clickhouse'" class="text-xs text-gray-400">{{ $t('只支持一条 SELECT / WITH 查询，以只读方式执行（readonly=1）；端口为 HTTP 接口端口（默认 8123，HTTPS 8443）；单条语句最长执行 10 分钟') }}</span>
      <span v-else class="text-xs text-gray-400">{{ $t('只支持一条 SELECT / WITH 查询，在只读事务中执行（建议使用只有查询权限的账号）；单条语句最长执行 10 分钟') }}</span>
    </div>
  </div>
</template>

<style scoped lang="scss">
.row {
  @apply flex items-center gap-3;
}

.label {
  @apply w-20 shrink-0 text-gray-500;
}

.sql :deep(textarea) {
  font-family: ui-monospace, Menlo, Consolas, monospace;
}
</style>
