<script setup lang="ts">
import type { WorkbookMeta } from '@/api/workbook'
import type { Visibility } from '@/stores/ledger-user'
import { ref } from 'vue'
import { shareWorkbookApi } from '@/api/workbook'
import { t } from '@/i18n'
import { useLedgerUserStore, VISIBILITY_LABEL } from '@/stores/ledger-user'

/**
 * 工作簿的共享范围：所有者与管理员可以修改，其他人只看到当前范围。
 * 没有开启登录校验时不显示（不区分用户）
 */
const props = defineProps<{
  workbook: WorkbookMeta
}>()

const emit = defineEmits<{
  (e: 'changed', meta: WorkbookMeta): void
}>()

const store = useLedgerUserStore()
const open = ref(false)
const saving = ref(false)

const OPTIONS: { value: Visibility, hint: string }[] = [
  { value: 'private', hint: t('只有你和管理员能看到') },
  { value: 'view', hint: t('同事可以打开查看，不能修改') },
  { value: 'edit', hint: t('同事可以查看和修改，删除与共享设置仍归你') },
]

async function choose(v: Visibility) {
  if (v === props.workbook.visibility) {
    open.value = false
    return
  }
  saving.value = true
  try {
    const meta = await shareWorkbookApi(props.workbook.id, v)
    emit('changed', meta)
    ElMessage.success(t('已设为「{0}」', [VISIBILITY_LABEL[v]]))
    open.value = false
  }
  finally {
    saving.value = false
  }
}
</script>

<template>
  <template v-if="store.authRequired">
    <el-popover v-if="store.accessLevel(workbook) >= 3" v-model:visible="open" trigger="click" :width="260" placement="bottom-start">
      <template #reference>
        <button type="button" class="share-chip cursor-pointer" :class="{ 'is-shared': workbook.visibility !== 'private' }" @click.stop>
          {{ VISIBILITY_LABEL[workbook.visibility] }} ▾
        </button>
      </template>
      <div v-loading="saving" class="flex flex-col gap-1" @click.stop>
        <button
          v-for="o in OPTIONS"
          :key="o.value"
          type="button"
          class="option"
          :class="{ on: workbook.visibility === o.value }"
          @click="choose(o.value)"
        >
          <span class="text-[13px] text-gray-800">{{ VISIBILITY_LABEL[o.value] }}</span>
          <span class="text-xs text-gray-400">{{ o.hint }}</span>
        </button>
      </div>
    </el-popover>
    <span v-else class="share-chip" :title="$t('所有者：{0}', [workbook.ownerName || '无（仅管理员可管理）'])">{{ VISIBILITY_LABEL[workbook.visibility] }}</span>
  </template>
</template>

<style scoped lang="scss">
.share-chip {
  @apply inline-block rounded border border-solid border-[#e0e0e0] bg-white px-2 py-0.5 text-xs text-gray-500;

  &.is-shared {
    @apply border-[#b7e0c8] bg-[#f0faf4] text-[#0E5A31];
  }
}

.option {
  @apply flex flex-col gap-0.5 rounded border border-solid border-transparent bg-transparent px-2 py-1.5 text-left cursor-pointer;

  &:hover {
    @apply bg-[#f5f7f6];
  }

  &.on {
    @apply border-[#107C41] bg-[#f0faf4];
  }
}
</style>
