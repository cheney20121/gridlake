<script setup lang="ts">
import type { ExternalInput, ExternalPreview } from '@/api/dataset'
import { computed, reactive, ref, watch } from 'vue'
import { listConnectorsApi } from '@/api/connector'
import { createExternalApi, previewExternalApi } from '@/api/dataset'
import BusyInline from '@/components/excel/common/BusyInline.vue'
import { formatBool, kindLabel } from '@/features/excel/dataset/column-kinds'
import { errorText } from '@/utils/busy-hint'

/**
 * 外部数据集：直接查询服务器收件目录里的 Parquet 文件，不导入（只读，文件有增减后在数据表上「刷新文件列表」）
 * 只有管理员能用；目录必须在 LEDGER_INBOX_ROOTS 之内
 */
const props = defineProps<{ visible: boolean }>()
const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'created', datasetId: string): void
}>()

const form = reactive<ExternalInput & { name: string }>({ name: '', path: '', pattern: '*.parquet', recursive: false, hive: false })
const roots = ref<string[]>([])
const preview = ref<ExternalPreview | null>(null)
const loading = ref(false)
const creating = ref(false)
const error = ref('')

watch(() => props.visible, (v) => {
  if (!v)
    return
  Object.assign(form, { name: '', path: '', pattern: '*.parquet', recursive: false, hive: false })
  preview.value = null
  error.value = ''
  // 允许的目录（与文件夹数据连接同一组）：只用来提示，拿不到不影响使用
  listConnectorsApi()
    .then((r) => {
      roots.value = r.inboxRoots
    })
    .catch(() => {
      roots.value = []
    })
})

// 改了来源就要重新预览
watch(() => [form.path, form.pattern, form.recursive, form.hive], () => {
  preview.value = null
})

function sizeText(n: number) {
  if (n >= 1024 ** 3)
    return `${(n / 1024 ** 3).toFixed(1)} GB`
  if (n >= 1024 ** 2)
    return `${(n / 1024 ** 2).toFixed(1)} MB`
  return `${Math.max(1, Math.round(n / 1024))} KB`
}

const input = computed<ExternalInput>(() => ({ path: form.path.trim(), pattern: form.pattern.trim() || '*.parquet', recursive: form.recursive, hive: form.hive }))

async function runPreview() {
  loading.value = true
  error.value = ''
  try {
    preview.value = await previewExternalApi(input.value)
    if (!form.name.trim())
      form.name = (form.path.trim().split(/[\\/]/).filter(Boolean).pop() ?? '').slice(0, 40)
  }
  catch (e) {
    error.value = errorText(e)
  }
  finally {
    loading.value = false
  }
}

async function create() {
  creating.value = true
  error.value = ''
  try {
    const id = await createExternalApi(form.name.trim(), input.value)
    emit('created', id)
    emit('update:visible', false)
  }
  catch (e) {
    error.value = errorText(e)
  }
  finally {
    creating.value = false
  }
}

function cell(v: unknown) {
  if (v == null)
    return ''
  return typeof v === 'boolean' ? formatBool(v) : String(v)
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('查询服务器上的 Parquet 文件')"
    width="860px"
    :close-on-click-modal="false"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="flex flex-col gap-3 text-[13px]">
      <div class="rounded bg-[#F7F8FA] px-3 py-2 text-xs text-gray-600 leading-5">
        {{ $t('不导入：直接读取服务器目录里的 Parquet 文件，数据始终是文件里的最新内容（文件有增减后在数据表上点「刷新文件列表」）。只读，可以筛选、透视、写公式和 SQL，不能填报写入。') }}
        <template v-if="roots.length">
          <br>{{ $t('可用的目录：{0}', [roots.join('、')]) }}
        </template>
      </div>
      <el-form label-width="96px" size="small" @submit.prevent>
        <el-form-item :label="$t('目录')" required>
          <el-input v-model="form.path" :placeholder="$t('如 订单（相对于第一个可用目录）或绝对路径')" @keyup.enter="runPreview" />
        </el-form-item>
        <el-form-item :label="$t('文件名规则')">
          <el-input v-model="form.pattern" class="!w-64" placeholder="*.parquet" />
          <span class="ml-2 text-xs text-gray-500">{{ $t('* 任意字符，多个用 ; 分隔') }}</span>
        </el-form-item>
        <el-form-item :label="$t('选项')">
          <el-checkbox v-model="form.recursive">
            {{ $t('包含子目录') }}
          </el-checkbox>
          <el-checkbox v-model="form.hive" :disabled="!form.recursive">
            {{ $t('把「年=2026/月=09」这样的目录名读成列') }}
          </el-checkbox>
        </el-form-item>
      </el-form>

      <div v-if="error" class="rounded bg-[#FEF2F2] px-3 py-2 text-[#B91C1C] leading-5" role="alert">
        {{ error }}
      </div>

      <template v-if="preview">
        <div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-gray-600">
          <span class="text-gray-900 font-medium">{{ $t('{0} 个文件', [preview.fileCount]) }}</span>
          <span>{{ $t('{0} 行', [preview.rows.toLocaleString()]) }}</span>
          <span>{{ sizeText(preview.size) }}</span>
          <span>{{ $t('{0} 列', [preview.columns.length]) }}</span>
          <span class="min-w-0 flex-1 truncate text-xs text-gray-400" :title="preview.files.join('\n')">
            {{ preview.files.join('、') }}<template v-if="preview.fileCount > preview.files.length"> …</template>
          </span>
        </div>
        <el-table :data="preview.sampleRows" size="small" max-height="260" border>
          <el-table-column v-for="(c, i) in preview.columns" :key="c.title" :min-width="120" show-overflow-tooltip>
            <template #header>
              <div class="leading-4">
                {{ c.title }}
                <div class="text-[11px] text-gray-400 font-normal" :title="c.sqlType">
                  {{ kindLabel(c.kind) }}
                </div>
              </div>
            </template>
            <template #default="{ row }">
              {{ cell(row[i]) }}
            </template>
          </el-table-column>
        </el-table>
        <el-form label-width="96px" size="small" @submit.prevent>
          <el-form-item :label="$t('数据集名称')" required>
            <el-input v-model="form.name" maxlength="40" class="!w-72" />
          </el-form-item>
        </el-form>
      </template>
    </div>

    <template #footer>
      <div v-if="loading" class="mr-auto text-xs">
        <BusyInline :loading="loading" :text="$t('正在读取文件结构')" />
      </div>
      <el-button @click="emit('update:visible', false)">
        {{ $t('取消') }}
      </el-button>
      <el-button v-if="!preview" type="primary" :loading="loading" :disabled="!form.path.trim()" @click="runPreview">
        {{ $t('预览') }}
      </el-button>
      <el-button v-else type="primary" :loading="creating" :disabled="!form.name.trim()" @click="create">
        {{ $t('创建数据集') }}
      </el-button>
    </template>
  </el-dialog>
</template>
