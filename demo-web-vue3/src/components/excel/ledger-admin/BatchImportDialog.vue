<script setup lang="ts">
import type { UploadFile } from 'element-plus'
import type { LedgerImportMode, LedgerImportPreview, LedgerImportResult, LedgerShop, ShopCandidate } from '@/api/ledger'
import { isAxiosError } from 'axios'
import { computed, reactive, ref, watch } from 'vue'
import { commitLedgerImportApi, getLedgerShopsApi, previewLedgerImportApi } from '@/api/ledger'
import { validateShopName } from '@/features/excel/shop-ledger/ledger-workbook'

const props = defineProps<{
  visible: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  /** 导入完成，携带涉及的月份 */
  (e: 'imported', periods: string[]): void
}>()

type Status = 'uploading' | 'ready' | 'error' | 'importing' | 'done' | 'failed'

interface ImportItem {
  uid: number
  file: File
  status: Status
  progress: number
  preview?: LedgerImportPreview
  error?: string
  shop: string
  mode: LedgerImportMode
  confirmed: boolean
  result?: LedgerImportResult
}

interface Check {
  level: 'danger' | 'warning' | 'info'
  text: string
}

const PREVIEW_CONCURRENCY = 2

const items = ref<ImportItem[]>([])
const shops = ref<LedgerShop[]>([])
const importing = ref(false)
let uid = 0
let previewing = 0

const shopMap = computed(() => new Map(shops.value.map(s => [s.name, s])))

watch(() => props.visible, (visible) => {
  if (visible) {
    items.value = []
    loadShops()
  }
})

async function loadShops() {
  shops.value = await getLedgerShopsApi()
}

function errorMessage(e: unknown): string {
  if (isAxiosError(e))
    return (e.response?.data as { msg?: string } | undefined)?.msg ?? (e.response ? e.message : '无法连接资金流水服务')
  return e instanceof Error ? e.message : String(e)
}

// ── 添加文件与预览 ────────────────────────────────────
function onFileChange(upload: UploadFile) {
  const file = upload.raw
  if (!file)
    return
  if (!/\.csv$/i.test(file.name)) {
    ElMessage.warning(`「${file.name}」不是 CSV 文件，已忽略`)
    return
  }
  if (items.value.some(i => i.file.name === file.name && i.file.size === file.size)) {
    ElMessage.info(`「${file.name}」已在列表中`)
    return
  }
  items.value.push(reactive<ImportItem>({ uid: ++uid, file, status: 'uploading', progress: 0, shop: '', mode: 'merge', confirmed: false }))
  pumpPreview()
}

/** 同时最多预览 PREVIEW_CONCURRENCY 个文件 */
function pumpPreview() {
  while (previewing < PREVIEW_CONCURRENCY) {
    const next = items.value.find(i => i.status === 'uploading' && i.progress === 0)
    if (!next)
      return
    next.progress = 1
    previewing++
    void runPreview(next).finally(() => {
      previewing--
      pumpPreview()
    })
  }
}

async function runPreview(item: ImportItem) {
  try {
    item.preview = await previewLedgerImportApi(item.file, (e) => {
      item.progress = e.total ? Math.max(1, Math.round((e.loaded / e.total) * 100)) : 1
    })
    item.shop = item.preview.suggestedShop ?? ''
    item.status = 'ready'
  }
  catch (e) {
    item.status = 'error'
    item.error = errorMessage(e)
  }
}

function remove(item: ImportItem) {
  items.value = items.value.filter(i => i !== item)
}

// ── 检查 ──────────────────────────────────────────────
const percent = (v: number) => `${Math.round(v * 100)}%`
const dupRatio = (p: LedgerImportPreview, c: ShopCandidate) => c.duplicateRows / Math.max(p.rowCount, 1)
const isStrong = (p: LedgerImportPreview, c: ShopCandidate) => dupRatio(p, c) >= 0.5 || c.productRatio >= 0.5

/**
 * 匹配度相近、无法确定归属的候选店铺
 * 同品牌多店常卖相同商品，商品重合度高很正常；只要流水重合（强证据）已明确指向一家店就不算相近
 */
