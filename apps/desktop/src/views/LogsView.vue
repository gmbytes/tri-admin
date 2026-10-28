<script setup lang="ts">
import { onMounted, onUnmounted, reactive, ref } from 'vue'
import { Refresh, Delete } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import type { LogKind, LogRecord, LogCleanupRequest, LogCleanupResult } from '../core/types'
import {
  loadPendingCleanups,
  savePendingCleanup,
  completePendingCleanup,
  parseCleanupResult,
  type CleanupScope,
} from '../core/log-cleanup'
import { dateText, errorText } from '../ui'

const session = useSession(),
  kind = ref<LogKind>('operation'),
  rows = ref<LogRecord[]>([]),
  loading = ref(false),
  loadError = ref('')
const page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  sort = ref('-createdAt')
const filters = reactive({
  action: '',
  username: '',
  actorId: '',
  requestId: '',
  outcome: 'all',
  status: '',
  from: '',
  to: '',
})
const kinds: Record<LogKind, string> = {
  login: '登录日志',
  operation: '操作日志',
  error: '错误日志',
}
const outcomes: Record<string, string> = {
  success: '成功',
  failure: '失败',
  blocked: '已阻断',
  revoked: '已撤销',
}
const detail = ref<LogRecord | null>(null),
  detailOpen = ref(false)
const pending = ref<LogCleanupRequest[]>([]),
  journalError = ref(''),
  cleanupOpen = ref(false),
  clearing = ref(false),
  cleanupError = ref('')
const cleanupKind = ref<LogKind>('operation'),
  cleanupForm = reactive({ before: '', limit: 1000 })
const pendingRequest = ref<LogCleanupRequest | null>(null),
  cleanupResult = ref<LogCleanupResult | null>(null)
let sequence = 0,
  attemptScope: CleanupScope | null = null
