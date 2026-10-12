<script setup lang="ts">
import type { TreeNode, View } from './manager/folder-model'
import type { TrashedWorkbook, WorkbookFolder, WorkbookMeta } from '@/api/workbook'
import { computed, nextTick, ref, watch } from 'vue'
import {
  createFolderApi,
  deleteFolderApi,
  deleteWorkbookApi,
  duplicateWorkbookApi,
  emptyTrashApi,
  listFoldersApi,
  listTrashApi,
  listWorkbooksApi,
  moveWorkbooksApi,
  purgeWorkbookApi,
  renameWorkbookApi,
  restoreFromTrashApi,
  starWorkbookApi,
  updateFolderApi,
} from '@/api/workbook'
import { useLedgerUserStore } from '@/stores/ledger-user'
import { ROOT } from './manager/folder-model'
import FolderSidebar from './manager/FolderSidebar.vue'
import MoveFolderDialog from './manager/MoveFolderDialog.vue'
import TrashTable from './manager/TrashTable.vue'
import WorkbookListTable from './manager/WorkbookListTable.vue'

/**
 * 服务端工作簿管理：文件夹（可嵌套）、星标、搜索、回收站；打开、新建、另存为、重命名、复制、移动、删除
 */
const props = defineProps<{
  visible: boolean
  currentId: string | null
  operator: string
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
  (e: 'open', meta: WorkbookMeta): void
  /** blank：新建空白；copy：把当前内容另存为新工作簿；folderId：放到哪个文件夹 */
  (e: 'create', name: string, mode: 'blank' | 'copy', folderId: string | null): void
  /** 当前工作簿被改名 */
  (e: 'renamed', meta: WorkbookMeta): void
  /** 从模板新建 */
  (e: 'template'): void
  /** 把当前工作簿发布为模板 */
  (e: 'publish'): void
}>()

const list = ref<WorkbookMeta[]>([])
const folders = ref<WorkbookFolder[]>([])
const trash = ref<TrashedWorkbook[]>([])
/** 回收站保留天数（不自动清理时为 null） */
const trashDays = ref<number | null>(null)
const loading = ref(false)
const keyword = ref('')
const view = ref<View>({ kind: 'folder', id: ROOT })
const selected = ref<WorkbookMeta[]>([])
const ledgerUser = useLedgerUserStore()
/** 当前用户对工作簿的权限：改名 / 移动需要编辑权限，删除需要管理权限 */
const canEdit = (w: WorkbookMeta) => ledgerUser.accessLevel(w) >= 2
const canManage = (w: WorkbookMeta) => ledgerUser.accessLevel(w) >= 3

async function load() {
  loading.value = true
  try {
    const [wbs, fds, tr] = await Promise.all([listWorkbooksApi(), listFoldersApi(), listTrashApi()])
    list.value = wbs
    folders.value = fds
    trash.value = tr.items
    trashDays.value = tr.days || null
    // 当前文件夹已被删除（别处操作）：回到根目录
    if (view.value.kind === 'folder' && view.value.id !== ROOT && !fds.some(f => f.id === (view.value as { id: string }).id))
      view.value = { kind: 'folder', id: ROOT }
  }
  finally {
    loading.value = false
  }
}

watch(() => props.visible, async (v) => {
  if (!v)
    return
  keyword.value = ''
  await load()
  // 打开时定位到当前工作簿所在的文件夹
  const cur = list.value.find(w => w.id === props.currentId)
  view.value = { kind: 'folder', id: cur?.folderId ?? ROOT }
})

// ── 文件夹树 ──

const directCount = computed(() => {
  const m = new Map<string, number>()
  for (const w of list.value)
    m.set(w.folderId ?? ROOT, (m.get(w.folderId ?? ROOT) ?? 0) + 1)
  return m
})

const tree = computed<TreeNode[]>(() => {
  const build = (parent: string | null): TreeNode[] => folders.value
    .filter(f => f.parentId === parent)
    .sort((a, b) => a.name.localeCompare(b.name, 'zh-CN'))
    .map(f => ({ id: f.id, label: f.name, count: directCount.value.get(f.id) ?? 0, children: build(f.id) }))
  return [{ id: ROOT, label: '我的工作簿', count: directCount.value.get(ROOT) ?? 0, children: build(null) }]
})

