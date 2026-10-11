<script setup lang="ts">
import { computed } from 'vue'
import { collabSession } from '@/features/excel/collaboration/session'
import { collaborationSync } from '@/features/excel/collaboration/sync-engine'
import { persistenceManager } from '@/features/excel/persistence/manager'
import { useExcelUIStore } from '@/stores/excel-ui'

/** 实时协作面板：谁在线、光标在哪；连接状态与「重新同步」 */
const uiStore = useExcelUIStore()
const session = collabSession

const me = computed(() => ({
  name: session.self.value?.name ?? collaborationSync.getUserName(),
  color: session.self.value?.color ?? collaborationSync.getUserColor(),
  canEdit: session.self.value?.canEdit !== false,
}))
const peers = computed(() => session.peers.value)
const onlineCount = computed(() => peers.value.length + 1)

const STATUS: Record<string, { text: string, tone: 'ok' | 'warn' | 'off' }> = {
  off: { text: '未连接实时协作', tone: 'off' },
  connecting: { text: '正在连接…', tone: 'warn' },
  online: { text: '实时协作中', tone: 'ok' },
  reconnecting: { text: '连接断开，正在重连…（修改会在重连后同步）', tone: 'warn' },
  waiting: { text: '等待在线的同事保存后加入…', tone: 'warn' },
  diverged: { text: '与服务器失去同步，需要重新载入', tone: 'warn' },
}
const status = computed(() => STATUS[session.status.value] ?? STATUS.off!)

function formatLastActive(timestamp: number): string {
  const diff = Date.now() - timestamp
  if (diff < 60_000)
    return '刚刚活跃'
  if (diff < 300_000)
    return `${Math.floor(diff / 60_000)} 分钟前活跃`
  return new Date(timestamp).toLocaleTimeString('zh-CN')
}

const cellOf = (c: { row: number, col: number }) => `${String.fromCharCode(65 + (c.col % 26))}${c.row + 1}`

async function resync() {
  if (persistenceManager.isDirty()) {
    const ok = await ElMessageBox.confirm('重新同步会载入服务器上最新保存的版本，你还没保存的修改会丢失。', '重新同步', { type: 'warning', confirmButtonText: '重新载入' }).then(() => true, () => false)
    if (!ok)
      return
  }
  await session.resync()
  ElMessage.success('已重新载入')
}
void uiStore
</script>

<template>
  <div class="collaboration-panel">
    <!-- 连接状态 -->
    <div class="online-summary" :class="status.tone">
      <span class="online-dot" />
      <span class="online-text">{{ status.text }}<template v-if="session.status.value === 'online'">（{{ onlineCount }} 人在线）</template></span>
    </div>
    <div v-if="session.staleBy.value || session.status.value === 'diverged'" class="stale">
      <template v-if="session.staleBy.value">
        {{ session.staleBy.value }} 做了一次很大的修改，没有实时同步给你。等他保存后点「重新同步」。
      </template>
      <template v-else>
        断线期间错过的修改已无法补齐，请重新同步。
      </template>
      <el-button size="small" type="primary" class="mt-1" @click="resync">
        重新同步
      </el-button>
    </div>

    <!-- 当前用户 -->
    <div class="panel-section">
      <div class="section-label">
        我
      </div>
      <div class="user-card current-user">
        <span class="user-avatar" :style="{ background: me.color }">
          {{ me.name.charAt(0) }}
        </span>
        <div class="user-info">
          <span class="user-name">{{ me.name }}</span>
          <span class="user-status">{{ me.canEdit ? '可编辑' : '只读（能看到别人的修改，自己的修改不会同步）' }}</span>
        </div>
      </div>
    </div>

    <!-- 协作者列表 -->
    <div class="panel-section">
      <div class="section-label">
        协作者
      </div>
      <div v-if="peers.length === 0" class="empty-hint">
        暂无其他协作者
        <span class="hint-tip">同事打开同一个工作簿后会出现在这里，修改实时互相可见</span>
      </div>
      <div
        v-for="c in peers"
        :key="c.id"
        class="user-card"
        data-peer
      >
        <span class="user-avatar" :style="{ background: c.color }">
          {{ c.name.charAt(0) }}
        </span>
        <div class="user-info">
          <span class="user-name">{{ c.name }}</span>
          <span class="user-status">{{ c.canEdit ? '' : '只读 · ' }}{{ formatLastActive(c.lastActive) }}</span>
        </div>
        <span v-if="c.cursor" class="cursor-badge" :style="{ background: c.color }">
          {{ cellOf(c.cursor) }}
        </span>
      </div>
    </div>

    <!-- 协作说明 -->
    <div class="collab-info">
      <div class="info-item">
        <span class="info-icon">●</span>
        单元格、格式、行列增删等修改实时同步，各人保存各自的修改
      </div>
      <div class="info-item">
        <span class="info-icon">●</span>
        同一个格子同时修改，以服务器收到的先后为准，后到的生效
      </div>
      <div class="info-item">
        <span class="info-icon">●</span>
        别人选中的格子旁显示他的名字；同时增删行列时可能错位，可以点「重新同步」
      </div>
    </div>
    <el-button v-if="session.status.value === 'online'" size="small" @click="resync">
      重新同步
    </el-button>
  </div>
</template>

<style lang="scss" scoped>
.collaboration-panel {
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 4px 0;
}

.online-summary {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background: var(--el-color-success-light-9);
  border-radius: 6px;

  &.warn {
    background: var(--el-color-warning-light-9);

    .online-dot { background: var(--el-color-warning); }
    .online-text { color: var(--el-color-warning); }
  }

  &.off {
    background: var(--el-fill-color-light);

    .online-dot { background: var(--el-text-color-placeholder); animation: none; }
    .online-text { color: var(--el-text-color-secondary); }
  }
}

.stale {
  font-size: 12px;
  padding: 8px 12px;
  border-radius: 6px;
  background: var(--el-color-warning-light-9);
  color: var(--el-color-warning-dark-2);
}

.online-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--el-color-success);
  animation: pulse 2s ease-in-out infinite;
}

.online-text {
  font-size: 13px;
  font-weight: 600;
  color: var(--el-color-success);
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}

.panel-section {
  .section-label {
    font-size: 13px;
    font-weight: 600;
    margin-bottom: 8px;
    color: var(--el-text-color-primary);
  }
}

.user-card {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px;
  border-radius: 6px;
  transition: background 0.15s;

  &:hover {
    background: var(--el-fill-color-light);
  }
}

.current-user {
  background: var(--el-color-primary-light-9);
}

.user-avatar {
  width: 32px;
  height: 32px;
  border-radius: 50%;
  display: grid;
  place-items: center;
  color: white;
  font-size: 14px;
  font-weight: 600;
  flex-shrink: 0;
}

.user-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
}

.user-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--el-text-color-primary);
}

.user-status {
  font-size: 11px;
  color: var(--el-text-color-secondary);
}

.cursor-badge {
  font-size: 10px;
  font-weight: 600;
  color: white;
  padding: 2px 6px;
  border-radius: 8px;
}

.empty-hint {
  font-size: 13px;
  color: var(--el-text-color-secondary);
  text-align: center;
  padding: 20px 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.hint-tip {
  font-size: 11px;
  color: var(--el-text-color-placeholder);
}

.collab-info {
  border-top: 1px solid var(--el-border-color-lighter);
  padding-top: 12px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.info-item {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--el-text-color-secondary);
}

.info-icon {
  color: var(--el-color-success);
  font-size: 8px;
}
</style>