function ambiguousCandidates(p: LedgerImportPreview): ShopCandidate[] {
  const byDup = p.candidates.filter(c => dupRatio(p, c) >= 0.5)
  if (byDup.length)
    return byDup
  const top = p.candidates[0]
  if (!top || top.productRatio < 0.5)
    return []
  return p.candidates.filter(c => top.productRatio - c.productRatio < 0.15)
}

function checks(item: ImportItem): Check[] {
  const p = item.preview
  if (!p)
    return []
  const out: Check[] = []
  const top = p.candidates[0]
  const chosen = p.candidates.find(c => c.shop === item.shop)
  const shop = shopMap.value.get(item.shop)
  const nameError = item.shop ? validateShopName(item.shop) : null

  if (!item.shop)
    out.push({ level: 'warning', text: '请选择或输入店铺' })
  if (nameError)
    out.push({ level: 'danger', text: nameError })
  if (item.shop && top && isStrong(p, top) && top.shop !== item.shop) {
    out.push({
      level: 'danger',
      text: `疑似串店：文件与「${top.shop}」更匹配（流水重合 ${percent(dupRatio(p, top))}，商品重合 ${percent(top.productRatio)}）`,
    })
  }
  const close = ambiguousCandidates(p)
  if (close.length > 1 && top?.shop === item.shop)
    out.push({ level: 'warning', text: `多个店铺匹配度相近：${close.map(c => c.shop).join('、')}，请确认` })
  const frozen = frozenPeriods(item)
  if (frozen.length)
    out.push({ level: 'danger', text: `「${item.shop}」${frozen.join('、')} 已锁账或审批中，不能导入（需先在月结中心反结账 / 撤回）` })
  if (item.shop && !shop && !nameError)
    out.push({ level: 'info', text: `将新建店铺「${item.shop}」` })
  if (shop?.status === 'closed')
    out.push({ level: 'warning', text: `「${item.shop}」已标记为关店` })
  if (shop && shop.entities.length) {
    const fresh = p.entities.filter(e => e.name && !shop.entities.includes(e.name))
    if (fresh.length)
      out.push({ level: 'warning', text: `出现该店铺以前没有的商户主体：${fresh.map(e => e.name).join('、')}（主体变更或文件导错？）` })
  }
  if (p.sameFile)
    out.push({ level: 'info', text: `此文件已于 ${p.sameFile.importedAt} 导入到「${p.sameFile.shop}」` })
  if (item.mode === 'merge' && chosen?.duplicateRows)
    out.push({ level: 'info', text: `${chosen.duplicateRows.toLocaleString()} 条流水已存在，将自动跳过` })
  if (item.mode === 'replace' && shop)
    out.push({ level: 'warning', text: `将先删除「${item.shop}」在 ${p.timeFrom} ~ ${p.timeTo} 的已有数据再写入` })
  if (p.duplicateInFile)
    out.push({ level: 'info', text: `文件内有 ${p.duplicateInFile} 条重复流水，只保留一条` })
  if (p.missingColumns.length)
    out.push({ level: 'warning', text: `缺少列：${p.missingColumns.join('、')}（相关汇总项按 0 计算）` })
  for (const n of p.invalidNumbers)
    out.push({ level: 'warning', text: `「${n.column}」有 ${n.count} 个无法识别的金额（如「${n.sample}」），按空值入库` })
  return out
}

/** 文件涉及的、所选店铺已冻结的月份 */
function frozenPeriods(item: ImportItem) {
  return (item.preview?.locks ?? []).filter(l => l.shop === item.shop).map(l => l.period)
}

function needsConfirm(item: ImportItem) {
  return checks(item).some(c => c.level === 'danger' && c.text.startsWith('疑似串店'))
}

function canImport(item: ImportItem) {
  return item.status === 'ready' && !!item.shop && !validateShopName(item.shop) && !frozenPeriods(item).length
    && (!needsConfirm(item) || item.confirmed)
}

const importable = computed(() => items.value.filter(canImport))
const pendingPreview = computed(() => items.value.some(i => i.status === 'uploading'))