const folderById = computed(() => new Map(folders.value.map(f => [f.id, f])))

/** 文件夹路径（如「财务 / 月报」）；根目录为空 */
function pathOf(folderId: string | null): string {
  const names: string[] = []
  let cur = folderId ? folderById.value.get(folderId) : undefined
  while (cur && names.length < 10) {
    names.unshift(cur.name)
    cur = cur.parentId ? folderById.value.get(cur.parentId) : undefined
  }
  return names.join(' / ')
}

/** 文件夹及其全部子孙 */
function subtree(id: string): Set<string> {
  const out = new Set([id])
  let grew = true
  while (grew) {
    grew = false
    for (const f of folders.value) {
      if (f.parentId && out.has(f.parentId) && !out.has(f.id)) {
        out.add(f.id)
        grew = true
      }
    }
  }
  return out
}

const currentFolderId = computed(() => (view.value.kind === 'folder' && view.value.id !== ROOT ? view.value.id : null))
const breadcrumb = computed(() => {
  const v = view.value
  if (keyword.value.trim())
    return `搜索「${keyword.value.trim()}」`
  if (v.kind === 'all')
    return '全部工作簿'
  if (v.kind === 'starred')
    return '星标'
  if (v.kind === 'trash')
    return '回收站'
  return v.id === ROOT ? '我的工作簿' : `我的工作簿 / ${pathOf(v.id)}`
})

const subfolders = computed(() => {
  const v = view.value
  if (v.kind !== 'folder' || keyword.value.trim())
    return []
  const parent = v.id === ROOT ? null : v.id
  return folders.value.filter(f => f.parentId === parent).sort((a, b) => a.name.localeCompare(b.name, 'zh-CN'))
})

/** 搜索：在全部工作簿里按名称与文件夹路径匹配 */
const shown = computed(() => {
  const kw = keyword.value.trim().toLowerCase()
  if (kw)
    return list.value.filter(w => w.name.toLowerCase().includes(kw) || pathOf(w.folderId).toLowerCase().includes(kw))
  const v = view.value
  if (v.kind === 'all')
    return list.value
  if (v.kind === 'starred')
    return list.value.filter(w => w.starred)
  if (v.kind === 'folder')
    return list.value.filter(w => (w.folderId ?? ROOT) === v.id)
  return []
})
const showPath = computed(() => !!keyword.value.trim() || view.value.kind === 'all' || view.value.kind === 'starred')

const sidebar = ref<{ expand: (id: string) => void }>()

// 进入某个文件夹时展开它的上级，侧栏里能看到当前位置
watch([view, folders], async () => {
  const v = view.value
  if (v.kind !== 'folder' || v.id === ROOT)
    return
  await nextTick()
  let cur = folderById.value.get(v.id)?.parentId ?? null
  const chain: string[] = [ROOT]
  while (cur && chain.length < 10) {
    chain.splice(1, 0, cur)
    cur = folderById.value.get(cur)?.parentId ?? null
  }
  chain.forEach(id => sidebar.value?.expand(id))
})

function go(v: View) {
  view.value = v
  keyword.value = ''
  selected.value = []
}

function isActive(v: View) {
  if (keyword.value.trim())
    return false
  const c = view.value
  return c.kind === v.kind && (c.kind !== 'folder' || (v.kind === 'folder' && c.id === v.id))
}

// ── 通用 ──

async function promptName(title: string, value: string, max = 80) {
  return ElMessageBox.prompt('', title, {
    inputValue: value,
    inputValidator: v => (!!v.trim() && v.trim().length <= max) || `名称不能为空，且不超过 ${max} 个字符`,
    confirmButtonText: '确定',
  }).then(r => r.value.trim(), () => null)
}

