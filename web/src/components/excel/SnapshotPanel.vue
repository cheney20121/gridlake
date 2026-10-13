<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { WorkbookVersion } from '@/api/workbook'
import { onBeforeUnmount, ref } from 'vue'
import { listWorkbookVersionsApi, restoreWorkbookVersionApi } from '@/api/workbook'
import { persistenceManager } from '@/features/excel/persistence/manager'
import { openServerWorkbook } from '@/features/excel/persistence/workbook-session'
import { useLedgerOperator } from '@/features/excel/workspace/use-operator'
import { t } from '@/i18n'

/** 版本历史（服务端）：每次保存都是一个版本；恢复会以旧版本内容生成新版本，历史不丢 */
const props = defineProps<{
  univerApi: FUniver | null
}>()

const operator = useLedgerOperator()
const versions = ref<WorkbookVersion[]>([])
const loading = ref(false)
const busy = ref(false)

async function refresh() {
  const meta = persistenceManager.getMeta()
  if (!meta) {
    versions.value = []
    return
  }
  loading.value = true
  try {
    versions.value = await listWorkbookVersionsApi(meta.id)
  }
  finally {
    loading.value = false
  }
}

// 每次保存后刷新列表
const stop = persistenceManager.onChange((state) => {
  if (state === 'saved')
    refresh()
})
onBeforeUnmount(stop)
refresh()

async function saveNamedVersion() {
  const description = await ElMessageBox.prompt(t('给这个版本起个名字，方便以后找回（如「9 月月结定稿」）'), t('保存版本'), {
    inputValidator: v => !!v.trim() || t('请填写版本说明'),
    confirmButtonText: t('保存'),
  }).then(r => r.value.trim(), () => null)
  if (!description)
    return
  busy.value = true
  try {
    const r = await persistenceManager.save({ description })
    if (r.state === 'saved')
      ElMessage.success(t('已保存版本'))
  }
  finally {
    busy.value = false
  }
}

async function restore(v: WorkbookVersion) {
  const meta = persistenceManager.getMeta()
  if (!meta || !props.univerApi)
    return
  const unsaved = persistenceManager.isDirty() ? '当前还有未保存的修改，恢复后将丢失。' : ''
  const ok = await ElMessageBox.confirm(
    t('{0}恢复到第 {1} 版（{2} {3}）？会以该版本内容生成一个新版本，现有版本仍保留在历史中。', [unsaved, v.version, v.savedAt, v.savedBy ?? '']),
    t('恢复版本'),
    { type: 'warning', confirmButtonText: t('恢复') },
  ).then(() => true, () => false)
  if (!ok)
    return
  busy.value = true
  try {
    const restored = await restoreWorkbookVersionApi(meta.id, v.version, operator.value)
    await openServerWorkbook(props.univerApi, restored)
    ElMessage.success(t('已恢复到第 {0} 版（新版本号 {1}）', [v.version, restored.version]))
    await refresh()
  }
  finally {
    busy.value = false
  }
}

function sizeText(n: number) {
  return n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1e3))} KB`
}
</script>

<template>
  <div class="snapshot-panel">
    <el-button type="primary" :loading="busy" @click="saveNamedVersion">
      {{ $t('保存为命名版本') }}
    </el-button>
    <div class="text-xs text-gray-400 leading-5">
      {{ $t('修改后自动保存到服务端。命名版本与手动保存永久保留，自动保存保留最近 30 个') }}
    </div>

    <div v-loading="loading" class="snapshot-list">
      <el-empty v-if="!versions.length" :description="$t('暂无版本记录')" :image-size="60" />
      <div v-for="v in versions" :key="v.version" class="snapshot-item">
        <div class="snap-info">
          <div class="snap-desc">
            {{ v.description || $t('保存') }}
          </div>
          <div class="snap-meta">
            <span>{{ v.savedAt.slice(5, 16) }}</span>
            <span>{{ v.savedBy }}</span>
            <span>v{{ v.version }}</span>
            <span>{{ sizeText(v.size) }}</span>
            <el-tag size="small" :type="v.isAuto ? 'info' : 'success'" round>
              {{ v.isAuto ? $t('自动') : $t('保留') }}
            </el-tag>
          </div>
        </div>
        <div class="snap-actions">
          <el-button size="small" :disabled="busy || v.version === versions[0]?.version" @click="restore(v)">
            {{ $t('恢复') }}
          </el-button>
        </div>
      </div>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.snapshot-panel {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 4px 0;
}

.snapshot-list {
  max-height: 460px;
  min-height: 80px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.snapshot-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px;
  border: 1px solid var(--#{$namespace}-border-color-lighter);
  border-radius: 6px;
  transition: box-shadow 0.15s;

  &:hover {
    box-shadow: 0 1px 3px rgba(15, 23, 42, 0.06);
  }
}

.snap-info {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.snap-desc {
  font-size: 13px;
  font-weight: 600;
  color: var(--#{$namespace}-text-color-primary);
}

.snap-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 11px;
  color: var(--#{$namespace}-text-color-secondary);
}

.snap-actions {
  display: flex;
  gap: 4px;
}
</style>
