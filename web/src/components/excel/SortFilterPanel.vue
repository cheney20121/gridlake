<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { FWorksheet } from '@univerjs/sheets/facade'
import { computed, ref, watch } from 'vue'
import { sortFilterEngine } from '@/features/excel/data/sort-filter'
import { t } from '@/i18n'
import { useExcelUIStore } from '@/stores/excel-ui'

const props = defineProps<{
  univerApi: FUniver | null
}>()

const uiStore = useExcelUIStore()

const selectedColumn = ref(0)
const sortOrder = ref<'asc' | 'desc'>('asc')
const uniqueValues = ref<{ value: unknown, count: number }[]>([])
const selectedValues = ref<Set<string>>(new Set())
const filterEnabled = ref(false)
const resultMessage = ref('')

const columnLabels = computed(() => {
  const sheet = getActiveSheet()
  if (!sheet)
    return []

  const colCount = sheet.getLastColumn() + 1
  const labels: { index: number, label: string }[] = []
  for (let c = 0; c < Math.min(colCount, 20); c++) {
    const headerCell = sheet.getRange(0, c)
    labels.push({ index: c, label: String(headerCell.getValue() ?? `列${c + 1}`) })
  }
  return labels
})

function getActiveSheet(): FWorksheet | null {
  const workbook = props.univerApi?.getActiveWorkbook()
  if (!workbook)
    return null
  return workbook.getActiveSheet()
}

function loadUniqueValues() {
  const sheet = getActiveSheet()
  if (!sheet)
    return

  const rowCount = sheet.getLastRow() + 1
  uniqueValues.value = sortFilterEngine.getUniqueValues(selectedColumn.value, 1, rowCount - 1)
  selectedValues.value = new Set(uniqueValues.value.map(v => String(v.value)))
  filterEnabled.value = false
}

function handleSort() {
  const sheet = getActiveSheet()
  if (!sheet)
    return

  const rowCount = sheet.getLastRow() + 1
  const colCount = sheet.getLastColumn() + 1
  const result = sortFilterEngine.sort(1, rowCount - 1, 0, colCount - 1, [{
    column: selectedColumn.value,
    order: sortOrder.value,
  }])

  resultMessage.value = result.message
  uiStore.setSortConfig({ column: selectedColumn.value, order: sortOrder.value })
}

function handleApplyFilter() {
  const sheet = getActiveSheet()
  if (!sheet)
    return

  const rowCount = sheet.getLastRow() + 1

  if (!filterEnabled.value || selectedValues.value.size === uniqueValues.value.length) {
    // 全选 = 清除筛选
    const result = sortFilterEngine.clearFilter(1, rowCount - 1)
    resultMessage.value = result.message
    uiStore.setFilterConfig(null)
  }
  else {
    const values = Array.from(selectedValues.value)
    const result = sortFilterEngine.filter(selectedColumn.value, values, 1, rowCount - 1)
    resultMessage.value = result.message
    uiStore.setFilterConfig({ column: selectedColumn.value, values })
  }
}

function handleClearFilter() {
  const sheet = getActiveSheet()
  if (!sheet)
    return

  const rowCount = sheet.getLastRow() + 1
  sortFilterEngine.clearFilter(1, rowCount - 1)
  filterEnabled.value = false
  selectedValues.value = new Set(uniqueValues.value.map(v => String(v.value)))
  resultMessage.value = t('已清除筛选')
  uiStore.setFilterConfig(null)
}

function toggleValue(value: string) {
  if (selectedValues.value.has(value))
    selectedValues.value.delete(value)
  else
    selectedValues.value.add(value)
}

function selectAll() {
  selectedValues.value = new Set(uniqueValues.value.map(v => String(v.value)))
}

function selectNone() {
  selectedValues.value.clear()
}

watch(selectedColumn, () => {
  loadUniqueValues()
  resultMessage.value = ''
})

// 初始化
watch(() => props.univerApi, (api) => {
  if (api) {
    sortFilterEngine.init(api)
    loadUniqueValues()
  }
}, { immediate: true })
</script>

<template>
  <div class="sort-filter-panel">
    <div class="panel-section">
      <div class="section-label">
        {{ $t('排序列') }}
      </div>
      <el-select v-model="selectedColumn" class="full-width" :placeholder="$t('选择列')">
        <el-option
          v-for="col in columnLabels"
          :key="col.index"
          :label="col.label"
          :value="col.index"
        />
      </el-select>
    </div>

    <div class="panel-section">
      <div class="section-label">
        {{ $t('排序方向') }}
      </div>
      <el-radio-group v-model="sortOrder">
        <el-radio-button value="asc">
          {{ $t('升序 ↑') }}
        </el-radio-button>
        <el-radio-button value="desc">
          {{ $t('降序 ↓') }}
        </el-radio-button>
      </el-radio-group>
    </div>

    <el-button type="primary" @click="handleSort">
      <svg viewBox="0 0 16 16" width="14" height="14" fill="none">
        <path d="M3 4l4 4M3 12l4-4M9 4h4M9 8h4M9 12h2" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" />
      </svg>
      {{ $t('执行排序') }}
    </el-button>

    <el-divider />

    <div class="panel-section">
      <div class="section-label">
        <span>{{ $t('列筛选') }}</span>
        <el-switch v-model="filterEnabled" size="small" @change="handleApplyFilter" />
      </div>

      <div v-if="uniqueValues.length > 0" class="filter-controls">
        <el-button size="small" text @click="selectAll">
          {{ $t('全选') }}
        </el-button>
        <el-button size="small" text @click="selectNone">
          {{ $t('清空') }}
        </el-button>
      </div>

      <div class="filter-list">
        <div
          v-for="item in uniqueValues.slice(0, 50)"
          :key="String(item.value)"
          class="filter-item"
          @click="toggleValue(String(item.value)); handleApplyFilter()"
        >
          <el-checkbox
            :model-value="selectedValues.has(String(item.value))"
            size="small"
            @click.stop
            @change="toggleValue(String(item.value)); handleApplyFilter()"
          />
          <span class="filter-value">{{ item.value }}</span>
          <span class="filter-count">{{ item.count }}</span>
        </div>
      </div>

      <el-button size="small" @click="handleClearFilter">
        {{ $t('清除筛选') }}
      </el-button>
    </div>

    <div v-if="resultMessage" class="result-msg">
      {{ resultMessage }}
    </div>
  </div>
</template>

<style lang="scss" scoped>
.sort-filter-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 4px 0;
}

.panel-section {
  .section-label {
    font-size: 13px;
    font-weight: 600;
    margin-bottom: 8px;
    color: var(--el-text-color-primary);
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
}

.full-width {
  width: 100%;
}

.filter-controls {
  display: flex;
  gap: 4px;
  margin-bottom: 8px;
}

.filter-list {
  max-height: 240px;
  overflow-y: auto;
  border: 1px solid var(--el-border-color-lighter);
  border-radius: 6px;
  padding: 4px;
  margin-bottom: 8px;
}

.filter-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 8px;
  cursor: pointer;
  border-radius: 4px;
  transition: background 0.15s;

  &:hover {
    background: var(--el-fill-color-light);
  }
}

.filter-value {
  flex: 1;
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.filter-count {
  font-size: 11px;
  color: var(--el-text-color-secondary);
  background: var(--el-fill-color);
  padding: 1px 6px;
  border-radius: 8px;
}

.result-msg {
  font-size: 12px;
  color: var(--el-color-success);
  padding: 8px;
  background: var(--el-color-success-light-9);
  border-radius: 4px;
}
</style>
