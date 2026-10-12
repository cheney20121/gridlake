<script setup lang="ts">
import type { FUniver } from '@univerjs/core/facade'
import type { CommentItem, CommentMention, CommentThread } from '@/api/comment'
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { listLedgerUsersApi } from '@/api/approval'
import { addCommentApi, deleteCommentApi, editCommentApi, resolveCommentApi } from '@/api/comment'
import { activeMention, insertMention, mentionsIn, splitMentions } from '@/features/excel/comments/mention'
import { commentFocus, commentThreads, reloadComments } from '@/features/excel/comments/state'
import { cellName } from '@/features/excel/writeback/input-table'
import { t as tr } from '@/i18n'
import { useLedgerUserStore } from '@/stores/ledger-user'

/**
 * 单元格评论面板：选中格子后发起评论，输入 @ 选人（被 @ 的人在顶栏铃铛里收到通知）；
 * 每条线程可回复、标记已解决、删除；「本页全部评论」点一下定位到格子
 */
const props = defineProps<{
  univerApi: FUniver | null
  /** 当前工作簿（服务端 ID）；未保存时为空 */
  workbookId: string | null
  operator?: string
}>()

const me = useLedgerUserStore()
const sheetId = ref('')
const sheetName = ref('')
const cell = ref({ row: 0, col: 0 })
const showResolved = ref(false)
const replyTo = ref<string | null>(null)
const busy = ref(false)

function readSelection() {
  const sheet = props.univerApi?.getActiveWorkbook()?.getActiveSheet()
  if (!sheet)
    return
  sheetId.value = sheet.getSheetId()
  sheetName.value = sheet.getSheetName()
  const r = sheet.getActiveRange()
  if (r)
    cell.value = { row: r.getRow(), col: r.getColumn() }
}

const offs: (() => void)[] = []
watch(() => props.univerApi, (api) => {
  offs.forEach(o => o())
  offs.length = 0
  if (!api)
    return
  readSelection()
  const a = api.addEvent(api.Event.SelectionChanged, readSelection)
  const b = api.addEvent(api.Event.ActiveSheetChanged, readSelection)
  offs.push(() => a.dispose(), () => b.dispose())
}, { immediate: true })
onBeforeUnmount(() => offs.forEach(o => o()))

watch(() => props.workbookId, id => reloadComments(id), { immediate: true })

const address = computed(() => cellName(cell.value.row, cell.value.col))
const here = computed(() => commentThreads.value.filter(t => t.sheetId === sheetId.value && t.row === cell.value.row && t.col === cell.value.col))
const onSheet = computed(() => commentThreads.value.filter(t => t.sheetId === sheetId.value && !here.value.includes(t) && (showResolved.value || !t.resolved)))
const resolvedCount = computed(() => commentThreads.value.filter(t => t.sheetId === sheetId.value && t.resolved).length)

// 从通知 / 格子旁的按钮跳来：选中那个格子
watch(commentFocus, async (f) => {
  if (!f)
    return
  const wb = props.univerApi?.getActiveWorkbook()
  const sheet = wb?.getSheetBySheetId(f.sheetId)
  if (!wb || !sheet)
    return
  wb.setActiveSheet(sheet)
  sheet.getRange(f.row, f.col).activate()
  await nextTick()
  readSelection()
}, { immediate: true })

function locate(t: CommentItem) {
  commentFocus.value = { sheetId: t.sheetId, row: t.row, col: t.col, nonce: Date.now() }
}

// ── 输入框与 @ ──

interface Draft {
  text: string
  picked: CommentMention[]
  caret: number
}
const drafts = ref<Record<string, Draft>>({})
const draftOf = (key: string): Draft => (drafts.value[key] ??= { text: '', picked: [], caret: 0 })
const NEW = 'new'

const menu = ref<{ key: string, start: number, keyword: string, items: CommentMention[] } | null>(null)
let searchSeq = 0