async function confirm(message: string, title: string, button: string) {
  return ElMessageBox.confirm(message, title, { type: 'warning', confirmButtonText: button, confirmButtonClass: 'ec-button--danger' }).then(() => true, () => false)
}

const trashDaysText = computed(() => (trashDays.value ? `（保留 ${trashDays.value} 天）` : ''))

// ── 工作簿操作 ──

async function create(mode: 'blank' | 'copy') {
  const current = list.value.find(w => w.id === props.currentId)
  const name = await promptName(mode === 'blank' ? '新建工作簿' : '另存为新工作簿', mode === 'blank' ? '未命名工作簿' : `${current?.name ?? '工作簿'} 副本`)
  if (!name)
    return
  emit('create', name, mode, currentFolderId.value)
  emit('update:visible', false)
}

async function rename(w: WorkbookMeta) {
  const name = await promptName('重命名工作簿', w.name)
  if (!name || name === w.name)
    return
  const meta = await renameWorkbookApi(w.id, name)
  if (w.id === props.currentId)
    emit('renamed', meta)
  await load()
}

async function duplicate(w: WorkbookMeta) {
  const name = await promptName('复制工作簿', `${w.name} 副本`)
  if (!name)
    return
  await duplicateWorkbookApi(w.id, name, props.operator)
  ElMessage.success('已复制（放在同一个文件夹）')
  await load()
}

async function toggleStar(w: WorkbookMeta) {
  const next = !w.starred
  w.starred = next
  try {
    await starWorkbookApi(w.id, next)
  }
  catch {
    w.starred = !next
  }
}

async function remove(items: WorkbookMeta[]) {
  if (items.some(w => w.id === props.currentId)) {
    ElMessage.warning('不能删除当前打开的工作簿，请先打开其他工作簿')
    return
  }
  const what = items.length === 1 ? `「${items[0]!.name}」` : `选中的 ${items.length} 个工作簿`
  if (!(await confirm(`把${what}移到回收站？可在回收站恢复${trashDaysText.value}。`, '删除工作簿', '移到回收站')))
    return
  for (const w of items)
    await deleteWorkbookApi(w.id, props.operator)
  ElMessage.success('已移到回收站')
  selected.value = []
  await load()
}

function open(w: WorkbookMeta) {
  if (w.id !== props.currentId)
    emit('open', w)
  emit('update:visible', false)
}

// ── 移动 ──

const moveDialog = ref<{ title: string, target: string, exclude: Set<string>, run: (folderId: string | null) => Promise<void> } | null>(null)

/** 移动对话框里的文件夹树（排除不能选的：文件夹自己与它的子孙） */
const moveTree = computed(() => {
  const ex = moveDialog.value?.exclude ?? new Set<string>()
  const strip = (nodes: TreeNode[]): TreeNode[] => nodes.filter(n => !ex.has(n.id)).map(n => ({ ...n, children: strip(n.children) }))
  return strip(tree.value)
})

const moveTarget = computed({
  get: () => moveDialog.value?.target ?? ROOT,
  set: (v: string) => {
    if (moveDialog.value)
      moveDialog.value.target = v
  },
})

const emptyText = computed(() => keyword.value.trim() ? '没有匹配的工作簿' : view.value.kind === 'starred' ? '还没有星标的工作簿（点名称前的 ☆ 加星标）' : '这里还没有工作簿')

function moveWorkbooks(items: WorkbookMeta[]) {
  const first = items[0]
  if (!first)
    return
  moveDialog.value = {
    title: items.length === 1 ? `移动「${first.name}」到` : `移动 ${items.length} 个工作簿到`,
    target: items.length === 1 ? (first.folderId ?? ROOT) : ROOT,
    exclude: new Set(),
    run: async (folderId) => {
      await moveWorkbooksApi(items.map(w => w.id), folderId)
      ElMessage.success(`已移到「${folderId ? pathOf(folderId) : '我的工作簿'}」`)
      selected.value = []
    },
  }
}

async function confirmMove() {
  const d = moveDialog.value
  if (!d)
    return
  await d.run(d.target === ROOT ? null : d.target)
  moveDialog.value = null
  await load()
}

