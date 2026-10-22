<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { FormulaDoc } from '@/features/excel/data/formula-helper'
import { computed, ref, watch } from 'vue'
import { FORMULA_DOCS, formulaHelper } from '@/features/excel/data/formula-helper'
import { t } from '@/i18n'

const props = defineProps<{
  univerApi: FUniver | null
}>()

const searchKeyword = ref('')
const activeCategory = ref<'全部' | '数学' | '统计' | '文本' | '日期' | '逻辑'>('全部')
const selectedFormula = ref<FormulaDoc | null>(null)
const applyResult = ref('')

const categories: Array<'全部' | '数学' | '统计' | '文本' | '日期' | '逻辑'> = ['全部', '数学', '统计', '文本', '日期', '逻辑']

const filteredFormulas = computed(() => {
  let list = FORMULA_DOCS
  if (activeCategory.value !== '全部')
    list = list.filter(f => f.category === activeCategory.value)
  if (searchKeyword.value) {
    const kw = searchKeyword.value.toLowerCase()
    list = list.filter(f =>
      f.name.toLowerCase().includes(kw) || f.description.includes(searchKeyword.value),
    )
  }
  return list
})

function selectFormula(f: FormulaDoc) {
  selectedFormula.value = f
  applyResult.value = ''
}

function insertFormula() {
  if (!selectedFormula.value || !props.univerApi)
    return

  const workbook = props.univerApi.getActiveWorkbook()
  if (!workbook)
    return

  const range = workbook.getActiveRange()
  if (!range) {
    applyResult.value = t('请先选择目标单元格')
    return
  }

  const rangeData = range.getRange()
  formulaHelper.setFormula(rangeData.startRow, rangeData.startColumn, selectedFormula.value.example)
  applyResult.value = t('已在 {0} 插入 {1}', [`${String.fromCharCode(65 + rangeData.startColumn)}${rangeData.startRow + 1}`, selectedFormula.value.name])
}

watch(() => props.univerApi, (api) => {
  if (api)
    formulaHelper.init(api)
}, { immediate: true })
</script>

<template>
  <div class="formula-panel">
    <!-- 搜索 -->
    <el-input
      v-model="searchKeyword"
      size="small"
      :placeholder="$t('搜索公式函数...')"
      clearable
      class="search-input"
    >
      <template #prefix>
        <svg viewBox="0 0 16 16" width="14" height="14" fill="none">
          <circle cx="7" cy="7" r="4" stroke="currentColor" stroke-width="1.3" />
          <path d="M10.5 10.5L13 13" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" />
        </svg>
      </template>
    </el-input>

    <!-- 分类 -->
    <div class="category-tabs">
      <button
        v-for="cat in categories"
        :key="cat"
        class="cat-btn"
        :class="{ active: activeCategory === cat }"
        @click="activeCategory = cat"
      >
        {{ cat }}
      </button>
    </div>

    <!-- 公式列表 -->
    <div class="formula-list">
      <div
        v-for="f in filteredFormulas"
        :key="f.name"
        class="formula-item"
        :class="{ active: selectedFormula?.name === f.name }"
        @click="selectFormula(f)"
      >
        <div class="formula-name">
          {{ f.name }}
        </div>
        <div class="formula-desc">
          {{ f.description }}
        </div>
        <el-tag size="small" round>
          {{ f.category }}
        </el-tag>
      </div>
      <el-empty v-if="filteredFormulas.length === 0" :description="$t('未找到匹配公式')" :image-size="60" />
    </div>

    <!-- 公式详情 -->
    <div v-if="selectedFormula" class="formula-detail">
      <div class="detail-title">
        {{ selectedFormula.name }}
      </div>
      <div class="detail-desc">
        {{ selectedFormula.description }}
      </div>
      <div class="detail-syntax">
        <span class="detail-label">{{ $t('语法') }}</span>
        <code>{{ selectedFormula.syntax }}</code>
      </div>
      <div class="detail-example">
        <span class="detail-label">{{ $t('示例') }}</span>
        <code>{{ selectedFormula.example }}</code>
      </div>
      <el-button type="primary" size="small" @click="insertFormula">
        {{ $t('插入到当前单元格') }}
      </el-button>
      <div v-if="applyResult" class="apply-result">
        {{ applyResult }}
      </div>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.formula-panel {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 4px 0;
}

.search-input {
  width: 100%;
}

.category-tabs {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.cat-btn {
  padding: 3px 10px;
  border: 1px solid var(--el-border-color);
  border-radius: 12px;
  background: var(--el-bg-color);
  font-size: 12px;
  color: var(--el-text-color-secondary);
  cursor: pointer;
  transition: all 0.15s;

  &.active {
    background: var(--el-color-primary);
    color: white;
    border-color: var(--el-color-primary);
  }
}

.formula-list {
  max-height: 280px;
  overflow-y: auto;
  border: 1px solid var(--el-border-color-lighter);
  border-radius: 6px;
  padding: 4px;
}

.formula-item {
  padding: 8px;
  border-radius: 4px;
  cursor: pointer;
  transition: background 0.15s;

  &:hover {
    background: var(--el-fill-color-light);
  }

  &.active {
    background: var(--el-color-primary-light-9);
  }
}

.formula-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--el-text-color-primary);
}

.formula-desc {
  font-size: 11px;
  color: var(--el-text-color-secondary);
  margin: 2px 0 4px;
}

.formula-detail {
  border: 1px solid var(--el-border-color-lighter);
  border-radius: 6px;
  padding: 10px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.detail-title {
  font-size: 14px;
  font-weight: 700;
  color: var(--el-color-primary);
}

.detail-desc {
  font-size: 12px;
  color: var(--el-text-color-secondary);
}

.detail-syntax,
.detail-example {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.detail-label {
  font-size: 11px;
  color: var(--el-text-color-placeholder);
}

.detail-syntax code,
.detail-example code {
  font-size: 12px;
  background: var(--el-fill-color-light);
  padding: 4px 6px;
  border-radius: 3px;
  color: var(--el-text-color-primary);
}

.apply-result {
  font-size: 12px;
  color: var(--el-color-success);
}
</style>