async function onInput(key: string, e: Event) {
  const el = e.target as HTMLTextAreaElement
  const d = draftOf(key)
  d.text = el.value
  d.caret = el.selectionStart ?? el.value.length
  const m = activeMention(d.text, d.caret)
  if (!m) {
    menu.value = null
    return
  }
  const seq = ++searchSeq
  const users = await listLedgerUsersApi(m.keyword).catch(() => [])
  if (seq !== searchSeq)
    return
  menu.value = { key, start: m.start, keyword: m.keyword, items: users.slice(0, 6).map(u => ({ id: u.id, name: u.name })) }
}

function pick(key: string, u: CommentMention) {
  const d = draftOf(key)
  const m = menu.value
  if (!m)
    return
  const r = insertMention(d.text, d.caret, m.start, u.name)
  d.text = r.text
  d.caret = r.caret
  if (!d.picked.some(p => p.id === u.id))
    d.picked.push(u)
  menu.value = null
}

/** Ctrl / ⌘ + Enter 发送；键盘事件不冒泡到工作表（否则输入会改动选中的格子） */
function onKey(e: KeyboardEvent, key: string, parentId?: string) {
  e.stopPropagation()
  if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault()
    submit(key, parentId)
  }
}

async function submit(key: string, parentId?: string) {
  const d = draftOf(key)
  const body = d.text.trim()
  if (!body || !props.workbookId || busy.value)
    return
  busy.value = true
  try {
    const mentions = mentionsIn(body, d.picked).map(m => m.id)
    const r = await addCommentApi(props.workbookId, parentId
      ? { parentId, body, mentions, operator: props.operator }
      : { sheetId: sheetId.value, row: cell.value.row, col: cell.value.col, body, mentions, operator: props.operator })
    if (r.notNotified.length)
      ElMessage.warning(tr('{0} 没有这个工作簿的查看权限，没有通知到。可以先在「文件 → 打开 / 新建 / 管理工作簿」里调整共享范围', [r.notNotified.join('、')]))
    drafts.value[key] = { text: '', picked: [], caret: 0 }
    if (parentId)
      replyTo.value = null
    await reloadComments(props.workbookId)
  }
  finally {
    busy.value = false
  }
}

const editing = ref<string | null>(null)

function startEdit(c: CommentItem) {
  editing.value = c.id
  drafts.value[`edit:${c.id}`] = { text: c.body, picked: [...c.mentions], caret: c.body.length }
}

async function saveEdit(c: CommentItem) {
  const d = draftOf(`edit:${c.id}`)
  if (!d.text.trim() || busy.value)
    return
  busy.value = true
  try {
    const r = await editCommentApi(c.id, d.text.trim(), mentionsIn(d.text, d.picked).map(m => m.id))
    if (r.notNotified.length)
      ElMessage.warning(tr('{0} 没有这个工作簿的查看权限，没有通知到', [r.notNotified.join('、')]))
    editing.value = null
    await reloadComments(props.workbookId)
  }
  finally {
    busy.value = false
  }
}

async function remove(c: CommentItem, isThread: boolean) {
  const ok = await ElMessageBox.confirm(isThread ? tr('删除这条评论线程？其中的回复一并删除。') : tr('删除这条回复？'), tr('删除评论'), { type: 'warning', confirmButtonText: tr('删除') }).then(() => true, () => false)
  if (!ok)
    return
  await deleteCommentApi(c.id)
  await reloadComments(props.workbookId)
}

async function toggleResolved(t: CommentThread) {
  await resolveCommentApi(t.id, !t.resolved)
  await reloadComments(props.workbookId)
}

const mine = (c: CommentItem) => !me.authRequired || (!!c.authorId && c.authorId === me.user?.id)
/** 删除：作者本人或管理员（工作簿所有者也可以，服务端判断，界面上多给一个入口会被拒绝时提示） */
const canDelete = (c: CommentItem) => mine(c) || me.isAdmin
</script>