/** 店铺下拉：已有店铺按匹配度排序，附带重合度 */
function shopOptions(item: ImportItem) {
  const p = item.preview
  const rank = new Map((p?.candidates ?? []).map((c, i) => [c.shop, i]))
  return [...shops.value]
    .sort((a, b) => (rank.get(a.name) ?? 999) - (rank.get(b.name) ?? 999))
    .map((s) => {
      const c = p?.candidates.find(x => x.shop === s.name)
      return {
        value: s.name,
        hint: c && p ? `流水 ${percent(dupRatio(p, c))} · 商品 ${percent(c.productRatio)}` : s.status === 'closed' ? '已关店' : '',
      }
    })
}

// ── 提交 ──────────────────────────────────────────────
async function importAll() {
  importing.value = true
  const periods = new Set<string>()
  try {
    for (const item of importable.value) {
      item.status = 'importing'
      try {
        item.result = await commitLedgerImportApi(item.preview!.token, item.shop.trim(), item.mode, item.file.name)
        item.result.periods.forEach(p => periods.add(p))
        item.status = 'done'
      }
      catch (e) {
        item.status = 'failed'
        item.error = errorMessage(e)
      }
    }
  }
  finally {
    importing.value = false
  }
  await loadShops()
  const done = items.value.filter(i => i.status === 'done')
  if (done.length) {
    const inserted = done.reduce((s, i) => s + (i.result?.insertedRows ?? 0), 0)
    ElMessage.success(`已导入 ${done.length} 个文件，新增 ${inserted.toLocaleString()} 条流水`)
    emit('imported', [...periods].sort())
  }
}

