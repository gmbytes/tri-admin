<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { useSession } from '../stores/session'
import { platform } from '../platform'
import { ApiError } from '../core/client'
import { BROWSER_FILE_LIMIT, createBrowserFileSink, type FileSink } from '../core/files'
import type { NativeSaveResult } from '../core/file-native'
import type { DataFormat, DataResource } from '../core/data-tools'
import {
  ExportRecoveryStore,
  exportInput,
  parseExportJob,
  parseExportPage,
  parseExportAction,
  validExportRequest,
  type ExportSource,
  type ExportJob,
  type ExportOperation,
  exportStates,
} from '../core/export-jobs'
import { dateText, errorText } from '../ui'
const props = defineProps<{
    resources: DataResource[]
    source: ExportSource | null
    format: DataFormat
    disabled: boolean
  }>(),
  session = useSession()
const rows = ref<ExportJob[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  resource = ref(''),
  state = ref(''),
  loading = ref(false),
  writing = ref(false),
  error = ref(''),
  listError = ref(''),
  recoveryError = ref(''),
  pending = ref<ExportOperation[]>([]),
  maxRows = ref(100000),
  request = ref(''),
  detail = ref<ExportJob | null>(null),
  downloadId = ref(''),
  downloadBytes = ref(0)
const labels: Record<string, string> = {
  queued: '排队中',
  running: '生成中',
  publishing: '发布中',
  reconciling: '核对结果',
  unknown: '待确认',
  completed: '已完成',
  failed: '失败',
  cancelled: '已取消',
  expired: '已过期',
}
const reasons: Record<string, string> = {
  ROW_LIMIT: '超过本次行数上限',
  PERMISSION_REVOKED: '执行权限已撤销',
  SCOPE_CHANGED: '数据范围已变化',
  PUBLICATION_PENDING: '正在确认存储结果',
  WORKSPACE_BUSY: '工作目录仍在使用',
  WORKSPACE_UNAVAILABLE: '工作目录暂不可用，等待重试',
  WORKSPACE_UNCONFIRMED: '工作目录归属未确认',
  FILE_STORAGE_UNAVAILABLE: '存储暂不可用',
  EXPORT_FAILED: '导出生成失败',
  LEASE_LOST: '执行已停止',
}
const operationLabels = { create: '创建', retry: '重试', cancel: '取消' }
const scope = computed(() => `${session.settings.apiBaseUrl}|${session.user?.id ?? ''}`),
  active = (job: ExportJob) =>
    ['queued', 'running', 'publishing', 'reconciling', 'unknown'].includes(job.state)
const blocked = computed(() => props.disabled || writing.value),
  canCreate = computed(
    () =>
      !!props.source &&
      session.can('exports.write') &&
      !blocked.value &&
      !recoveryError.value &&
      Number.isSafeInteger(maxRows.value) &&
      maxRows.value >= 1 &&
      maxRows.value <= 1000000 &&
      !pending.value.some((value) => value.kind === 'create'),
  )
let store: ExportRecoveryStore | null = null,
  alive = true,
  epoch = 0,
  sequence = 0,
  current: AbortController | null = null,
  downloadControl: AbortController | null = null
const controls = new Set<AbortController>()
function loadPending() {
  if (!store) return
  try {
    pending.value = store.read()
    recoveryError.value = ''
  } catch (failure) {
    pending.value = []
    recoveryError.value = errorText(failure)
  }
}
function resetScope() {
  epoch++
  current?.abort()
  for (const control of controls) control.abort()
  downloadControl?.abort()
  downloadControl = null
  downloadId.value = ''
  downloadBytes.value = 0
  writing.value = false
  rows.value = []
  total.value = 0
  detail.value = null
  pending.value = []
  store = null
  recoveryError.value = ''
  error.value = ''
  listError.value = ''
  if (session.user)
    try {
      store = new ExportRecoveryStore(localStorage, session.settings.apiBaseUrl, session.user.id)
      loadPending()
    } catch (failure) {
      recoveryError.value = errorText(failure)
    }
  void load()
}
async function load() {
  if (!session.user) return
  const token = ++sequence
  current?.abort()
  const control = new AbortController()
  current = control
  loading.value = true
  listError.value = ''
  const expected = {
    page: page.value,
    pageSize: pageSize.value,
    resource: resource.value,
    state: state.value,
  }
  const parameters = new URLSearchParams({
    page: String(page.value),
    pageSize: String(pageSize.value),
  })
  if (resource.value) parameters.set('resource', resource.value)
  if (state.value) parameters.set('state', state.value)
  try {
    const result = await session.request<unknown[]>('/data-tools/exports?' + parameters, {
      signal: control.signal,
    })
    if (
      !alive ||
      token !== sequence ||
      control.signal.aborted ||
      page.value !== expected.page ||
      pageSize.value !== expected.pageSize ||
      resource.value !== expected.resource ||
      state.value !== expected.state
    )
      return
    const parsed = parseExportPage(result, expected)
    rows.value = parsed.rows
    total.value = parsed.total
    const updated = parsed.rows.find((row) => row.id === detail.value?.id)
    if (updated && (!detail.value || updated.version >= detail.value.version))
      detail.value = updated
  } catch (failure) {
    if (alive && token === sequence && !control.signal.aborted) listError.value = errorText(failure)
  } finally {
    if (alive && token === sequence) loading.value = false
  }
}
function refresh() {
  if (page.value !== 1) page.value = 1
  else void load()
}
async function perform(
  action: (signal: AbortSignal, recovery: ExportRecoveryStore) => Promise<void>,
) {
  if (blocked.value || !store) return
  const stamp = epoch,
    control = new AbortController(),
    recovery = store
  controls.add(control)
  writing.value = true
  error.value = ''
  try {
    await action(control.signal, recovery)
  } catch (failure) {
    if (alive && stamp === epoch && !control.signal.aborted) error.value = errorText(failure)
  } finally {
    controls.delete(control)
    if (alive && stamp === epoch) {
      writing.value = false
      loadPending()
      void load()
    }
  }
}
function record(kind: ExportOperation['kind'], jobId: string | null): ExportOperation {
  return { requestId: crypto.randomUUID(), kind, jobId, createdAt: new Date().toISOString() }
}
async function create() {
  if (!canCreate.value || !props.source) return
  const operation = record('create', null)
  let body: ReturnType<typeof exportInput>
  try {
    body = exportInput(props.source, props.format, maxRows.value, operation.requestId)
  } catch (failure) {
    error.value = errorText(failure)
    return
  }
  await perform(async (signal, recovery) => {
    recovery.put(operation)
    loadPending()
    try {
      const result = await session.request<unknown>('/data-tools/exports', {
        method: 'POST',
        body,
        signal,
      })
      if (signal.aborted) return
      detail.value = parseExportJob(result.data)
      recovery.remove(operation.requestId)
      refresh()
      ElMessage.success('任务已创建，可查询后台进度')
    } catch (failure) {
      if (
        failure instanceof ApiError &&
        [400, 403, 404, 413, 415, 422, 429].includes(failure.status)
      )
        recovery.remove(operation.requestId)
      throw failure
    }
  })
}
async function queryJob(id: string) {
  await perform(async (signal) => {
    const result = await session.request<unknown>(`/data-tools/exports/${id}`, { signal })
    if (!signal.aborted) detail.value = parseExportJob(result.data)
  })
}
async function queryRequest() {
  if (!validExportRequest(request.value)) {
    error.value = '请输入完整的请求 UUID'
    return
  }
  await perform(async (signal) => {
    const result = await session.request<unknown>(
      `/data-tools/exports/by-request/${request.value}`,
      { signal },
    )
    if (!signal.aborted) detail.value = parseExportJob(result.data)
  })
}
async function recover(operation: ExportOperation) {
  await perform(async (signal, recovery) => {
    if (operation.kind === 'create') {
      const result = await session.request<unknown>(
        `/data-tools/exports/by-request/${operation.requestId}`,
        { signal },
      )
      if (signal.aborted) return
      const job = parseExportJob(result.data)
      if (job.requestId !== operation.requestId) throw new Error('请求回执不匹配')
      detail.value = job
      recovery.remove(operation.requestId)
    } else {
      if (operation.kind === 'retry') {
        const result = await session.request<unknown>(
          `/data-tools/exports/${operation.jobId}/actions/${operation.requestId}`,
          { signal },
        )
        const action = parseExportAction(result.data)
        if (action.jobId !== operation.jobId || action.requestId !== operation.requestId)
          throw new Error('操作回执不匹配')
      }
      const result = await session.request<unknown>(`/data-tools/exports/${operation.jobId}`, {
        signal,
      })
      if (signal.aborted) return
      const job = parseExportJob(result.data)
      detail.value = job
      if (operation.kind === 'retry' || !active(job)) recovery.remove(operation.requestId)
      else error.value = '取消结果尚未确认，可继续查询或再次明确取消。'
    }
  })
}
async function retry(job: ExportJob) {
  const expected = epoch
  if (
    !(await confirm('重试会先确认原结果；必要时按原条件和当前权限重新生成。', '重试导出')) ||
    !alive ||
    expected !== epoch
  )
    return
  const operation = record('retry', job.id)
  await perform(async (signal, recovery) => {
    recovery.put(operation)
    try {
      const result = await session.request<unknown>(`/data-tools/exports/${job.id}/retry`, {
        method: 'POST',
        body: { requestId: operation.requestId },
        signal,
      })
      if (signal.aborted) return
      detail.value = parseExportJob(result.data)
      recovery.remove(operation.requestId)
    } catch (failure) {
      if (failure instanceof ApiError && [400, 403, 404, 409, 422, 429].includes(failure.status))
        recovery.remove(operation.requestId)
      throw failure
    }
  })
}
async function cancel(job: ExportJob) {
  const expected = epoch
  if (
    !(await confirm('确认取消此后台导出？已完成的任务将保留结果。', '取消导出')) ||
    !alive ||
    expected !== epoch
  )
    return
  const operation = record('cancel', job.id)
  await perform(async (signal, recovery) => {
    recovery.put(operation)
    try {
      const result = await session.request<unknown>(`/data-tools/exports/${job.id}/cancel`, {
        method: 'POST',
        body: {},
        signal,
      })
      if (signal.aborted) return
      detail.value = parseExportJob(result.data)
      recovery.remove(operation.requestId)
    } catch (failure) {
      if (failure instanceof ApiError && [400, 403, 404, 409, 422].includes(failure.status))
        recovery.remove(operation.requestId)
      throw failure
    }
  })
}
async function forget(operation: ExportOperation) {
  const expected = epoch
  if (
    !(await confirm('仅移除本机待确认标记，服务器任务可在历史中继续查询。', '移除本机标记')) ||
    !alive ||
    expected !== epoch
  )
    return
  store?.remove(operation.requestId)
  loadPending()
}
async function clearRecovery() {
  const expected = epoch
  if (
    !(await confirm('清除本机恢复记录后，可在服务器历史中查询任务。', '清除恢复记录')) ||
    !alive ||
    expected !== epoch
  )
    return
  store?.clear()
  loadPending()
}
async function confirm(message: string, title: string) {
  try {
    await ElMessageBox.confirm(message, title, { type: 'warning' })
    return true
  } catch {
    return false
  }
}
async function download(job: ExportJob) {
  if (!job.file || job.state !== 'completed' || blocked.value) return
  const file = Object.freeze({ ...job.file })
  await perform(async (signal) => {
    downloadId.value = job.id
    downloadBytes.value = 0
    const local = new AbortController()
    downloadControl = local
    const combined = AbortSignal.any([signal, local.signal])
    let sink: FileSink<Blob | NativeSaveResult> | null = null
    try {
      sink = platform.files
        ? await platform.files.chooseTarget(file.name, file, { signal: combined })
        : createBrowserFileSink(file)
      if (!sink) return
      const saved = await session.downloadExport(job.id, file, sink, {
        signal: combined,
        onProgress: (bytes) => {
          if (!combined.aborted) downloadBytes.value = bytes
        },
      })
      if (combined.aborted) return
      if (saved instanceof Blob) {
        const url = URL.createObjectURL(new Blob([saved], { type: file.mimeType })),
          link = document.createElement('a')
        link.href = url
        link.download = file.name
        document.body.append(link)
        link.click()
        link.remove()
        setTimeout(() => URL.revokeObjectURL(url), 1000)
      } else if (saved.cleanupPending) ElMessage.warning('结果已保存，临时清理将重试')
      else ElMessage.success('结果已保存')
    } finally {
      if (downloadControl === local) {
        downloadControl = null
        downloadId.value = ''
      }
    }
  })
}
watch(scope, resetScope)
watch([resource, state, pageSize], () => {
  page.value = 1
})
watch([page, pageSize, resource, state], () => void load(), { flush: 'post' })
const ticker = setInterval(() => {
  if (alive && !writing.value && !loading.value && rows.value.some(active)) void load()
}, 3000)
onMounted(resetScope)
onBeforeUnmount(() => {
  alive = false
  epoch++
  sequence++
  current?.abort()
  for (const control of controls) control.abort()
  downloadControl?.abort()
  clearInterval(ticker)
})
function size(bytes: number) {
  return bytes < 1024 * 1024
    ? `${(bytes / 1024).toFixed(1)} KiB`
    : `${(bytes / 1024 / 1024).toFixed(1)} MiB`
}
</script>
<template>
  <section class="surface export-jobs-panel">
    <div class="job-heading">
      <div>
        <h2>持久导出任务</h2>
        <p class="tiny subtle">
          使用上方资源、字段或所选模板在后台生成。结果保留 24 小时，权限和数据范围在下载时重新检查。
        </p>
      </div>
      <el-button :disabled="writing" @click="load">刷新任务</el-button>
    </div>
    <div v-if="session.can('exports.write')" class="job-create">
      <el-form-item label="后台行数上限"
        ><el-input-number
          v-model="maxRows"
          aria-label="后台行数上限"
          :min="1"
          :max="1000000"
          :step="1000"
          :disabled="blocked" /></el-form-item
      ><el-button
        type="primary"
        :disabled="!canCreate"
        :loading="writing && !downloadId"
        @click="create"
        >创建后台任务</el-button
      ><span class="tiny subtle"
        >超过上限会明确失败；最多 5 GiB。{{
          platform.native
            ? '使用桌面磁盘保存结果。'
            : `浏览器保存上限 ${BROWSER_FILE_LIMIT / 1024 / 1024} MiB，较大结果请使用桌面程序。`
        }}</span
      >
    </div>
    <el-alert v-if="error" :title="error" type="error" :closable="false" class="job-error" />
    <el-alert
      v-if="listError"
      :title="listError"
      type="error"
      :closable="false"
      class="job-error"
    />
    <el-alert v-if="recoveryError" :title="recoveryError" type="error" :closable="false"
      ><el-button @click="clearRecovery">清除恢复记录</el-button></el-alert
    >
    <div v-if="pending.length" class="job-pending">
      <strong>待确认操作</strong>
      <div v-for="operation in pending" :key="operation.requestId" class="pending-line">
        <span>{{ operationLabels[operation.kind] }} · {{ operation.requestId }}</span
        ><el-button :disabled="blocked" @click="recover(operation)">恢复查询</el-button
        ><el-button text :disabled="blocked" @click="forget(operation)">移除本机标记</el-button>
      </div>
    </div>
    <div class="job-filters">
      <el-select v-model="resource" aria-label="任务资源" placeholder="全部资源"
        ><el-option label="全部资源" value="" /><el-option
          v-for="item in resources"
          :key="item.key"
          :label="item.label"
          :value="item.key" /></el-select
      ><el-select v-model="state" aria-label="任务状态" placeholder="全部状态"
        ><el-option label="全部状态" value="" /><el-option
          v-for="value in exportStates"
          :key="value"
          :label="labels[value]"
          :value="value" /></el-select
      ><el-input
        v-model="request"
        aria-label="导出请求标识"
        placeholder="按请求 UUID 查询"
        maxlength="36"
      /><el-button :disabled="blocked" @click="queryRequest">查询请求</el-button>
    </div>
    <div v-if="detail" class="job-detail tiny">
      <strong>当前查询：{{ labels[detail.state] }}</strong> · {{ detail.id }} ·
      {{ detail.rowCount }} / {{ detail.maxRows
      }}<span v-if="detail.lastError"> · {{ reasons[detail.lastError] ?? detail.lastError }}</span
      ><span v-if="detail.cleanupPending"> · 仍有内容待确认或清理</span>
    </div>
    <div v-if="downloadId" class="job-download">
      <span>已接收 {{ size(downloadBytes) }}</span
      ><el-button @click="downloadControl?.abort()">停止下载</el-button>
    </div>
    <el-table v-loading="loading" :data="rows" row-key="id"
      ><el-table-column label="资源 / 请求标识" min-width="200"
        ><template #default="{ row }"
          ><div>
            {{ resources.find((item) => item.key === row.resource)?.label ?? row.resource }} ·
            {{ row.format.toUpperCase() }}
          </div>
          <div class="tiny subtle job-id">{{ row.requestId }}</div>
          <div v-if="row.template" class="tiny">
            {{ row.template.name }} · v{{ row.template.version }}
          </div></template
        ></el-table-column
      ><el-table-column label="状态" min-width="115"
        ><template #default="{ row }"
          ><el-tag
            :type="
              row.state === 'failed' ? 'danger' : row.state === 'completed' ? 'success' : 'info'
            "
            >{{ labels[row.state] }}</el-tag
          >
          <div v-if="row.lastError" class="tiny subtle">
            {{ reasons[row.lastError] ?? row.lastError }}
          </div>
          <div v-if="row.cleanupPending" class="tiny subtle">待确认或清理</div></template
        ></el-table-column
      ><el-table-column label="行数 / 上限" min-width="130"
        ><template #default="{ row }"
          >{{ row.rowCount }} / {{ row.maxRows }}</template
        ></el-table-column
      ><el-table-column label="结果" min-width="85"
        ><template #default="{ row }">{{
          row.file ? size(row.file.size) : '—'
        }}</template></el-table-column
      ><el-table-column label="创建时间" min-width="170"
        ><template #default="{ row }">{{ dateText(row.createdAt) }}</template></el-table-column
      ><el-table-column label="操作" min-width="220" fixed="right"
        ><template #default="{ row }"
          ><el-button text :disabled="blocked" @click="queryJob(row.id)">查询任务</el-button
          ><el-button
            v-if="row.state === 'completed' && row.file"
            text
            :disabled="blocked"
            @click="download(row as ExportJob)"
            >下载结果</el-button
          ><template v-if="session.can('exports.write')"
            ><el-button
              v-if="active(row as ExportJob)"
              text
              :disabled="blocked"
              @click="cancel(row as ExportJob)"
              >取消任务</el-button
            ><el-button
              v-if="['failed', 'unknown'].includes(row.state)"
              text
              :disabled="blocked || !!recoveryError"
              @click="retry(row as ExportJob)"
              >重试任务</el-button
            ></template
          ></template
        ></el-table-column
      ></el-table
    >
    <el-pagination
      v-model:current-page="page"
      v-model:page-size="pageSize"
      :page-sizes="[10, 20, 50, 100]"
      :total="total"
      layout="total, sizes, prev, pager, next"
    />
  </section>
</template>
<style scoped>
.export-jobs-panel {
  padding: 24px;
  margin-bottom: 20px;
}
.job-heading {
  display: flex;
  justify-content: space-between;
  gap: 20px;
}
.job-heading h2 {
  margin: 0 0 12px;
  font-size: 18px;
}
.job-create,
.job-filters,
.pending-line,
.job-download {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin: 18px 0;
}
.job-create .el-form-item {
  margin: 0;
}
.job-filters > .el-select {
  width: 170px;
}
.job-filters > .el-input {
  width: 300px;
}
.job-error {
  margin: 16px 0;
}
.job-id {
  overflow-wrap: anywhere;
}
.job-pending,
.job-detail {
  padding: 14px 18px;
  background: var(--el-fill-color-light);
  border-radius: 8px;
  margin: 16px 0;
}
.pending-line {
  margin: 10px 0;
}
.pending-line > span {
  overflow-wrap: anywhere;
}
.el-pagination {
  margin-top: 18px;
  justify-content: flex-end;
}
</style>
