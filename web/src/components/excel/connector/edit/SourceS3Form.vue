<script setup lang="ts">
import type { S3Form } from './form-state'

/** 对象存储来源（S3 及兼容 S3 协议的存储）：收取某个前缀下的 CSV / XLSX */
defineProps<{
  /** 修改已有连接器且已保存过密钥（密钥框留空表示沿用） */
  editing: boolean
  hasPassword: boolean
}>()

const s3 = defineModel<S3Form>({ required: true })
</script>

<template>
  <div class="hint">
    {{ $t('收取 S3（或兼容 S3 协议的 MinIO、阿里云 OSS、腾讯云 COS 等）存储桶里某个前缀下的 CSV / XLSX：每个对象只导入一次，对象被覆盖更新后会重新导入。访问密钥只需要读权限（s3:ListBucket 与 s3:GetObject）。') }}
  </div>
  <div class="row">
    <span class="label">{{ $t('服务地址') }}</span>
    <el-input v-model="s3.endpoint" class="!w-[420px]" placeholder="https://minio.corp:9000" data-testid="s3-endpoint" />
    <span class="text-xs text-gray-400">{{ $t('用 AWS S3 时留空') }}</span>
  </div>
  <div class="row">
    <span class="label">{{ $t('存储桶') }}</span>
    <el-input v-model="s3.bucket" class="!w-56" data-testid="s3-bucket" />
    <span class="text-gray-500">{{ $t('区域') }}</span>
    <el-input v-model="s3.region" class="!w-36" placeholder="us-east-1" />
    <span class="text-gray-500">{{ $t('地址风格') }}</span>
    <el-select v-model="s3.pathStyle" class="!w-44">
      <el-option :label="$t('自动（推荐）')" value="auto" />
      <el-option :label="$t('路径风格')" value="path" />
      <el-option :label="$t('虚拟主机风格')" value="virtual" />
    </el-select>
  </div>
  <div class="row">
    <span class="label">{{ $t('访问密钥') }}</span>
    <el-input v-model="s3.accessKey" class="!w-56" autocomplete="off" placeholder="Access Key ID" data-testid="s3-access-key" />
    <el-input
      v-model="s3.secretKey"
      type="password"
      show-password
      class="!w-72"
      autocomplete="new-password"
      placeholder="Secret Access Key"
      data-testid="s3-secret-key"
    />
    <span v-if="editing && hasPassword" class="text-xs text-gray-400">{{ $t('已保存，留空不修改') }}</span>
  </div>
  <div class="row">
    <span class="label">{{ $t('前缀') }}</span>
    <el-input v-model="s3.prefix" class="!w-64" placeholder="reports/2026/" data-testid="s3-prefix" />
    <span class="text-gray-500">{{ $t('文件名规则') }}</span>
    <el-input v-model="s3.pattern" class="!w-52" :placeholder="$t('如 流水_*.csv')" data-testid="s3-pattern" />
    <el-checkbox v-model="s3.recursive">
      {{ $t('包含子目录') }}
    </el-checkbox>
  </div>
  <div class="ml-[92px] text-xs text-gray-400">
    {{ $t('* 任意字符、? 单个字符，多个规则用 ; 分隔；服务地址只写主机和端口，存储桶和前缀单独填') }}
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
</style>
