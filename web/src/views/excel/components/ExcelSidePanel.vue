<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { DatasetFilter, DatasetInfo } from '@/api/dataset'
import AiAssistantPanel from '@/components/excel/ai/AiAssistantPanel.vue'
import ChartPanel from '@/components/excel/chart/ChartPanel.vue'
import CollaborationPanel from '@/components/excel/CollaborationPanel.vue'
import CommentPanel from '@/components/excel/comments/CommentPanel.vue'
import DashboardPanel from '@/components/excel/dashboard/DashboardPanel.vue'
import FormulaPanel from '@/components/excel/FormulaPanel.vue'
import PivotPanel from '@/components/excel/pivot/PivotPanel.vue'
import SnapshotPanel from '@/components/excel/SnapshotPanel.vue'
import SortFilterPanel from '@/components/excel/SortFilterPanel.vue'
import StylePanel from '@/components/excel/StylePanel.vue'
import WritebackPanel from '@/components/excel/writeback/WritebackPanel.vue'
import { persistenceManager } from '@/features/excel/persistence/manager'
import { useExcelUIStore } from '@/stores/excel-ui'

/** 编辑器右侧的浮层面板：按 uiStore.activePanel 切换 AI 助手、透视表字段、图表、仪表盘、填报等 */
defineProps<{
  univerApi: FUniver | null
  datasets: DatasetInfo[]
  workbookId: string | null
  operator: string
  /** AI 对话链接（?ai=）指定的对话：AI 面板首次打开时选中 */
  aiLinkThread: string | null
  /** 图表面板用：从数据表「图表分析」带入的数据集与筛选 */
  chartInitialDataset: { datasetId: string, filters: DatasetFilter[] } | null
}>()

const emit = defineEmits<{
  /** 数据集有变化（AI 助手 / 填报写入后）：页面刷新数据集与公式 */
  (e: 'datasetsChanged'): void
  (e: 'addChart', datasetId: string): void
  (e: 'editChart', chartId: string): void
  (e: 'present'): void
  (e: 'schedule'): void
  /** 透视表转为数据表：新建了汇总数据集 */
  (e: 'pivotConverted', datasetId: string): void
}>()

/** 正在编辑的仪表盘部件（切片器 / 指标卡 / 表格卡片 / 数据表）；为空时面板为新建 */
const dashboardEditId = defineModel<string | null>('dashboardEditId', { default: null })
/** 正在编辑的图表；为空时面板为新建 */
const chartEditId = defineModel<string | null>('chartEditId', { default: null })

const uiStore = useExcelUIStore()
</script>

<template>
  <!-- 侧边面板（浮层覆盖，不挤压工作表） -->
  <transition name="slide">
    <div v-if="uiStore.activePanel !== 'none'" class="side-panel" :class="{ wide: uiStore.activePanel === 'ai' }">
      <div class="panel-header">
        <span class="panel-title">
          {{ {
            'sort-filter': $t('排序与筛选'),
            'style': $t('单元格样式'),
            'formula': $t('公式函数'),
            'collaboration': $t('实时协作'),
            'snapshots': $t('版本历史'),
            'pivot': $t('数据透视表字段'),
            'chart': $t('图表'),
            'dashboard': $t('仪表盘'),
            'ai': $t('AI 分析助手'),
            'writeback': $t('填报回写'),
            'comments': $t('评论'),
          }[uiStore.activePanel] }}
        </span>
        <el-button text circle size="small" @click="uiStore.closePanel()">
          <svg viewBox="0 0 16 16" width="14" height="14" fill="none">
            <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
          </svg>
        </el-button>
      </div>
      <div class="panel-content" :class="{ flush: uiStore.activePanel === 'ai' }">
        <AiAssistantPanel
          v-if="uiStore.activePanel === 'ai'"
          :univer-api="univerApi"
          :workbook-id="workbookId"
          :operator="operator"
          :initial-thread-id="aiLinkThread"
          @changed="persistenceManager.markDirty()"
          @datasets-changed="emit('datasetsChanged')"
        />
        <CommentPanel v-else-if="uiStore.activePanel === 'comments'" :univer-api="univerApi" :workbook-id="workbookId" :operator="operator" />
        <SortFilterPanel v-else-if="uiStore.activePanel === 'sort-filter'" :univer-api="univerApi" />
        <StylePanel v-else-if="uiStore.activePanel === 'style'" :univer-api="univerApi" />
        <FormulaPanel v-else-if="uiStore.activePanel === 'formula'" :univer-api="univerApi" />
        <CollaborationPanel v-else-if="uiStore.activePanel === 'collaboration'" />
        <SnapshotPanel v-else-if="uiStore.activePanel === 'snapshots'" :univer-api="univerApi" />
        <PivotPanel v-else-if="uiStore.activePanel === 'pivot'" :univer-api="univerApi" :datasets="datasets" @changed="persistenceManager.markDirty()" @converted="(id: string) => emit('pivotConverted', id)" />
        <WritebackPanel
          v-else-if="uiStore.activePanel === 'writeback'"
          :univer-api="univerApi"
          :datasets="datasets"
          :workbook-id="workbookId"
          :operator="operator"
          :initial-thread-id="aiLinkThread"
          @changed="persistenceManager.markDirty()"
          @datasets-changed="emit('datasetsChanged')"
        />
        <DashboardPanel
          v-else-if="uiStore.activePanel === 'dashboard'"
          v-model:edit-id="dashboardEditId"
          :univer-api="univerApi"
          :datasets="datasets"
          @changed="persistenceManager.markDirty()"
          @add-chart="(id: string) => emit('addChart', id)"
          @edit-chart="(id: string) => emit('editChart', id)"
          @present="emit('present')"
          @schedule="emit('schedule')"
        />
        <ChartPanel
          v-else-if="uiStore.activePanel === 'chart'"
          v-model:chart-id="chartEditId"
          :univer-api="univerApi"
          :datasets="datasets"
          :initial-dataset="chartInitialDataset"
          @changed="persistenceManager.markDirty()"
        />
      </div>
    </div>
  </transition>
</template>

<style lang="scss" scoped>
/* Side Panel —— 浮层覆盖，不挤压工作表 */
.side-panel {
  position: absolute;
  right: 12px;
  top: 12px;
  bottom: 12px;
  width: 320px;
  background: white;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.12);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  z-index: 10;
}

.panel-header {
  height: 44px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 12px;
  border-bottom: 1px solid #e2e8f0;
  flex-shrink: 0;
}

.panel-title {
  font-size: 14px;
  font-weight: 600;
  color: #0f172a;
}

.panel-content {
  flex: 1;
  overflow-y: auto;
  padding: 12px;
}

.side-panel.wide {
  width: 400px;
}

.panel-content.flush {
  padding: 0;
  overflow: hidden;
}

/* Slide Transition */
.slide-enter-active,
.slide-leave-active {
  transition: transform 0.25s ease, opacity 0.25s ease;
}

.slide-enter-from,
.slide-leave-to {
  transform: translateX(100%);
  opacity: 0;
}
</style>
