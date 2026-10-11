<script setup lang="ts">
import type { HttpForm } from './form-state'
import { computed } from 'vue'
import { applyDoudianOrders, BODY_EXAMPLE, PH, URL_EXAMPLE, usedPlaceholders } from './form-state'

/** HTTP 接口来源：地址、请求体、鉴权（含抖店开放平台）、记录位置、分页 */
defineProps<{
  editing: boolean
  hasPassword: boolean
}>()

const http = defineModel<HttpForm>({ required: true })

/** 当前分页方式需要的占位符 */
const pagePlaceholder = computed(() => ({ none: '', page: PH.page, offset: PH.offset, cursor: PH.cursor })[http.value.pageKind])
const placeholders = computed(() => usedPlaceholders(http.value))
</script>

<template>
  <div class="hint">
    调用返回 JSON 的接口（ERP、自建系统、各类开放平台），按计划把记录同步进数据集。地址或请求体里可写占位符：
    <code>{{ PH.page }}</code> <code>{{ PH.size }}</code> 页码分页、<code>{{ PH.offset }}</code> 偏移分页、<code>{{ PH.cursor }}</code> 游标分页、<code>{{ PH.watermark }}</code> 增量起点（上次同步到的值）
  </div>
  <div class="row">
    <span class="label">接口地址</span>
    <el-select v-model="http.method" class="!w-24">
      <el-option label="GET" value="get" />
      <el-option label="POST" value="post" />
    </el-select>
    <el-input v-model="http.url" class="flex-1" :placeholder="URL_EXAMPLE" />
  </div>
  <div v-if="http.method === 'post'" class="flex gap-3">
    <span class="label pt-1">请求体</span>
    <el-input v-model="http.body" type="textarea" :rows="3" class="sql flex-1" :placeholder="BODY_EXAMPLE" />
  </div>
  <div class="row">
    <span class="label">鉴权</span>
    <el-select v-model="http.authKind" class="!w-40">
      <el-option label="不需要" value="none" />
      <el-option label="Bearer 令牌" value="bearer" />
      <el-option label="账号密码（Basic）" value="basic" />
      <el-option label="请求头密钥" value="header" />
      <el-option label="抖店开放平台（自用型）" value="doudian" />
    </el-select>
    <template v-if="http.authKind === 'doudian'">
      <el-input v-model="http.appKey" class="!w-40" placeholder="App Key" autocomplete="off" />
      <el-input v-model="http.shopId" class="!w-36" placeholder="店铺 ID（数字）" autocomplete="off" />
    </template>
    <el-input v-if="http.authKind === 'basic'" v-model="http.authUser" class="!w-40" placeholder="账号" autocomplete="off" />
    <el-input v-if="http.authKind === 'header'" v-model="http.authHeader" class="!w-40" placeholder="请求头名，如 X-API-Key" />
    <el-input
      v-if="http.authKind !== 'none'"
      v-model="http.secret"
      type="password"
      show-password
      class="!w-60"
      autocomplete="new-password"
      :placeholder="editing && hasPassword ? '已保存，留空不修改' : (http.authKind === 'basic' ? '密码' : http.authKind === 'doudian' ? 'App Secret' : '令牌 / 密钥')"
    />
    <span v-if="http.authKind !== 'none'" class="text-xs text-gray-400">加密保存，不会再显示</span>
  </div>
  <div v-if="http.authKind === 'doudian'" class="hint ml-[92px]">
    抖店自用型应用：地址写方法路径（<code>/order/searchList</code> 对应方法 <code>order.searchList</code>），请求体写业务参数，签名与访问令牌由平台自动处理；
    <code>{{ PH.now }}</code> 是当前时间的 Unix 秒数。参数名以抖店开放平台文档为准，先点「测试连接」确认。
    <el-button link type="primary" @click="applyDoudianOrders(http)">
      填入「订单列表」示例
    </el-button>
  </div>
  <div class="flex gap-3">
    <span class="label pt-1">请求头</span>
    <div class="flex flex-col gap-1.5">
      <div v-for="(h, i) in http.headers" :key="i" class="flex items-center gap-2">
        <el-input v-model="h.name" class="!w-44" placeholder="名称" />
        <el-input v-model="h.value" class="!w-72" placeholder="值（不要放密钥，密钥填在鉴权里）" />
        <el-button link type="danger" @click="http.headers.splice(i, 1)">
          删除
        </el-button>
      </div>
      <el-button link type="primary" class="self-start" @click="http.headers.push({ name: '', value: '' })">
        + 添加请求头
      </el-button>
    </div>
  </div>
  <div class="row">
    <span class="label">记录位置</span>
    <el-input v-model="http.recordsPath" class="!w-56" placeholder="如 data.list；为空表示整个响应是数组" />
    <span class="text-xs text-gray-400">嵌套对象展开成「a.b」列，数组保留为文本</span>
  </div>
  <div class="row">
    <span class="label">分页</span>
    <el-select v-model="http.pageKind" class="!w-36">
      <el-option label="不分页" value="none" />
      <el-option label="页码" value="page" />
      <el-option label="偏移量" value="offset" />
      <el-option label="游标" value="cursor" />
    </el-select>
    <template v-if="http.pageKind === 'page'">
      <span class="text-gray-500">起始页</span>
      <el-input-number v-model="http.pageStart" :min="0" :max="1" :controls="false" class="!w-16" />
    </template>
    <template v-if="http.pageKind === 'page' || http.pageKind === 'offset'">
      <span class="text-gray-500">每页</span>
      <el-input-number v-model="http.pageSize" :min="1" :max="10000" :controls="false" class="!w-24" />
      <span class="text-xs text-gray-400">取到不满一页为止</span>
    </template>
    <template v-if="http.pageKind === 'cursor'">
      <span class="text-gray-500">下一页游标</span>
      <el-input v-model="http.cursorPath" class="!w-48" placeholder="如 data.next_cursor" />
      <span class="text-xs text-gray-400">游标为空时结束</span>
    </template>
  </div>
  <div v-if="pagePlaceholder && !placeholders.includes(pagePlaceholder)" class="ml-[92px] text-xs text-[#B45309]">
    地址或请求体里还没有写 {{ pagePlaceholder }}
  </div>
</template>

<style scoped lang="scss">
.row {
  @apply flex items-center gap-3;
}

.label {
  @apply w-20 shrink-0 text-gray-500;
}

.hint {
  @apply rounded bg-[#F7F8FA] px-3 py-2 text-xs text-gray-600 leading-5;
}

.sql :deep(textarea) {
  font-family: ui-monospace, Menlo, Consolas, monospace;
}
</style>