// ── 文件夹操作 ──

async function newFolder(parentId: string | null) {
  const name = await promptName(parentId ? `在「${pathOf(parentId)}」下新建文件夹` : '新建文件夹', '新建文件夹', 40)
  if (!name)
    return
  const f = await createFolderApi(name, parentId, props.operator)
  await load()
  go({ kind: 'folder', id: f.id })
}

async function renameFolder(f: WorkbookFolder) {
  const name = await promptName('重命名文件夹', f.name, 40)
  if (!name || name === f.name)
    return
  await updateFolderApi(f.id, name, f.parentId)
  await load()
}

function moveFolder(f: WorkbookFolder) {
  moveDialog.value = {
    title: `移动文件夹「${f.name}」到`,
    target: f.parentId ?? ROOT,
    exclude: subtree(f.id),
    run: async (parentId) => {
      await updateFolderApi(f.id, f.name, parentId)
      ElMessage.success('已移动')
    },
  }
}

async function deleteFolder(f: WorkbookFolder) {
  const ids = subtree(f.id)
  const inside = list.value.filter(w => w.folderId && ids.has(w.folderId))
  if (inside.some(w => w.id === props.currentId)) {
    ElMessage.warning('当前打开的工作簿在这个文件夹里，请先把它移出或打开其他工作簿')
    return
  }
  const msg = inside.length
    ? `删除文件夹「${f.name}」${ids.size > 1 ? `及其 ${ids.size - 1} 个子文件夹` : ''}，其中的 ${inside.length} 个工作簿移到回收站（恢复时回到「我的工作簿」）。`
    : `删除文件夹「${f.name}」${ids.size > 1 ? `及其 ${ids.size - 1} 个子文件夹` : ''}？`
  if (!(await confirm(msg, '删除文件夹', '删除')))
    return
  const n = await deleteFolderApi(f.id, props.operator)
  ElMessage.success(n ? `已删除，${n} 个工作簿移到回收站` : '已删除')
  if (view.value.kind === 'folder' && ids.has(view.value.id))
    go({ kind: 'folder', id: f.parentId ?? ROOT })
  await load()
}

function onFolderCommand(id: string, cmd: 'new' | 'rename' | 'move' | 'delete') {
  if (cmd === 'new') {
    newFolder(id === ROOT ? null : id)
    return
  }
  const f = folderById.value.get(id)
  if (!f)
    return
  if (cmd === 'rename')
    renameFolder(f)
  else if (cmd === 'move')
    moveFolder(f)
  else
    deleteFolder(f)
}

// ── 回收站 ──

async function restore(t: TrashedWorkbook) {
  const meta = await restoreFromTrashApi(t.id)
  ElMessage.success(`已恢复到「${meta.folderId ? pathOf(meta.folderId) : '我的工作簿'}」`)
  await load()
}

async function purge(t: TrashedWorkbook) {
  if (!(await confirm(`彻底删除「${t.name}」及其全部历史版本，删除后不可恢复。`, '彻底删除', '彻底删除')))
    return
  await purgeWorkbookApi(t.id)
  ElMessage.success('已彻底删除')
  await load()
}

async function emptyTrash() {
  if (!(await confirm(`彻底删除回收站里的 ${trash.value.length} 个工作簿及其全部历史版本，删除后不可恢复。`, '清空回收站', '清空')))
    return
  const n = await emptyTrashApi()
  ElMessage.success(`已彻底删除 ${n} 个工作簿`)
  await load()
}
</script>

