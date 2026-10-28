<script setup lang="ts">
import { computed, ref, shallowRef, reactive, onMounted, onBeforeUnmount, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Refresh, UploadFilled } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import { platform } from '../platform'
import { ApiError } from '../core/client'
import { UploadJournal, type UploadDraft } from '../core/upload-journal'
import { UploadController, type UploadTransport } from '../core/upload-controller'
import {
  parseUploadRecord,
  type UploadRecord,
  type UploadAction,
  type UploadRequest,
} from '../core/file-models'
import { browserUploadSource } from '../core/file-source'
import type { UploadSource } from '../core/files'
import { dateText, errorText, notifyError } from '../ui'
import FileCategorySelect from './FileCategorySelect.vue'
import FileStorageSelect from './FileStorageSelect.vue'
const props = defineProps<{ active: boolean }>(),
  emit = defineEmits<{ published: [] }>(),
  session = useSession()
const scope = { endpoint: session.settings.apiBaseUrl, actorId: session.user!.id }
let alive = true,
  sequence = 0,
  selectionSequence = 0,
  selectionControl: AbortController | null = null,
  timer: ReturnType<typeof setInterval> | undefined
const current = () =>
  alive && session.user?.id === scope.actorId && session.settings.apiBaseUrl === scope.endpoint
const drafts = ref<UploadDraft[]>([]),
  journalError = ref(''),
  serverRows = ref<UploadRecord[]>([]),
  liveRows = ref(new Map<string, UploadRecord>()),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  loading = ref(false),
  loadError = ref(''),
  transfers = ref(new Set<string>()),
  actions = ref(new Set<string>()),
  errors = ref(new Map<string, string>()),
  progress = ref(new Map<string, number>()),
  sourceControls = new Map<string, AbortController>()