function formatSize(bytes: number) {
  return bytes > 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.ceil(bytes / 1024)} KB`
}

const levelClass = { danger: 'check-danger', warning: 'check-warning', info: 'check-info' }
</script>

<template>
  <el-dialog
    :model-value="visible"
    title="批量导入店铺资金流水"
    width="1240px"
    top="5vh"
    :close-on-click-modal="false"
    @update:model-value="emit('update:visible', $event)"
  >
    <el-upload
      drag
      multiple
      accept=".csv"
      :auto-upload="false"
      :show-file-list="false"
      :on-change="onFileChange"
      class="import-drop"
      :class="{ compact: items.length > 0 }"
    >
      <div class="text-sm text-gray-600">
        拖入抖店「资金流水」CSV，或<em class="text-[#107C41] not-italic">点击选择</em>（可一次选择多个店铺、多个月份的文件）
      </div>
      <div v-if="!items.length" class="mt-1 text-xs text-gray-400">
        系统会根据商品与流水自动识别所属店铺；同一笔流水重复导入会自动跳过
      </div>
    </el-upload>

    <el-table v-if="items.length" :data="items" row-key="uid" class="mt-3" max-height="56vh">
      <el-table-column label="文件" min-width="230">
        <template #default="{ row }">
          <div class="truncate font-medium text-gray-800" :title="row.file.name">
            {{ row.file.name }}
          </div>
          <div class="text-xs text-gray-400">
            {{ formatSize(row.file.size) }}
            <template v-if="row.preview">
              · {{ row.preview.rowCount.toLocaleString() }} 行 · {{ row.preview.periods.join('、') }}
            </template>
          </div>
          <div v-if="row.preview" class="text-xs text-gray-400">
            {{ row.preview.timeFrom.slice(5, 16) }} ~ {{ row.preview.timeTo.slice(5, 16) }}
          </div>
        </template>
      </el-table-column>

      <el-table-column label="商户主体" min-width="170">
        <template #default="{ row }">
          <div v-for="e in row.preview?.entities ?? []" :key="e.name" class="truncate text-xs" :title="e.name">
            {{ e.name }} <span class="text-gray-400">{{ e.rowCount.toLocaleString() }}</span>
          </div>
        </template>
      </el-table-column>

      <el-table-column label="导入到店铺" width="230">
        <template #default="{ row }">
          <el-select
            v-model="row.shop"
            filterable
            allow-create
            default-first-option
            placeholder="选择或输入新店铺"
            :disabled="row.status !== 'ready'"
            class="w-full"
          >
            <el-option v-for="o in shopOptions(row)" :key="o.value" :label="o.value" :value="o.value">
              <span>{{ o.value }}</span>
              <span class="float-right text-xs text-gray-400">{{ o.hint }}</span>
            </el-option>
          </el-select>
        </template>
      </el-table-column>

      <el-table-column label="方式" width="118">
        <template #default="{ row }">
          <el-select v-model="row.mode" :disabled="row.status !== 'ready'" size="small">
            <el-option label="合并去重" value="merge" />
            <el-option label="替换时间段" value="replace" />
          </el-select>
        </template>
      </el-table-column>

      <el-table-column label="检查结果" min-width="300">
        <template #default="{ row }">
          <div v-for="(c, i) in checks(row)" :key="i" class="check" :class="levelClass[c.level]">
            {{ c.text }}
          </div>
          <el-checkbox v-if="row.status === 'ready' && needsConfirm(row)" v-model="row.confirmed" size="small" class="mt-1">
            我已核对，确认导入到「{{ row.shop }}」
          </el-checkbox>
        </template>
      </el-table-column>

      <el-table-column label="状态" width="170">
        <template #default="{ row }">
          <el-progress v-if="row.status === 'uploading'" :percentage="row.progress" :stroke-width="6" />
          <span v-else-if="row.status === 'ready'" class="text-xs text-gray-500">待导入</span>
          <span v-else-if="row.status === 'importing'" class="text-xs text-[#107C41]">导入中…</span>
          <div v-else-if="row.status === 'done' && row.result" class="text-xs leading-5">
            <div class="text-[#107C41]">
              ✓ 新增 {{ row.result.insertedRows.toLocaleString() }}
            </div>
            <div v-if="row.result.skippedRows" class="text-gray-500">
              跳过 {{ row.result.skippedRows.toLocaleString() }}
            </div>
            <div v-if="row.result.replacedRows" class="text-gray-500">
              替换删除 {{ row.result.replacedRows.toLocaleString() }}
            </div>
            <el-popover v-if="row.result.changedRows" trigger="click" :width="420">
              <template #reference>
                <button class="text-[#D97706] underline">
                  变更 {{ row.result.changedRows }} 条（未覆盖）
                </button>
              </template>
              <div class="mb-1 text-xs text-gray-500">
                流水号已存在但内容不同。如确认平台已修正数据，请用「替换时间段」重新导入。
              </div>
              <div v-for="c in row.result.changedSamples" :key="c.id" class="border-0 border-t border-solid border-[#f0f0f0] py-1 text-xs">
                <div class="text-gray-500">
                  {{ c.time }} · {{ c.id }}
                </div>
                <div>原：{{ c.before }}</div>
                <div class="text-[#D97706]">
                  新：{{ c.after }}
                </div>
              </div>
            </el-popover>
          </div>
          <span v-else class="text-xs text-[#DC2626]">{{ row.error }}</span>
        </template>
      </el-table-column>

      <el-table-column width="64" align="center">
        <template #default="{ row }">
          <el-button v-if="row.status !== 'importing'" link size="small" @click="remove(row)">
            移除
          </el-button>
        </template>
      </el-table-column>
    </el-table>

    <template #footer>
      <div class="flex items-center justify-between">
        <span class="text-xs text-gray-500">
          <template v-if="items.length">
            共 {{ items.length }} 个文件，可导入 {{ importable.length }} 个
            <template v-if="pendingPreview">（预览中…）</template>
          </template>
        </span>
        <span>
          <el-button @click="emit('update:visible', false)">关闭</el-button>
          <el-button type="primary" :loading="importing" :disabled="!importable.length" @click="importAll">
            开始导入（{{ importable.length }}）
          </el-button>
        </span>
      </div>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.import-drop {
  :deep(.#{$namespace}-upload-dragger) {
    @apply py-8;
  }

  &.compact :deep(.#{$namespace}-upload-dragger) {
    @apply py-3;
  }
}

.check {
  @apply mb-0.5 rounded px-1.5 py-0.5 text-xs leading-5;
}

.check-danger {
  @apply bg-[#FEE2E2] text-[#B91C1C] font-medium;
}

.check-warning {
  @apply bg-[#FEF3C7] text-[#92400E];
}

.check-info {
  @apply bg-[#F1F5F9] text-gray-600;
}
</style>