<template>
  <el-dialog :model-value="visible" title="工作簿" width="1200px" append-to-body @update:model-value="emit('update:visible', $event)">
    <div class="mb-3 flex items-center gap-2">
      <el-input v-model="keyword" size="small" clearable placeholder="搜索全部工作簿（名称、文件夹）" class="!w-72" />
      <div class="ml-auto flex gap-2">
        <el-button size="small" :disabled="!currentId" title="把当前工作簿发布为模板，供同事一键使用" @click="emit('update:visible', false); emit('publish')">
          发布为模板…
        </el-button>
        <el-button size="small" title="另存到当前浏览的文件夹" @click="create('copy')">
          另存为…
        </el-button>
        <el-button size="small" type="primary" plain @click="emit('update:visible', false); emit('template')">
          从模板新建
        </el-button>
        <el-button size="small" type="primary" title="新建到当前浏览的文件夹" @click="create('blank')">
          新建空白工作簿
        </el-button>
      </div>
    </div>

    <div v-loading="loading" class="flex h-[460px] border border-[#ebeef5] rounded border-solid">
      <FolderSidebar
        ref="sidebar"
        :tree="tree"
        :all-count="list.length"
        :starred-count="list.filter(w => w.starred).length"
        :trash-count="trash.length"
        :is-active="isActive"
        @go="go"
        @new-folder="newFolder"
        @folder-command="onFolderCommand"
      />

      <!-- 右侧内容 -->
      <main class="min-w-0 flex flex-1 flex-col">
        <div class="flex items-center gap-2 border-b border-b-[#ebeef5] border-b-solid px-3 py-2 text-[13px]">
          <span class="font-medium text-gray-700">{{ breadcrumb }}</span>
          <template v-if="view.kind === 'trash' && !keyword.trim()">
            <span class="text-xs text-gray-400">{{ trashDays ? `删除的工作簿保留 ${trashDays} 天后自动彻底删除` : '删除的工作簿先放在这里，可以恢复' }}</span>
            <el-button class="ml-auto" size="small" type="danger" plain :disabled="!trash.length" @click="emptyTrash">
              清空回收站
            </el-button>
          </template>
          <template v-else-if="selected.length">
            <span class="ml-auto text-xs text-gray-500">已选 {{ selected.length }} 个</span>
            <el-button size="small" :disabled="selected.some(w => !canEdit(w))" @click="moveWorkbooks(selected)">
              移动到…
            </el-button>
            <el-button size="small" type="danger" plain :disabled="selected.some(w => !canManage(w))" @click="remove(selected)">
              删除
            </el-button>
          </template>
          <el-button v-else-if="view.kind === 'folder' && !keyword.trim()" class="ml-auto" size="small" @click="newFolder(currentFolderId)">
            新建文件夹
          </el-button>
        </div>

        <!-- 子文件夹 -->
        <div v-if="subfolders.length" class="flex flex-wrap gap-2 px-3 pt-2">
          <button v-for="f in subfolders" :key="f.id" class="chip" @click="go({ kind: 'folder', id: f.id })">
            <span class="text-gray-400">▸</span> {{ f.name }}
            <span class="text-gray-400">{{ directCount.get(f.id) ?? 0 }}</span>
          </button>
        </div>

        <TrashTable v-if="view.kind === 'trash' && !keyword.trim()" :items="trash" @restore="restore" @purge="purge" />
        <WorkbookListTable
          v-else
          :items="shown"
          :current-id="currentId"
          :show-path="showPath"
          :empty-text="emptyText"
          :path-of="pathOf"
          :can-edit="canEdit"
          :can-manage="canManage"
          @open="open"
          @star="toggleStar"
          @rename="rename"
          @duplicate="duplicate"
          @move="moveWorkbooks"
          @remove="remove"
          @selection="(rows: WorkbookMeta[]) => selected = rows"
        />
      </main>
    </div>
    <div class="mt-2 text-xs text-gray-400">
      工作簿保存在服务端：修改后自动保存，每次保存都会留下版本，可在「快照」面板中查看与恢复。双击打开；删除的工作簿先进回收站
    </div>

    <MoveFolderDialog v-model:target="moveTarget" :open="!!moveDialog" :title="moveDialog?.title" :tree="moveTree" @cancel="moveDialog = null" @confirm="confirmMove" />
  </el-dialog>
</template>

<style scoped lang="scss">
.chip {
  @apply flex items-center gap-1.5 rounded border border-[#e5e7eb] border-solid bg-white px-2.5 py-1 text-[13px] text-gray-700 cursor-pointer hover:border-[#217346];
}
</style>
