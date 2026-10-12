<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { SelectionWatcher } from '@/features/excel/data/selection'
import { onUnmounted, ref, watch } from 'vue'
import { watchSelection } from '@/features/excel/data/selection'
import { styleManager } from '@/features/excel/data/style-manager'

const props = defineProps<{
  univerApi: FUniver | null
}>()

// 当前选区信息
const startRow = ref(0)
const startCol = ref(0)
const endRow = ref(0)
const endCol = ref(0)
const hasSelection = ref(false)

// 样式状态
const fontSize = ref(11)
const fontFamily = ref('Microsoft YaHei')
const fontColor = ref('#000000')
const bgColor = ref('#FFFFFF')
const fontWeight = ref<'normal' | 'bold'>('normal')
const hAlign = ref<'left' | 'center' | 'right'>('left')
const vAlign = ref<'top' | 'middle' | 'bottom'>('middle')

// 预设颜色
const presetColors = [
  '#FFFFFF',
  '#F3F4F6',
  '#FEE2E2',
  '#FED7AA',
  '#FEF3C7',
  '#D1FAE5',
  '#DBEAFE',
  '#E9D5FF',
  '#000000',
  '#374151',
  '#DC2626',
  '#EA580C',
  '#D97706',
  '#16A34A',
  '#2563EB',
  '#7C3AED',
]

function updateSelection() {
  if (!props.univerApi)
    return

  const workbook = props.univerApi.getActiveWorkbook()
  if (!workbook)
    return

  const range = workbook.getActiveRange()
  if (!range) {
    hasSelection.value = false
    return
  }

  const rangeData = range.getRange()
  startRow.value = rangeData.startRow
  startCol.value = rangeData.startColumn
  endRow.value = rangeData.endRow
  endCol.value = rangeData.endColumn
  hasSelection.value = true

  // 读取当前样式
  const style = styleManager.getCellStyle(startRow.value, startCol.value)
  if (style) {
    fontSize.value = style.fontSize ?? 11
    fontFamily.value = style.fontFamily ?? 'Microsoft YaHei'
    fontColor.value = style.fontColor ?? '#000000'
    bgColor.value = style.color ?? '#FFFFFF'
    fontWeight.value = style.fontWeight ?? 'normal'
    hAlign.value = style.horizontal ?? 'left'
    vAlign.value = style.vertical ?? 'middle'
  }
}

function applyFontSize() {
  styleManager.setFontSize(startRow.value, startCol.value, endRow.value, endCol.value, fontSize.value)
}

function applyFontFamily() {
  styleManager.setFontFamily(startRow.value, startCol.value, endRow.value, endCol.value, fontFamily.value)
}

function applyFontColor() {
  styleManager.setFontColor(startRow.value, startCol.value, endRow.value, endCol.value, fontColor.value)
}

function applyBgColor() {
  styleManager.setBackgroundColor(startRow.value, startCol.value, endRow.value, endCol.value, bgColor.value)
}

function toggleBold() {
  fontWeight.value = fontWeight.value === 'bold' ? 'normal' : 'bold'
  styleManager.setFontWeight(startRow.value, startCol.value, endRow.value, endCol.value, fontWeight.value)
}

function applyHAlign() {
  styleManager.setHorizontalAlignment(startRow.value, startCol.value, endRow.value, endCol.value, hAlign.value)
}

function applyVAlign() {
  styleManager.setVerticalAlignment(startRow.value, startCol.value, endRow.value, endCol.value, vAlign.value)
}

function applyPreset(preset: 'header' | 'highlight' | 'warning' | 'success' | 'default') {
  styleManager.applyPreset(startRow.value, startCol.value, endRow.value, endCol.value, preset)
}

const fontFamilies = ['Microsoft YaHei', 'SimSun', 'SimHei', 'KaiTi', 'Arial', 'Times New Roman', 'Courier New']
const fontSizes = [10, 11, 12, 13, 14, 16, 18, 20, 24, 28, 32]

let selectionListener: SelectionWatcher | null = null

watch(() => props.univerApi, (api) => {
  selectionListener?.dispose()
  selectionListener = null
  if (api) {
    styleManager.init(api)
    // 监听选区变化（快照恢复重建工作簿后自动重新绑定）
    selectionListener = watchSelection(api, () => {
      updateSelection()
    })
    updateSelection()
  }
}, { immediate: true })

// 面板通过 v-if 挂载，关闭时必须释放监听，否则每次打开都会叠加一个订阅
onUnmounted(() => {
  selectionListener?.dispose()
})
</script>