function scope(): CleanupScope | null {
  return session.user ? { endpoint: session.settings.apiBaseUrl, actorId: session.user.id } : null
}
function refreshPending() {
  try {
    const current = scope()
    pending.value = current ? loadPendingCleanups(localStorage, current) : []
    journalError.value = ''
  } catch {
    journalError.value = '本机清理记录不可用，请先核对待确认请求'
  }
}
function localDate(ms: number) {
  const date = new Date(ms)
  return new Date(ms - date.getTimezoneOffset() * 60000).toISOString().slice(0, 16)
}
function iso(raw: string) {
  const date = new Date(raw)
  if (!Number.isFinite(date.getTime())) throw new Error('请输入有效日期时间')
  return date.toISOString()
}
async function load() {
  const request = ++sequence
  loading.value = true
  loadError.value = ''
  try {
    const query = new URLSearchParams({
      kind: kind.value,
      page: String(page.value),
      pageSize: String(pageSize.value),
      sort: sort.value,
    })
    for (const key of ['action', 'actorId', 'requestId'] as const)
      if (filters[key].trim()) query.set(key, filters[key].trim())
    if (kind.value === 'login' && filters.username.trim())
      query.set('username', filters.username.trim())
    if (filters.outcome !== 'all') query.set('outcome', filters.outcome)
    if (filters.status.trim()) {
      if (!/^[1-5][0-9]{2}$/.test(filters.status.trim())) throw new Error('HTTP 状态需为 100–599')
      query.set('status', filters.status.trim())
    }
    if (filters.from) query.set('from', iso(filters.from))
    if (filters.to) query.set('to', iso(filters.to))
    if (filters.from && filters.to && Date.parse(filters.from) >= Date.parse(filters.to))
      throw new Error('结束时间必须晚于开始时间')
    const response = await session.request<LogRecord[]>(`/logs?${query}`)
    if (request !== sequence) return
    rows.value = response.data
    total.value = response.meta?.total ?? 0
  } catch (error) {
    if (request === sequence) loadError.value = errorText(error)
  } finally {
    if (request === sequence) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
function reset() {
  Object.assign(filters, {
    action: '',
    username: '',
    actorId: '',
    requestId: '',
    outcome: 'all',
    status: '',
    from: '',
    to: '',
  })
  query()
}
function showDetail(row: LogRecord) {
  detail.value = row
  detailOpen.value = true
}
function actor(row: LogRecord) {
  return row.actorName ?? row.username ?? (row.actorId ? '历史用户' : '匿名 / 系统')
}
function openCleanup(next = false) {
  refreshPending()
  if (journalError.value) return
  const current = scope()
  if (!current) return
  attemptScope = current
  cleanupError.value = ''
  cleanupResult.value = null
  const saved = pending.value[0]
  pendingRequest.value = saved ?? null
  cleanupKind.value = saved?.kind ?? (next ? cleanupKind.value : kind.value)
  cleanupForm.before = saved
    ? localDate(Date.parse(saved.before))
    : next
      ? cleanupForm.before
      : localDate(Date.now() - 7 * 86400000)
  cleanupForm.limit = saved?.limit ?? (next ? cleanupForm.limit : 1000)
  cleanupOpen.value = true
}
async function clean() {
  if (clearing.value || !session.can('audit.delete')) return
  cleanupError.value = ''
  const current = scope()
  if (
    !current ||
    !attemptScope ||
    current.endpoint !== attemptScope.endpoint ||
    current.actorId !== attemptScope.actorId
  ) {
    cleanupError.value = '会话已切换，请重新打开清理窗口'
    return
  }
  let input: LogCleanupRequest
  try {
    input = pendingRequest.value ?? {
      requestId: crypto.randomUUID(),
      kind: cleanupKind.value,
      before: iso(cleanupForm.before),
      limit: cleanupForm.limit,
    }
    if (
      !Number.isInteger(input.limit) ||
      input.limit < 1 ||
      input.limit > 1000 ||
      Date.parse(input.before) > Date.now() - 86400000
    )
      throw new Error('只能清理24小时前记录，每批1–1000条')
  } catch (error) {
    cleanupError.value = errorText(error)
    return
  }
  try {
    savePendingCleanup(localStorage, attemptScope, input)
    pendingRequest.value = input
    refreshPending()
  } catch {
    cleanupError.value = '无法保存本机清理记录，本次请求尚未提交'
    return
  }
  clearing.value = true
  try {
    const response = await session.request<unknown>('/logs/cleanup', {
      method: 'POST',
      body: input,
    })
    cleanupResult.value = parseCleanupResult(response.data, input)
    try {
      completePendingCleanup(localStorage, attemptScope, input.requestId)
      pendingRequest.value = null
    } catch {
      cleanupError.value = '清理已完成，本机待确认标记未清除；可重试原请求'
    }
    refreshPending()
    await load()
  } catch (error) {
    cleanupError.value = `结果尚未确认：${errorText(error)}。重试会使用原条件继续完成或确认同一批清理。`
  } finally {
    clearing.value = false
  }
}
function closeCleanup() {
  cleanupOpen.value = false
  refreshPending()
}
onMounted(() => {
  refreshPending()
  void load()
  window.addEventListener('storage', refreshPending)
})
onUnmounted(() => {
  sequence++
  window.removeEventListener('storage', refreshPending)
})
</script>

<template>
  <div class="page-heading">
    <div>
      <h1>日志中心</h1>
      <p>追踪登录与管理操作，通过安全摘要和关联号定位失败请求。</p>
    </div>
    <el-button
      v-if="session.can('audit.delete')"
      :icon="Delete"
      :disabled="!!journalError"
      @click="openCleanup()"
      >清理历史</el-button
    >
  </div>
  <el-alert v-if="journalError" :title="journalError" type="warning" :closable="false" />
  <div v-if="pending.length" class="pending-cleanup surface">
    <div>
      <strong>{{ pending.length }} 个清理请求待确认</strong>
      <p class="subtle">
        {{
          session.can('audit.delete')
            ? '先确认原请求，再开始新的批次。'
            : '需要清理权限才能继续确认原请求。'
        }}
      </p>
    </div>
    <el-button v-if="session.can('audit.delete')" type="primary" plain @click="openCleanup()"
      >继续确认</el-button
    >
  </div>
  <section class="surface table-surface">
    <el-tabs v-model="kind" @tab-change="reset"
      ><el-tab-pane label="操作日志" name="operation" /><el-tab-pane
        label="登录日志"
        name="login" /><el-tab-pane label="错误日志" name="error"
    /></el-tabs>
    <el-form label-position="top" class="log-filters">
      <el-form-item label="动作标识"
        ><el-input
          v-model="filters.action"
          placeholder="动作精确匹配"
          clearable
          maxlength="100"
          @keyup.enter="query"
      /></el-form-item>
      <el-form-item v-if="kind === 'login'" label="账号"
        ><el-input
          v-model="filters.username"
          placeholder="账号精确匹配"
          clearable
          maxlength="64"
          @keyup.enter="query"
      /></el-form-item>
      <el-form-item label="操作者 ID"
        ><el-input
          v-model="filters.actorId"
          placeholder="操作者 ObjectId"
          clearable
          maxlength="24"
          @keyup.enter="query"
      /></el-form-item>
      <el-form-item label="请求关联号"
        ><el-input
          v-model="filters.requestId"
          placeholder="请求关联号 UUID"
          clearable
          maxlength="36"
          @keyup.enter="query"
      /></el-form-item>
      <el-form-item label="结果"
        ><el-select v-model="filters.outcome"
          ><el-option label="全部结果" value="all" /><el-option
            v-for="(label, value) in outcomes"
            :key="value"
            :label="label"
            :value="value" /></el-select
      ></el-form-item>
      <el-form-item label="HTTP 状态"
        ><el-input
          v-model="filters.status"
          placeholder="如 401、503"
          clearable
          maxlength="3"
          @keyup.enter="query"
      /></el-form-item>
      <el-form-item label="开始时间" for="logs-from"
        ><input id="logs-from" v-model="filters.from" type="datetime-local" class="date-input"
      /></el-form-item>
      <el-form-item label="结束时间" for="logs-to"
        ><input id="logs-to" v-model="filters.to" type="datetime-local" class="date-input"
      /></el-form-item>
    </el-form>
    <div class="table-toolbar">
      <el-button type="primary" @click="query">查询</el-button
      ><el-button @click="reset">重置</el-button
      ><small class="subtle">全局安全记录 · 时间范围包含开始、不包含结束</small>
      <div class="toolbar-spacer" />
      <el-select v-model="sort" class="sort-select" aria-label="日志排序" @change="query"
        ><el-option label="最近发生" value="-createdAt" /><el-option
          label="最早发生"
          value="createdAt" /></el-select
      ><el-button :icon="Refresh" text @click="load">刷新</el-button>
    </div>
    <el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-table v-loading="loading" :data="rows" row-key="id" class="logs-table">
      <el-table-column label="时间" min-width="170"
        ><template #default="{ row }">{{ dateText(row.createdAt) }}</template></el-table-column
      >
      <el-table-column label="动作 / 路由" min-width="260"
        ><template #default="{ row }"
          ><strong>{{ row.action }}</strong
          ><small class="table-secondary">{{ row.method ?? '—' }} {{ row.route ?? '' }}</small
          ><small v-if="row.errorCode" class="table-secondary">{{ row.errorCode }}</small></template
        ></el-table-column
      >
      <el-table-column label="操作者" min-width="165"
        ><template #default="{ row }"
          >{{ actor(row as LogRecord)
          }}<small v-if="row.username && row.username !== row.actorName" class="table-secondary">{{
            row.username
          }}</small></template
        ></el-table-column
      >
      <el-table-column label="结果" width="100"
        ><template #default="{ row }"
          ><el-tag
            :type="
              row.outcome === 'success'
                ? 'success'
                : row.outcome === 'failure'
                  ? 'danger'
                  : 'warning'
            "
            >{{ outcomes[row.outcome] ?? row.outcome }}</el-tag
          ></template
        ></el-table-column
      >
      <el-table-column label="HTTP" width="75"
        ><template #default="{ row }">{{ row.status ?? '—' }}</template></el-table-column
      ><el-table-column label="耗时" width="90"
        ><template #default="{ row }">{{
          row.durationMs === null ? '—' : `${row.durationMs} ms`
        }}</template></el-table-column
      >
      <el-table-column label="操作" width="80" fixed="right"
        ><template #default="{ row }"
          ><el-button link type="primary" @click="showDetail(row as LogRecord)"
            >详情</el-button
          ></template
        ></el-table-column
      >
    </el-table>
    <div class="pagination">
      <el-pagination
        v-model:current-page="page"
        v-model:page-size="pageSize"
        :page-sizes="[10, 20, 50, 100]"
        layout="total,sizes,prev,pager,next"
        :total="total"
        @change="load"
      />
    </div>
  </section>
  <el-drawer v-model="detailOpen" title="日志详情" size="560px"
    ><el-descriptions v-if="detail" :column="1" border
      ><el-descriptions-item label="类别">{{ kinds[detail.kind] }}</el-descriptions-item
      ><el-descriptions-item label="时间">{{ dateText(detail.createdAt) }}</el-descriptions-item
      ><el-descriptions-item label="动作">{{ detail.action }}</el-descriptions-item
      ><el-descriptions-item label="结果">{{
        outcomes[detail.outcome] ?? detail.outcome
      }}</el-descriptions-item
      ><el-descriptions-item label="操作者">{{ actor(detail) }}</el-descriptions-item
      ><el-descriptions-item label="账号">{{ detail.username ?? '—' }}</el-descriptions-item
      ><el-descriptions-item label="操作者 ID"
        ><code>{{ detail.actorId ?? '—' }}</code></el-descriptions-item
      ><el-descriptions-item label="目标 ID"
        ><code>{{ detail.targetId ?? '—' }}</code></el-descriptions-item
      ><el-descriptions-item label="请求关联号"
        ><code>{{ detail.requestId ?? '—' }}</code></el-descriptions-item
      ><el-descriptions-item label="连接 IP">{{ detail.ip ?? '—' }}</el-descriptions-item
      ><el-descriptions-item label="方法 / 路由"
        >{{ detail.method ?? '—' }} {{ detail.route ?? '' }}</el-descriptions-item
      ><el-descriptions-item label="HTTP 状态">{{ detail.status ?? '—' }}</el-descriptions-item
      ><el-descriptions-item label="处理耗时">{{
        detail.durationMs === null ? '—' : `${detail.durationMs} ms`
      }}</el-descriptions-item
      ><el-descriptions-item label="错误码">{{ detail.errorCode ?? '—' }}</el-descriptions-item
      ><el-descriptions-item label="摘要">{{
        detail.message ?? '—'
      }}</el-descriptions-item></el-descriptions
    ><template #footer
      ><el-button @click="detailOpen = false">关闭详情</el-button></template
    ></el-drawer
  >
  <el-dialog
    v-model="cleanupOpen"
    title="清理历史日志"
    width="580px"
    :close-on-click-modal="false"
    :show-close="!clearing"
    :close-on-press-escape="!clearing"
  >
    <p>
      本批仅清理<strong>{{ kinds[cleanupKind] }}</strong
      >，不影响其他类别。只允许清理 24 小时以前的记录，清理后不能在界面恢复。
    </p>
    <el-alert
      v-if="pendingRequest && !cleanupResult"
      title="原请求结果尚未确认"
      description="重试将使用原条件继续完成或确认同一批清理，不会自动开始下一批。"
      type="warning"
      :closable="false"
    />
    <el-form
      label-position="top"
      class="cleanup-form"
      :disabled="clearing || !!pendingRequest || !!cleanupResult"
    >
      <el-form-item label="清理时间之前" for="logs-cleanup-before"
        ><input
          id="logs-cleanup-before"
          v-model="cleanupForm.before"
          type="datetime-local"
          class="date-input"
          :max="localDate(Date.now() - 86400000)"
          :disabled="clearing || !!pendingRequest || !!cleanupResult"
      /></el-form-item>
      <el-form-item label="本批上限"
        ><el-input-number
          v-model="cleanupForm.limit"
          :min="1"
          :max="1000"
          :precision="0"
          controls-position="right"
      /></el-form-item>
    </el-form>
    <el-alert v-if="cleanupError" :title="cleanupError" type="error" :closable="false" />
    <el-alert
      v-if="cleanupResult"
      :title="`已清理 ${cleanupResult.deletedCount} 条${kinds[cleanupResult.kind]}`"
      :description="
        cleanupResult.hasMore
          ? '仍有更早记录，确认本批结果后可新建下一批。'
          : '该时间之前已无更多记录。'
      "
      type="success"
      :closable="false"
    />
    <template #footer
      ><el-button :disabled="clearing" @click="closeCleanup">关闭</el-button
      ><el-button
        v-if="cleanupResult?.hasMore && !pendingRequest"
        :disabled="!!pending.length"
        @click="openCleanup(true)"
        >新建下一批</el-button
      ><el-button
        v-if="!cleanupResult || pendingRequest"
        type="danger"
        :loading="clearing"
        @click="clean"
        >{{ pendingRequest ? '重试原请求' : '清理本批' }}</el-button
      ></template
    >
  </el-dialog>
</template>
<style scoped>
.log-filters {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 0 14px;
  padding: 20px 0 0;
}
.date-input {
  width: 100%;
  min-height: 32px;
  border: 1px solid var(--el-border-color);
  background: var(--el-fill-color-blank);
  color: var(--el-text-color-regular);
  border-radius: var(--el-border-radius-base);
  padding: 5px 10px;
  font: inherit;
}
.date-input:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
.date-input:focus-visible {
  outline: 2px solid var(--tri-accent);
  outline-offset: 2px;
}
.sort-select {
  width: 145px;
}
.pending-cleanup {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  margin-bottom: 16px;
  gap: 16px;
}
.pending-cleanup p {
  margin: 5px 0 0;
}
.cleanup-form {
  margin-top: 18px;
}
code {
  overflow-wrap: anywhere;
}
@media (max-width: 1100px) {
  .log-filters {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
@media (max-width: 600px) {
  .log-filters {
    grid-template-columns: minmax(0, 1fr);
  }
  .pending-cleanup {
    flex-wrap: wrap;
  }
}
</style>