let journal: UploadJournal | undefined
try {
  journal = new UploadJournal(window.localStorage, scope)
  drafts.value = journal.list()
} catch (error) {
  journalError.value = errorText(error)
}
interface Item {
  key: string
  draft: UploadDraft | null
  record: UploadRecord | null
  name: string
  size: number
  createdAt: string
}
function syncDrafts() {
  if (!journal) return
  try {
    drafts.value = journal.list()
  } catch (error) {
    journalError.value = errorText(error)
  }
}
function receive(row: UploadRecord) {
  if (!current()) return
  const before = liveRows.value.get(row.requestId)
  liveRows.value.set(row.requestId, row)
  if (row.state === 'completed') errors.value.delete(row.requestId)
  if (row.state === 'completed' && before?.state !== 'completed') emit('published')
  syncDrafts()
}
const api: UploadTransport = {
  request: (path, options) => session.request(path, options),
  putChunk: (id, index, bytes, sha256, options) =>
    session.putChunk(id, index, bytes, sha256, options),
}
const controller = new UploadController(api, scope, {
  journal,
  isCurrent: current,
  onUpdate: receive,
  onProgress: (id, bytes) => {
    if (current()) progress.value.set(id, bytes)
  },
})
const items = computed(() => {
  const result = new Map<string, Item>()
  for (const row of serverRows.value) {
    const record = liveRows.value.get(row.requestId) ?? row
    result.set(row.requestId, {
      key: row.requestId,
      draft: null,
      record,
      name: record.name,
      size: record.size,
      createdAt: record.createdAt,
    })
  }
  for (const draft of drafts.value) {
    const record = liveRows.value.get(draft.request.requestId) ?? null
    result.set(draft.request.requestId, {
      key: draft.request.requestId,
      draft,
      record,
      name: draft.request.name,
      size: draft.request.size,
      createdAt: record?.createdAt ?? draft.createdAt,
    })
  }
  return [...result.values()].sort((a, b) => b.createdAt.localeCompare(a.createdAt))
})
const busy = (key: string) => transfers.value.has(key) || actions.value.has(key)
function sizeText(value: number) {
  return value < 1024
    ? `${value} B`
    : value < 1024 * 1024
      ? `${(value / 1024).toFixed(1)} KiB`
      : value < 1024 * 1024 * 1024
        ? `${(value / 1024 / 1024).toFixed(1)} MiB`
        : `${(value / 1024 / 1024 / 1024).toFixed(2)} GiB`
}
function status(item: Item) {
  if (!item.record) return transfers.value.has(item.key) ? '正在创建' : '创建待确认'
  if (transfers.value.has(item.key) && item.record.state === 'receiving') return '上传中'
  return (
    {
      receiving: '待续传',
      queued: '处理中',
      verifying: '校验中',
      publishing: '发布中',
      reconciling: '核对结果',
      unknown: '结果待确认',
      failed: '未完成',
      cancelRequested: '取消待确认',
      cancelled: '已取消',
      completed: '已发布',
    } as const
  )[item.record.state]
}
function percent(item: Item) {
  const bytes = progress.value.get(item.key) ?? item.record?.receivedBytes ?? 0
  return item.size
    ? Math.min(100, Math.round((bytes / item.size) * 100))
    : item.record?.state === 'completed'
      ? 100
      : 0
}
function reason(item: Item) {
  if (errors.value.has(item.key)) return errors.value.get(item.key)
  const code = item.record?.lastError
  return code
    ? ((
        {
          CHECKSUM_MISMATCH: '整份文件校验失败，请取消后重新新建上传',
          CHUNK_MISSING: '部分内容需要重新选择原文件补传',
          CHUNK_CORRUPT: '部分内容需要重新选择原文件补传',
          PERSISTENCE_PENDING: '正在确认已接收内容的发布结果',
          STORAGE_OUTCOME_UNKNOWN: '发布结果待确认，请先查询',
          STORAGE_UNAVAILABLE: '存储暂不可用，可修复后再次发布',
          PERMISSION_REVOKED: '写入权限已变化，请核对账号权限',
        } as Record<string, string>
      )[code] ?? '任务尚未完成，请刷新核对')
    : ''
}
async function load() {
  const seq = ++sequence
  loading.value = true
  loadError.value = ''
  syncDrafts()
  try {
    const response = await session.request<unknown[]>(
      `/uploads?${new URLSearchParams({ page: String(page.value), pageSize: String(pageSize.value), search: search.value, sort: '-createdAt' })}`,
    )
    if (!current() || seq !== sequence) return
    const rows = response.data.map((value) => parseUploadRecord(value, scope.actorId))
    serverRows.value = rows
    total.value = response.meta?.total ?? 0
    for (const row of rows) {
      receive(row)
      const draft = drafts.value.find((value) => value.request.requestId === row.requestId)
      if (draft && !draft.uploadId && journal && !journalError.value) {
        try {
          parseUploadRecord(row, scope.actorId, draft.request)
          journal.bind(row.requestId, row.id)
        } catch (error) {
          journalError.value = errorText(error)
        }
      }
    }
    const present = new Set(rows.map((row) => row.id)),
      missing = drafts.value.filter((row) => row.uploadId && !present.has(row.uploadId))
    await Promise.allSettled(
      missing.map(async (draft) => {
        const row = parseUploadRecord(
          (await session.request<unknown>(`/uploads/${draft.uploadId}`)).data,
          scope.actorId,
          draft.request,
          true,
        )
        if (current() && seq === sequence) receive(row)
      }),
    )
    syncDrafts()
  } catch (error) {
    if (current() && seq === sequence) loadError.value = errorText(error)
  } finally {
    if (current() && seq === sequence) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
function storageFailure(error: unknown) {
  if (error instanceof ApiError && error.code.startsWith('UPLOAD_JOURNAL'))
    journalError.value = errorText(error)
}
async function inspect(item: Item) {
  if (busy(item.key)) return
  actions.value.add(item.key)
  errors.value.delete(item.key)
  try {
    if (item.draft && !journalError.value && (item.draft.uploadId || !item.record))
      await controller.query(item.key)
    else if (item.record) await controller.queryKnown(item.record.id)
    else await controller.query(item.key)
    syncDrafts()
  } catch (error) {
    storageFailure(error)
    errors.value.set(item.key, errorText(error))
    notifyError(error)
  } finally {
    actions.value.delete(item.key)
  }
}
function pause(item: Item) {
  controller.stop(item.key)
  sourceControls.get(item.key)?.abort()
}
async function act(item: Item, kind: UploadAction) {
  if (actions.value.has(item.key)) return
  if (kind === 'cancel') pause(item)
  actions.value.add(item.key)
  errors.value.delete(item.key)
  try {
    if (item.draft && !journalError.value) await controller.action(item.key, kind)
    else if (item.record) await controller.actionKnown(item.record.id, kind)
    else throw new ApiError('UPLOAD_JOURNAL_UNAVAILABLE', '请先核对本机续传记录')
    ElMessage.success(
      kind === 'cancel'
        ? '取消请求已提交'
        : kind === 'cleanup'
          ? '已核对清理状态'
          : kind === 'recover'
            ? '已核对可恢复内容'
            : '处理请求已提交',
    )
    await load()
  } catch (error) {
    storageFailure(error)
    errors.value.set(item.key, errorText(error))
    notifyError(error)
    void load()
  } finally {
    actions.value.delete(item.key)
    syncDrafts()
  }
}
async function forget(item: Item) {
  if (!journal) return
  try {
    await ElMessageBox.confirm(
      '仅移除本机续传记录，不会取消或删除服务器任务。请先核对任务结果。',
      '移除本机记录',
      { type: 'warning', confirmButtonText: '移除记录', cancelButtonText: '保留' },
    )
    journal.forget(item.key)
    syncDrafts()
    if (!serverRows.value.some((row) => row.requestId === item.key)) liveRows.value.delete(item.key)
  } catch (error) {
    if (error !== 'cancel' && error !== 'close') notifyError(error)
  }
}
const open = ref(false),
  resumeKey = ref<string | null>(null),
  source = shallowRef<UploadSource | null>(null),
  hashing = ref(false),
  hashBytes = ref(0),
  dialogError = ref(''),
  fileInput = ref<HTMLInputElement | null>(null),
  form = reactive({
    name: '',
    categoryId: null as string | null,
    storageProfileId: null as string | null,
  })
async function closeSelection() {
  selectionSequence++
  selectionControl?.abort()
  selectionControl = null
  const old = source.value
  source.value = null
  await old?.close().catch(() => undefined)
  hashing.value = false
}
async function openNew() {
  if (!session.can('files.write')) return
  if (!journal || journalError.value) {
    ElMessage.error(journalError.value || '本机续传记录不可用')
    return
  }
  await closeSelection()
  resumeKey.value = null
  Object.assign(form, { name: '', categoryId: null, storageProfileId: null })
  dialogError.value = ''
  open.value = true
}
function adopt(item: Item): UploadDraft {
  if (!journal || journalError.value)
    throw new ApiError('UPLOAD_JOURNAL_UNAVAILABLE', journalError.value || '本机续传记录不可用')
  if (item.draft) return item.draft
  if (!item.record) throw new ApiError('UPLOAD_DRAFT_MISSING', '上传记录不可用')
  const row = item.record,
    input: UploadRequest = {
      requestId: row.requestId,
      name: row.name,
      mediaType: row.mediaType,
      size: row.size,
      sha256: row.sha256,
      ...(row.categoryId ? { categoryId: row.categoryId } : {}),
      storageProfileId: row.storageProfileId,
    }
  journal.create(input)
  const result = journal.bind(row.requestId, row.id)
  syncDrafts()
  return result
}
async function resume(item: Item) {
  try {
    const draft = adopt(item)
    await closeSelection()
    resumeKey.value = item.key
    Object.assign(form, {
      name: draft.request.name,
      categoryId: draft.request.categoryId ?? null,
      storageProfileId: draft.request.storageProfileId ?? null,
    })
    dialogError.value = ''
    open.value = true
  } catch (error) {
    storageFailure(error)
    notifyError(error)
  }
}
async function select(file?: File) {
  await closeSelection()
  const seq = ++selectionSequence,
    control = new AbortController()
  selectionControl = control
  hashing.value = true
  hashBytes.value = 0
  dialogError.value = ''
  try {
    const selected = platform.files
      ? await platform.files.openSource({
          signal: control.signal,
          onProgress: (value) => {
            if (current() && seq === selectionSequence) hashBytes.value = value
          },
        })
      : file
        ? await browserUploadSource(file, control.signal)
        : null
    if (!selected) return
    if (!current() || seq !== selectionSequence) {
      await selected.close()
      return
    }
    if (resumeKey.value) {
      const draft = journal!.get(resumeKey.value)
      if (selected.size !== draft.request.size || selected.sha256 !== draft.request.sha256) {
        await selected.close()
        throw new ApiError('UPLOAD_SOURCE_MISMATCH', '所选文件与原上传内容不一致，请重新选择')
      }
    } else form.name = selected.name
    source.value = selected
  } catch (error) {
    if (
      current() &&
      !loading.value &&
      seq === selectionSequence &&
      !(error instanceof ApiError && ['REQUEST_CANCELLED', 'SESSION_CHANGED'].includes(error.code))
    )
      dialogError.value = errorText(error)
  } finally {
    if (current() && seq === selectionSequence) hashing.value = false
  }
}
function pick() {
  if (platform.files) void select()
  else fileInput.value?.click()
}
function inputChanged(event: Event) {
  const input = event.target as HTMLInputElement,
    file = input.files?.[0]
  input.value = ''
  if (file) void select(file)
}
async function start() {
  if (!source.value || !journal || !session.can('files.write')) return
  let draft: UploadDraft
  try {
    draft = resumeKey.value
      ? journal.get(resumeKey.value)
      : journal.create({
          requestId: crypto.randomUUID(),
          name: form.name,
          mediaType: source.value.mediaType,
          size: source.value.size,
          sha256: source.value.sha256,
          ...(form.categoryId ? { categoryId: form.categoryId } : {}),
          ...(form.storageProfileId ? { storageProfileId: form.storageProfileId } : {}),
        })
  } catch (error) {
    storageFailure(error)
    dialogError.value = errorText(error)
    return
  }
  const selected = source.value,
    control = selectionControl
  source.value = null
  selectionControl = null
  open.value = false
  syncDrafts()
  transfers.value.add(draft.request.requestId)
  if (control) sourceControls.set(draft.request.requestId, control)
  errors.value.delete(draft.request.requestId)
  try {
    const row = await controller.transfer(draft.request.requestId, selected)
    if (current())
      ElMessage.success(row.state === 'completed' ? '文件已发布' : '文件已接收，正在核对发布结果')
  } catch (error) {
    if (
      current() &&
      !(error instanceof ApiError && ['REQUEST_CANCELLED', 'SESSION_CHANGED'].includes(error.code))
    ) {
      storageFailure(error)
      errors.value.set(draft.request.requestId, errorText(error))
      notifyError(error)
    }
  } finally {
    control?.abort()
    sourceControls.delete(draft.request.requestId)
    if (current()) {
      transfers.value.delete(draft.request.requestId)
      syncDrafts()
      void load()
    }
  }
}
function resumable(item: Item) {
  return (
    !item.record ||
    (['receiving', 'failed'].includes(item.record.state) &&
      item.record.receivedCount < item.record.totalChunks &&
      item.record.lastError !== 'CHECKSUM_MISMATCH')
  )
}
function publishable(item: Item) {
  return (
    !!item.record &&
    ['receiving', 'failed'].includes(item.record.state) &&
    item.record.receivedCount === item.record.totalChunks &&
    item.record.lastError !== 'CHECKSUM_MISMATCH'
  )
}
function cancellable(item: Item) {
  return !item.record || !['completed', 'cancelled', 'cancelRequested'].includes(item.record.state)
}
defineExpose({ openNew })
onMounted(() => {
  void load()
  timer = setInterval(() => {
    if (
      current() &&
      (props.active ||
        transfers.value.size ||
        items.value.some(
          (item) =>
            item.record &&
            [
              'queued',
              'verifying',
              'publishing',
              'reconciling',
              'unknown',
              'cancelRequested',
            ].includes(item.record.state),
        ))
    )
      void load()
  }, 2000)
})
watch(
  () => session.can('files.write'),
  (value) => {
    if (!value) {
      for (const item of items.value) pause(item)
      void closeSelection()
      open.value = false
    }
  },
)
onBeforeUnmount(() => {
  alive = false
  sequence++
  if (timer) clearInterval(timer)
  controller.dispose()
  selectionControl?.abort()
  void source.value?.close()
  for (const control of sourceControls.values()) control.abort()
})
</script>
<template>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <el-input
        v-model="search"
        class="search-input"
        clearable
        placeholder="按上传名称前缀搜索"
        @keyup.enter="query"
      /><el-button @click="query">查询</el-button>
      <div class="toolbar-spacer" />
      <span class="subtle">仅显示当前账号的上传任务</span
      ><el-button text :icon="Refresh" @click="load">刷新</el-button>
    </div>
    <el-alert
      v-if="journalError"
      :title="journalError"
      description="新建和续传暂不可用；已有任务仍可查询或取消。"
      type="warning"
      :closable="false"
    /><el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-table class="file-uploads-table" :data="items" v-loading="loading" row-key="key"
      ><el-table-column label="上传文件" min-width="220"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><small class="table-secondary">{{ sizeText(row.size) }}</small
          ><small v-if="reason(row as Item)" class="field-error">{{
            reason(row as Item)
          }}</small></template
        ></el-table-column
      ><el-table-column label="进度" min-width="150"
        ><template #default="{ row }"
          ><el-progress
            :percentage="percent(row as Item)"
            :status="
              row.record?.state === 'completed'
                ? 'success'
                : row.record?.state === 'failed'
                  ? 'exception'
                  : undefined
            " /></template></el-table-column
      ><el-table-column label="状态" min-width="150"
        ><template #default="{ row }"
          ><el-tag
            :type="
              row.record?.state === 'completed'
                ? 'success'
                : row.record?.state === 'unknown'
                  ? 'warning'
                  : 'info'
            "
            >{{ status(row as Item) }}</el-tag
          ><small v-if="row.draft?.pendingAction" class="table-secondary">上次操作待确认</small
          ><small v-if="row.record?.cleanupPending" class="table-secondary"
            >清理待完成</small
          ></template
        ></el-table-column
      ><el-table-column label="创建时间" min-width="160"
        ><template #default="{ row }">{{ dateText(row.createdAt) }}</template></el-table-column
      ><el-table-column label="操作" min-width="270" fixed="right"
        ><template #default="{ row }">
          <el-button link :disabled="busy(row.key)" @click="inspect(row as Item)"
            >查询结果</el-button
          ><template v-if="session.can('files.write')"
            ><el-button
              v-if="resumable(row as Item)"
              link
              type="primary"
              :disabled="busy(row.key) || !!journalError"
              @click="resume(row as Item)"
              >继续上传</el-button
            ><el-button v-if="transfers.has(row.key)" link @click="pause(row as Item)"
              >暂停</el-button
            ><el-button
              v-if="publishable(row as Item)"
              link
              type="primary"
              :disabled="busy(row.key)"
              @click="act(row as Item, 'complete')"
              >发布</el-button
            ><el-button
              v-if="row.record?.state === 'unknown'"
              link
              type="primary"
              :disabled="busy(row.key)"
              @click="act(row as Item, 'retry')"
              >重试发布</el-button
            ><el-button
              v-if="row.record && ['receiving', 'failed'].includes(row.record.state)"
              link
              :disabled="busy(row.key)"
              @click="act(row as Item, 'recover')"
              >核对分片</el-button
            ><el-button
              v-if="cancellable(row as Item)"
              link
              type="danger"
              :disabled="actions.has(row.key)"
              @click="act(row as Item, 'cancel')"
              >取消上传</el-button
            ><el-button
              v-if="row.record?.cleanupPending"
              link
              :disabled="busy(row.key)"
              @click="act(row as Item, 'cleanup')"
              >重试清理</el-button
            ></template
          ><el-button v-if="row.draft" link :disabled="busy(row.key)" @click="forget(row as Item)"
            >移除本机记录</el-button
          >
        </template></el-table-column
      ></el-table
    >
    <div class="pagination-row">
      <span class="subtle">服务器 {{ total }} 个任务 · 本机 {{ drafts.length }} 条续传记录</span
      ><el-pagination
        v-model:current-page="page"
        v-model:page-size="pageSize"
        :page-sizes="[20, 50, 100]"
        :total="total"
        layout="sizes, prev, pager, next"
        @current-change="load"
        @size-change="query"
      />
    </div>
  </section>
  <el-dialog
    v-model="open"
    :title="resumeKey ? '继续上传' : '新建上传'"
    width="600px"
    :close-on-click-modal="false"
    @closed="closeSelection"
    ><el-alert v-if="dialogError" :title="dialogError" type="error" :closable="false" />
    <div class="file-picker">
      <input
        ref="fileInput"
        data-testid="upload-source"
        type="file"
        class="file-source-input"
        aria-label="上传源文件"
        @change="inputChanged"
      /><el-button :icon="UploadFilled" :loading="hashing" @click="pick">{{
        source ? '重新选择文件' : '选择文件'
      }}</el-button
      ><span v-if="hashing" class="subtle"
        >正在校验内容 {{ hashBytes ? sizeText(hashBytes) : '' }}</span
      ><el-tag v-else-if="source" type="success">文件已校验</el-tag>
      <p v-if="source">{{ source.name }} · {{ sizeText(source.size) }}</p>
      <p v-else class="subtle">
        {{
          resumeKey
            ? '重新选择原文件，核验后继续已接收的进度。'
            : '选择文件后校验内容，再开始上传。'
        }}
      </p>
    </div>
    <el-form label-position="top"
      ><el-form-item label="文件名称"
        ><el-input v-model="form.name" :disabled="!!resumeKey" /></el-form-item
      ><el-form-item label="文件分类"
        ><FileCategorySelect v-model="form.categoryId" :disabled="!!resumeKey" /></el-form-item
      ><el-form-item v-if="session.can('files.write') && !resumeKey" label="文件存储"
        ><FileStorageSelect v-model="form.storageProfileId" /></el-form-item></el-form
    ><template #footer
      ><el-button @click="open = false">取消</el-button
      ><el-button type="primary" :disabled="!source || hashing" @click="start">{{
        resumeKey ? '继续上传' : '开始上传'
      }}</el-button></template
    ></el-dialog
  >
</template>