<template>
  <div class="style-panel">
    <div v-if="!hasSelection" class="no-selection">
      {{ $t('请先选择单元格或区域') }}
    </div>

    <template v-else>
      <!-- 预设样式 -->
      <div class="panel-section">
        <div class="section-label">
          {{ $t('预设样式') }}
        </div>
        <div class="preset-grid">
          <el-button size="small" @click="applyPreset('header')">
            {{ $t('表头') }}
          </el-button>
          <el-button size="small" @click="applyPreset('highlight')">
            {{ $t('高亮') }}
          </el-button>
          <el-button size="small" type="danger" plain @click="applyPreset('warning')">
            {{ $t('警告') }}
          </el-button>
          <el-button size="small" type="success" plain @click="applyPreset('success')">
            {{ $t('成功') }}
          </el-button>
        </div>
      </div>

      <!-- 字体设置 -->
      <div class="panel-section">
        <div class="section-label">
          {{ $t('字体') }}
        </div>
        <div class="style-row">
          <el-select v-model="fontFamily" size="small" style="width: 130px" @change="applyFontFamily">
            <el-option v-for="f in fontFamilies" :key="f" :label="f" :value="f" />
          </el-select>
          <el-select v-model="fontSize" size="small" style="width: 80px" @change="applyFontSize">
            <el-option v-for="s in fontSizes" :key="s" :label="`${s}px`" :value="s" />
          </el-select>
        </div>
        <div class="style-row">
          <el-button
            size="small"
            :type="fontWeight === 'bold' ? 'primary' : 'default'"
            @click="toggleBold"
          >
            <span style="font-weight: 700">B</span>
          </el-button>
        </div>
      </div>

      <!-- 颜色设置 -->
      <div class="panel-section">
        <div class="section-label">
          {{ $t('颜色') }}
        </div>
        <div class="color-row">
          <span class="color-label">{{ $t('文字') }}</span>
          <el-color-picker v-model="fontColor" size="small" @change="applyFontColor" />
          <span class="color-label">{{ $t('背景') }}</span>
          <el-color-picker v-model="bgColor" size="small" @change="applyBgColor" />
        </div>
        <div class="preset-colors">
          <div
            v-for="color in presetColors"
            :key="color"
            class="color-swatch"
            :style="{ background: color }"
            @click="bgColor = color; applyBgColor()"
          />
        </div>
      </div>

      <!-- 对齐设置 -->
      <div class="panel-section">
        <div class="section-label">
          {{ $t('对齐方式') }}
        </div>
        <div class="align-row">
          <el-button-group>
            <el-button size="small" :type="hAlign === 'left' ? 'primary' : 'default'" @click="hAlign = 'left'; applyHAlign()">
              {{ $t('左') }}
            </el-button>
            <el-button size="small" :type="hAlign === 'center' ? 'primary' : 'default'" @click="hAlign = 'center'; applyHAlign()">
              {{ $t('中') }}
            </el-button>
            <el-button size="small" :type="hAlign === 'right' ? 'primary' : 'default'" @click="hAlign = 'right'; applyHAlign()">
              {{ $t('右') }}
            </el-button>
          </el-button-group>
        </div>
        <div class="align-row">
          <el-button-group>
            <el-button size="small" :type="vAlign === 'top' ? 'primary' : 'default'" @click="vAlign = 'top'; applyVAlign()">
              {{ $t('上') }}
            </el-button>
            <el-button size="small" :type="vAlign === 'middle' ? 'primary' : 'default'" @click="vAlign = 'middle'; applyVAlign()">
              {{ $t('中') }}
            </el-button>
            <el-button size="small" :type="vAlign === 'bottom' ? 'primary' : 'default'" @click="vAlign = 'bottom'; applyVAlign()">
              {{ $t('下') }}
            </el-button>
          </el-button-group>
        </div>
      </div>

      <!-- 选区信息 -->
      <div class="selection-info">
        {{ $t('选区: {0}{1}:{2}{3}', [String.fromCharCode(65 + startCol), startRow + 1, String.fromCharCode(65 + endCol), endRow + 1]) }}
      </div>
    </template>
  </div>
</template>

<style lang="scss" scoped>
.style-panel {
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 4px 0;
}

.no-selection {
  text-align: center;
  color: var(--el-text-color-secondary);
  font-size: 13px;
  padding: 40px 0;
}

.panel-section {
  .section-label {
    font-size: 13px;
    font-weight: 600;
    margin-bottom: 8px;
    color: var(--el-text-color-primary);
  }
}

.preset-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 6px;
}

.style-row {
  display: flex;
  gap: 8px;
  margin-bottom: 6px;
}

.color-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}

.color-label {
  font-size: 12px;
  color: var(--el-text-color-secondary);
  min-width: 28px;
}

.preset-colors {
  display: grid;
  grid-template-columns: repeat(8, 1fr);
  gap: 4px;
}

.color-swatch {
  width: 100%;
  aspect-ratio: 1;
  border-radius: 3px;
  cursor: pointer;
  border: 1px solid var(--el-border-color-lighter);
  transition: transform 0.15s;

  &:hover {
    transform: scale(1.15);
  }
}

.align-row {
  margin-bottom: 6px;
}

.selection-info {
  font-size: 12px;
  color: var(--el-text-color-secondary);
  padding: 6px 10px;
  background: var(--el-fill-color-light);
  border-radius: 4px;
  text-align: center;
}
</style>
