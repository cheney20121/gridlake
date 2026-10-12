<script setup lang="ts">
import type { LedgerShop } from '@/api/ledger'
import { reactive, ref, watch } from 'vue'
import { createLedgerShopApi, getLedgerShopsApi, updateLedgerShopApi } from '@/api/ledger'
import { validateShopName } from '@/features/excel/shop-ledger/ledger-workbook'

const props = defineProps<{
  visible: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 店铺信息变更（改名/关店），父组件需刷新汇总 */
  (e: 'changed'): void
}>()

const shops = ref<LedgerShop[]>([])
const loading = ref(false)
const form = reactive({ name: '', douyinShopId: '', remark: '' })
/** 正在编辑的行：原店铺名 → 编辑中的值 */
const editing = reactive<Record<string, { name: string, douyinShopId: string, remark: string }>>({})

watch(() => props.visible, (visible) => {
  if (visible)
    load()
})

async function load() {
  loading.value = true
  try {
    shops.value = await getLedgerShopsApi()
  }
  finally {
    loading.value = false
  }
}

async function create() {
  const error = validateShopName(form.name)
  if (error) {
    ElMessage.warning(error)
    return
  }
  shops.value = await createLedgerShopApi({ name: form.name.trim(), douyinShopId: form.douyinShopId, remark: form.remark })
  ElMessage.success(`已新建店铺「${form.name.trim()}」`)
  Object.assign(form, { name: '', douyinShopId: '', remark: '' })
}

function startEdit(shop: LedgerShop) {
  editing[shop.name] = { name: shop.name, douyinShopId: shop.douyinShopId ?? '', remark: shop.remark ?? '' }
}

async function save(shop: LedgerShop) {
  const draft = editing[shop.name]
  if (!draft)
    return
  const newName = draft.name.trim()
  if (newName !== shop.name) {
    const error = validateShopName(newName)
    if (error) {
      ElMessage.warning(error)
      return
    }
    try {
      await ElMessageBox.confirm(
        `将「${shop.name}」改名为「${newName}」：该店铺 ${shop.rowCount.toLocaleString()} 条历史明细与导入记录会同步更新，汇总表列名随之变化。`,
        '店铺改名',
        { type: 'warning', confirmButtonText: '确认改名' },
      )
    }
    catch {
      return
    }
  }
  shops.value = await updateLedgerShopApi({ name: shop.name, newName, douyinShopId: draft.douyinShopId, remark: draft.remark })
  delete editing[shop.name]
  ElMessage.success('已保存')
  emit('changed')
}

async function toggleStatus(shop: LedgerShop, active: boolean) {
  shops.value = await updateLedgerShopApi({ name: shop.name, status: active ? 'active' : 'closed' })
  ElMessage.success(active ? `「${shop.name}」已恢复运营` : `「${shop.name}」已标记关店，之后的月份不再提示缺数`)
  emit('changed')
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    title="店铺档案"
    width="1120px"
    top="6vh"
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="mb-3 flex items-center gap-2">
      <el-input v-model="form.name" placeholder="店铺名称" class="!w-56" />
      <el-input v-model="form.douyinShopId" placeholder="抖店店铺 ID（选填）" class="!w-48" />
      <el-input v-model="form.remark" placeholder="备注（选填）" class="!w-56" />
      <el-button type="primary" :disabled="!form.name.trim()" @click="create">
        新建店铺
      </el-button>
      <span class="ml-auto text-xs text-gray-400">导入时选择新店铺名也会自动建档</span>
    </div>

    <el-table v-loading="loading" :data="shops" row-key="name" max-height="60vh">
      <el-table-column label="店铺名称" min-width="170">
        <template #default="{ row }">
          <el-input v-if="editing[row.name]" v-model="editing[row.name]!.name" size="small" />
          <span v-else class="font-medium text-gray-800">{{ row.name }}</span>
        </template>
      </el-table-column>
      <el-table-column label="抖店店铺 ID" width="150">
        <template #default="{ row }">
          <el-input v-if="editing[row.name]" v-model="editing[row.name]!.douyinShopId" size="small" />
          <span v-else class="text-gray-600">{{ row.douyinShopId || '—' }}</span>
        </template>
      </el-table-column>
      <el-table-column label="状态" width="110">
        <template #default="{ row }">
          <el-switch
            :model-value="row.status === 'active'"
            inline-prompt
            active-text="运营"
            inactive-text="关店"
            @change="(v: string | number | boolean) => toggleStatus(row, Boolean(v))"
          />
        </template>
      </el-table-column>
      <el-table-column label="商户主体" min-width="200">
        <template #default="{ row }">
          <div v-for="e in row.entities" :key="e" class="truncate text-xs" :title="e">
            {{ e }}
          </div>
          <span v-if="!row.entities.length" class="text-xs text-gray-400">—</span>
        </template>
      </el-table-column>
      <el-table-column label="数据" width="170">
        <template #default="{ row }">
          <div class="text-xs">
            {{ row.rowCount.toLocaleString() }} 条 · {{ row.productCount }} 个商品
          </div>
          <div class="text-xs text-gray-400">
            {{ row.periods.join('、') || '暂无数据' }}
          </div>
        </template>
      </el-table-column>
      <el-table-column label="备注" min-width="140">
        <template #default="{ row }">
          <el-input v-if="editing[row.name]" v-model="editing[row.name]!.remark" size="small" />
          <span v-else class="text-xs text-gray-600">{{ row.remark || '' }}</span>
        </template>
      </el-table-column>
      <el-table-column label="操作" width="120" align="center">
        <template #default="{ row }">
          <template v-if="editing[row.name]">
            <el-button link type="primary" size="small" @click="save(row)">
              保存
            </el-button>
            <el-button link size="small" @click="delete editing[row.name]">
              取消
            </el-button>
          </template>
          <el-button v-else link type="primary" size="small" @click="startEdit(row)">
            编辑
          </el-button>
        </template>
      </el-table-column>
    </el-table>
  </el-dialog>
</template>
