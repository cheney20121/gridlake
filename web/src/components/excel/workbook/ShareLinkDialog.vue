<script setup lang="ts">
import type { CreatedShareLink, ShareLink } from '@/api/share'
import { ref, watch } from 'vue'
import { createShareLinkApi, listShareLinksApi, revokeShareLinkApi, shareUrl } from '@/api/share'
import { t } from '@/i18n'

/** 分享链接：把当前工作簿只读开放给没有账号的人。链接创建时显示一次（服务端只存摘要），之后只能吊销、不能再看到 */
const props = defineProps<{
  visible: boolean
  workbookId: string | null
  workbookName: string
  /** 分享前保存当前工作簿（访客看到的是已保存的版本）；返回 false 表示保存失败 */
  beforeShare?: () => Promise<boolean>
}>()

const emit = defineEmits<{ (e: 'update:visible', val: boolean): void }>()

const DAYS = [
  { label: t('7 天'), value: 7 },
  { label: t('30 天'), value: 30 },
  { label: t('90 天'), value: 90 },
  { label: t('永不过期'), value: 0 },
]

const links = ref<ShareLink[]>([])
const loading = ref(false)
const creating = ref(false)
const form = ref({ note: '', days: 30 })
const created = ref<CreatedShareLink | null>(null)

async function load() {
  if (!props.workbookId)
    return
  loading.value = true
  try {
    links.value = await listShareLinksApi(props.workbookId)
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, async (v) => {
  if (!v)
    return
  created.value = null
  form.value = { note: '', days: 30 }
  links.value = []
  if (props.beforeShare && !(await props.beforeShare())) {
    emit('update:visible', false)
    return
  }
  load()
}, { immediate: true })

async function create() {
  if (!props.workbookId)
    return
  creating.value = true
  try {
    created.value = await createShareLinkApi(props.workbookId, form.value.note.trim(), form.value.days || null)
    form.value.note = ''
    await load()
  }
  finally {
    creating.value = false
  }
}

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    ElMessage.success(t('已复制'))
  }
  catch {
    ElMessage.warning(t('复制失败，请手动选中复制'))
  }
}

async function revoke(l: ShareLink) {
  await ElMessageBox.confirm(t('吊销后，拿着这个链接的人立即打不开。确定吗？'), l.note || t('分享链接 {0}…', [l.prefix]), { type: 'warning', confirmButtonText: t('吊销'), cancelButtonText: t('取消') })
  await revokeShareLinkApi(l.id)
  if (created.value?.id === l.id)
    created.value = null
  ElMessage.success(t('已吊销'))
  load()
}

function expiryText(l: ShareLink) {
  if (l.expired)
    return t('已过期')
  return l.expiresAt ? t('{0} 到期', [l.expiresAt.slice(0, 16)]) : t('永不过期')
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    :title="$t('分享「{0}」', [workbookName])"
    width="640px"
    append-to-body
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="mb-3 text-xs text-gray-500 leading-5">
      {{ $t('拿到链接的人不用登录，就能只读查看这个工作簿，以及它用到的数据集（图表、透视、公式的数据）。看不到别的工作簿和数据集，也不能修改、导出、写 SQL。访客能看到的数据不会超过你自己的数据范围。') }}
      <br>
      {{ $t('访客看到的是已保存的最新版本：以后再修改工作簿，保存后刷新链接就能看到。') }}
    </div>

    <div class="flex items-center gap-2">
      <el-input v-model="form.note" class="min-w-0 flex-1" maxlength="60" :placeholder="$t('备注（可选），如：给某某客户看')" @keyup.enter="create" />
      <el-select v-model="form.days" class="!w-32">
        <el-option v-for="d in DAYS" :key="d.value" :label="d.label" :value="d.value" />
      </el-select>
      <el-button type="primary" :loading="creating" @click="create">
        {{ $t('创建链接') }}
      </el-button>
    </div>

    <div v-if="created" class="created">
      <div class="mb-1 text-[13px] text-[#0B5A2E] font-medium">
        {{ $t('链接已创建。现在复制，关掉后就看不到完整链接了：') }}
      </div>
      <div class="flex items-center gap-2">
        <el-input :model-value="shareUrl(created.token)" readonly @focus="($event.target as HTMLInputElement).select()" />
        <el-button type="primary" @click="copy(shareUrl(created.token))">
          {{ $t('复制') }}
        </el-button>
      </div>
    </div>

    <div class="mb-1 mt-4 text-[13px] text-gray-700 font-medium">
      {{ $t('已有的分享链接') }}
    </div>
    <el-table v-loading="loading" :data="links" size="small" :empty-text="$t('还没有分享链接')">
      <el-table-column :label="$t('备注')" min-width="150" show-overflow-tooltip>
        <template #default="{ row }">
          {{ row.note || $t('（无备注）') }} <span class="text-xs text-gray-400">{{ row.prefix }}…</span>
        </template>
      </el-table-column>
      <el-table-column :label="$t('有效期')" width="150">
        <template #default="{ row }">
          <span :class="row.expired ? 'text-[#B91C1C]' : ''">{{ expiryText(row) }}</span>
        </template>
      </el-table-column>
      <el-table-column :label="$t('访问')" width="120">
        <template #default="{ row }">
          <span class="text-xs text-gray-500">{{ $t('{0} 次', [row.viewCount]) }}<template v-if="row.lastUsedAt"> · {{ row.lastUsedAt.slice(5, 16) }}</template></span>
        </template>
      </el-table-column>
      <el-table-column :label="$t('创建')" width="100" show-overflow-tooltip>
        <template #default="{ row }">
          {{ row.createdByName }}
        </template>
      </el-table-column>
      <el-table-column width="64" align="right">
        <template #default="{ row }">
          <el-button link type="danger" @click="revoke(row)">
            {{ $t('吊销') }}
          </el-button>
        </template>
      </el-table-column>
    </el-table>
  </el-dialog>
</template>

<style scoped lang="scss">
.created {
  @apply mt-3 rounded-md border border-[#c7e3d1] border-solid bg-[#f1f8f3] p-3;
}
</style>