<template>
  <div class="comments flex h-full flex-col text-[13px]">
    <div v-if="!workbookId" class="m-3 rounded-md bg-[#f9f9f7] p-3 text-xs text-gray-500">
      {{ $t('工作簿保存到服务端后即可评论。') }}
    </div>
    <template v-else>
      <div class="border-b border-b-[#eef0f3] border-b-solid px-3 py-2">
        <div class="mb-1 text-xs text-gray-500">
          {{ $t('{0} · 选中的格子', [sheetName]) }} <b class="text-[#0E5A31]">{{ address }}</b>
        </div>
        <div class="relative">
          <textarea
            class="box"
            rows="3"
            :value="draftOf(NEW).text"
            :placeholder="$t('写评论，输入 @ 提醒同事（Ctrl+Enter 发送）')"
            @input="onInput(NEW, $event)"
            @keydown="onKey($event, NEW)"
          />
          <div v-if="menu && menu.key === NEW && menu.items.length" class="menu">
            <button v-for="u in menu.items" :key="u.id" type="button" class="menu-item" @mousedown.prevent @click="pick(NEW, u)">
              {{ u.name }}
            </button>
          </div>
        </div>
        <div class="mt-1 flex justify-end">
          <el-button size="small" type="primary" :disabled="!draftOf(NEW).text.trim()" :loading="busy" @click="submit(NEW)">
            {{ $t('评论') }}
          </el-button>
        </div>
      </div>

      <div class="min-h-0 flex-1 overflow-y-auto px-3 py-2" @wheel.stop>
        <template v-if="here.length">
          <div class="mb-1 text-xs text-gray-500">
            {{ $t('{0} 的评论', [address]) }}
          </div>
          <article v-for="t in here" :key="t.id" class="thread" :class="{ done: t.resolved }" data-thread>
            <template v-for="(c, i) in [t, ...t.replies]" :key="c.id">
              <div class="cm" :class="{ reply: i > 0 }">
                <div class="meta">
                  <b>{{ c.authorName }}</b>
                  <span class="text-gray-400">{{ c.createdAt.slice(5, 16) }}{{ c.editedAt ? $t('（已编辑）') : '' }}</span>
                  <span v-if="i === 0 && t.resolved" class="tag">{{ $t('已解决') }}</span>
                </div>
                <template v-if="editing === c.id">
                  <textarea class="box" rows="2" :value="draftOf(`edit:${c.id}`).text" @input="onInput(`edit:${c.id}`, $event)" @keydown.stop />
                  <div v-if="menu && menu.key === `edit:${c.id}` && menu.items.length" class="menu">
                    <button v-for="u in menu.items" :key="u.id" type="button" class="menu-item" @mousedown.prevent @click="pick(`edit:${c.id}`, u)">
                      {{ u.name }}
                    </button>
                  </div>
                  <div class="mt-1 flex justify-end gap-1">
                    <el-button size="small" @click="editing = null">
                      {{ $t('取消') }}
                    </el-button>
                    <el-button size="small" type="primary" :loading="busy" @click="saveEdit(c)">
                      {{ $t('保存') }}
                    </el-button>
                  </div>
                </template>
                <div v-else class="body">
                  <template v-for="(p, pi) in splitMentions(c.body, c.mentions)" :key="pi">
                    <span v-if="p.mention" class="at">{{ p.text }}</span><template v-else>
                      {{ p.text }}
                    </template>
                  </template>
                </div>
                <div v-if="editing !== c.id" class="acts">
                  <button v-if="i === 0" type="button" @click="replyTo = replyTo === t.id ? null : t.id">
                    {{ $t('回复') }}
                  </button>
                  <button v-if="i === 0" type="button" @click="toggleResolved(t)">
                    {{ t.resolved ? $t('重新打开') : $t('标记已解决') }}
                  </button>
                  <button v-if="mine(c)" type="button" @click="startEdit(c)">
                    {{ $t('编辑') }}
                  </button>
                  <button v-if="canDelete(c)" type="button" class="danger" @click="remove(c, i === 0)">
                    {{ $t('删除') }}
                  </button>
                </div>
              </div>
            </template>
            <div v-if="replyTo === t.id" class="reply-box relative">
              <textarea
                class="box"
                rows="2"
                :value="draftOf(`reply:${t.id}`).text"
                :placeholder="$t('回复…（Ctrl+Enter 发送）')"
                @input="onInput(`reply:${t.id}`, $event)"
                @keydown="onKey($event, `reply:${t.id}`, t.id)"
              />
              <div v-if="menu && menu.key === `reply:${t.id}` && menu.items.length" class="menu">
                <button v-for="u in menu.items" :key="u.id" type="button" class="menu-item" @mousedown.prevent @click="pick(`reply:${t.id}`, u)">
                  {{ u.name }}
                </button>
              </div>
              <div class="mt-1 flex justify-end">
                <el-button size="small" type="primary" :disabled="!draftOf(`reply:${t.id}`).text.trim()" :loading="busy" @click="submit(`reply:${t.id}`, t.id)">
                  {{ $t('回复') }}
                </el-button>
              </div>
            </div>
          </article>
        </template>
        <div v-else class="mb-2 text-xs text-gray-400">
          {{ $t('{0} 还没有评论', [address]) }}
        </div>

        <div class="mb-1 mt-3 flex items-center justify-between text-xs text-gray-500">
          <span>{{ $t('本页其他评论（{0}）', [onSheet.length]) }}</span>
          <el-checkbox v-if="resolvedCount" v-model="showResolved" size="small">
            {{ $t('含已解决（{0}）', [resolvedCount]) }}
          </el-checkbox>
        </div>
        <button v-for="t in onSheet" :key="t.id" type="button" class="row" :class="{ done: t.resolved }" @click="locate(t)">
          <span class="addr">{{ cellName(t.row, t.col) }}</span>
          <span class="min-w-0 flex-1 truncate">{{ t.authorName }}：{{ t.body }}</span>
          <span v-if="t.replies.length" class="text-gray-400">{{ $t('{0} 回复', [t.replies.length]) }}</span>
        </button>
        <div v-if="!onSheet.length" class="text-xs text-gray-400">
          {{ $t('没有') }}
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped lang="scss">
.box {
  @apply box-border w-full resize-none rounded-md border border-[#dcdfe6] border-solid px-2 py-1.5 text-[13px] leading-5 outline-none focus:border-[#217346];
  font-family: inherit;
}

.menu {
  @apply absolute left-0 right-0 z-10 mt-0.5 flex flex-col rounded-md border border-[#e5e7eb] border-solid bg-white py-1 shadow-lg;
}

.menu-item {
  @apply cursor-pointer border-none bg-transparent px-3 py-1 text-left text-[13px] hover:bg-[#E7F4EC];
}

.thread {
  @apply mb-2 rounded-md border border-[#e5e7eb] border-solid bg-white p-2;

  &.done {
    @apply bg-[#f9f9f7] opacity-80;
  }
}

.cm {
  @apply relative py-1;

  &.reply {
    @apply ml-3 border-l-2 border-l-[#e5e7eb] border-l-solid pl-2;
  }
}

.meta {
  @apply flex items-center gap-2 text-xs;
}

.tag {
  @apply rounded bg-[#E7F4EC] px-1.5 text-[11px] text-[#217346];
}

.body {
  @apply whitespace-pre-wrap break-words leading-5;
}

.at {
  @apply rounded bg-[#E7F4EC] px-0.5 text-[#0E5A31];
}

.acts {
  @apply mt-0.5 flex gap-3 text-xs;

  button {
    @apply cursor-pointer border-none bg-transparent p-0 text-gray-500 hover:text-[#217346];

    &.danger:hover {
      @apply text-[#B91C1C];
    }
  }
}

.reply-box {
  @apply mt-1;
}

.row {
  @apply mb-1 w-full flex cursor-pointer items-center gap-2 rounded-md border-none bg-[#f9f9f7] px-2 py-1.5 text-left text-xs hover:bg-[#E7F4EC];

  &.done {
    @apply opacity-60;
  }

  .addr {
    @apply shrink-0 text-[#0E5A31] font-medium;
  }
}
</style>
