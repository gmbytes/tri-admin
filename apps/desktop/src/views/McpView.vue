<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { useSession } from '../stores/session'
import { ApiError } from '../core/client'
import { parsePage, objectId } from '../core/open-access'
import {
  parseConnection,
  parseAction,
  parseCapabilities,
  parseTools,
  parseToolLines,
  parseTransport,
  parseResult,
  verifyResult,
  resultLabel,
  validateArguments,
  McpRecoveryStore,
  verifyMcpReceipt,
  type McpConnection,
  type McpAction,
  type McpCapability,
  type McpTool,
  type McpOperation,
  type McpKind,
  type McpResult,
} from '../core/mcp'
import { errorText, dateText } from '../ui'
import PluginSourcePicker from '../components/PluginSourcePicker.vue'
import type { McpPluginCapability } from '../core/mcp'
const session = useSession(),
  identity = computed(() => `${session.settings.apiBaseUrl}|${session.user?.id ?? ''}`),
  canWrite = computed(() => session.can('mcp.write')),
  canInvoke = computed(() => session.can('mcp.invoke')),
  canBind = computed(() => session.can('settings.write') && session.can('settings.read'))
const rows = ref<McpConnection[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  status = ref<'active' | 'deleted'>('active'),
  search = ref(''),
  loading = ref(false),
  error = ref(''),
  writing = ref(false)
const selected = ref<McpConnection | null>(null),
  managerOpen = ref(false),
  tab = ref('tools'),
  tools = ref<McpTool[]>([]),
  toolsError = ref(''),
  actions = ref<McpAction[]>([]),
  historyPage = ref(1),
  historySize = ref(20),
  historyTotal = ref(0),
  historyError = ref('')
const editorOpen = ref(false),
  editing = ref<McpConnection | null>(null),
  formError = ref(''),
  caps = ref<McpCapability[]>([]),
  profiles = ref<Profile[]>([]),
  optionsError = ref('')
interface Profile {
  id: string
  name: string
  code: string
  version: number
  enabled: boolean
}
const form = reactive({
  name: '',
  kind: 'stdio' as 'stdio' | 'http' | 'plugin',
  capabilityId: '',
  profileId: '',
  profileVersion: 1,
  allowedTools: '',
  extensionId: '',
  revisionId: '',
  contributionId: '',
  enabled: false,
})
const sourcePickerOpen = ref(false),
  selectedSource = ref<McpPluginCapability | null>(null)
const canUseSource = computed(
    () => selected.value?.transport.kind !== 'plugin' || session.can('skills.read'),
  ),
  canCall = computed(() => canInvoke.value && canUseSource.value)
function chooseSource(value: McpPluginCapability) {
  selectedSource.value = value
  form.extensionId = value.extensionId
  form.revisionId = value.revisionId
  form.contributionId = value.contributionId
}
function transportText(row: McpConnection) {
  const source = row.transport
  return source.kind === 'stdio'
    ? `STDIO · ${source.capabilityId}`
    : source.kind === 'http'
      ? `HTTP · 配置 v${source.profileVersion}`
      : `插件 MCP · ${source.contributionId} · ${source.revisionId.slice(0, 8)}`
}
const callOpen = ref(false),
  callTool = ref<McpTool | null>(null),
  argumentsJson = ref('{}'),
  callError = ref(''),
  resultOpen = ref(false),
  result = ref<McpResult | null>(null),
  resultError = ref(''),
  resultLoading = ref(false)
const orphanOpen = ref(false),
  orphan = reactive({ confirmed: false, reason: '' }),
  orphanError = ref('')
const pending = ref<McpOperation[]>([]),
  recoveryError = ref(''),
  recovering = ref('')
const blocked = computed(() => writing.value || pending.value.length > 0 || !!recoveryError.value),
  active = computed(
    () =>
      !!selected.value &&
      !selected.value.deletedAt &&
      selected.value.runtime?.state === 'running' &&
      !selected.value.runtime.releaseConfirmed &&
      selected.value.capabilityCurrent,
  ),
  releasable = (row: McpConnection) =>
    !row.runtime || row.runtime.releaseConfirmed || row.runtime.state === 'retired'
const selectedProfile = computed(() => profiles.value.find((row) => row.id === form.profileId)),
  canConfirm = computed(
    () =>
      canWrite.value &&
      session.user?.dataScope === 'all' &&
      selected.value?.runtime?.transportKind === 'stdio' &&
      selected.value.runtime.state === 'orphaned',
  )
const kindLabels: Record<string, string> = {
  create: '创建连接',
  start: '启动',
  stop: '停止',
  restart: '重启',
  refresh: '刷新工具',
  call: '调用',
  cancel: '请求取消',
  'orphan.confirm': '运维确认',
}
const stateLabels: Record<string, string> = {
  starting: '启动中',
  running: '就绪',
  stopping: '释放中',
  stopped: '已停止',
  failed: '失败',
  orphaned: '遗留进程未确认',
  unconfirmed: '所有权未确认',
  disconnected: '已断开',
  retired: '已记录清理声明',
}
let epoch = 0,
  alive = true,
  listSequence = 0,
  detailSequence = 0,
  toolsSequence = 0,
  historySequence = 0,
  resultSequence = 0,
  polling = false,
  configuring = false,
  store: McpRecoveryStore | null = null,
  timer: ReturnType<typeof setTimeout> | null = null
const controls = new Set<AbortController>(),
  current = (ticket: number) => alive && ticket === epoch
async function request<T = unknown>(
  path: string,
  options: { method?: string; body?: unknown } = {},
) {
  const control = new AbortController()
  controls.add(control)
  try {
    return await session.request<T>(path, { ...options, signal: control.signal })
  } finally {
    controls.delete(control)
  }
}
function readPending() {
  try {
    if (!store && session.user)
      store = new McpRecoveryStore(localStorage, session.settings.apiBaseUrl, session.user.id)
    pending.value = store?.read() ?? []
    recoveryError.value = ''
  } catch {
    pending.value = []
    recoveryError.value = '本机恢复记录不可用，请先核对操作历史'
  }
}
function reset() {
  epoch++
  listSequence++
  detailSequence++
  toolsSequence++
  historySequence++
  polling = false
  loading.value = false
  if (timer) clearTimeout(timer)
  timer = null
  for (const control of controls) control.abort()
  controls.clear()
  rows.value = []
  selected.value = null
  tools.value = []
  actions.value = []
  managerOpen.value = false
  editorOpen.value = false
  sourcePickerOpen.value = false
  selectedSource.value = null
  callOpen.value = false
  resultOpen.value = false
  result.value = null
  pending.value = []
  store = null
  writing.value = false
  error.value = ''
  recoveryError.value = ''
  caps.value = []
  profiles.value = []
}
function schedule() {
  if (timer) clearTimeout(timer)
  timer = null
  if (!alive || polling) return
  const busy = actions.value.some((row) => row.status === 'pending' || row.status === 'running')
  if (
    managerOpen.value ||
    rows.value.some(
      (row) => row.runtime && !row.runtime.releaseConfirmed && row.runtime.state !== 'retired',
    )
  )
    timer = setTimeout(
      async () => {
        const ticket = epoch
        timer = null
        polling = true
        try {
          await load(false)
          if (current(ticket) && managerOpen.value && selected.value)
            await details(selected.value.id, false)
        } finally {
          if (current(ticket)) {
            polling = false
            schedule()
          }
        }
      },
      busy ? 500 : 2000,
    )
}
async function load(show = true) {
  const ticket = epoch,
    sequence = ++listSequence
  if (show) {
    loading.value = true
    error.value = ''
  }
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
      status: status.value,
      search: search.value,
      sort: '-createdAt',
    })
    const parsed = parsePage(await request(`/mcp/connections?${query}`), parseConnection)
    if (current(ticket) && sequence === listSequence) {
      rows.value = parsed.data
      total.value = parsed.meta.total
      schedule()
    }
  } catch (value) {
    if (current(ticket) && sequence === listSequence) error.value = errorText(value)
  } finally {
    if (current(ticket) && sequence === listSequence) loading.value = false
  }
}
async function details(id: string, initial = true) {
  const ticket = epoch,
    sequence = ++detailSequence
  try {
    const row = parseConnection((await request(`/mcp/connections/${id}`)).data)
    if (!current(ticket) || sequence !== detailSequence) return
    selected.value = row
    if (initial) {
      historyPage.value = 1
      tab.value = row.deletedAt ? 'history' : 'tools'
      managerOpen.value = true
    }
    await Promise.all([loadTools(row), loadHistory(row)])
    schedule()
  } catch (value) {
    if (current(ticket) && sequence === detailSequence) error.value = errorText(value)
  }
}
async function loadTools(row = selected.value) {
  const ticket = epoch,
    sequence = ++toolsSequence
  if (!row) return
  if (row.deletedAt || !row.runtime) {
    tools.value = []
    toolsError.value = ''
    return
  }
  try {
    const value = parseTools((await request(`/mcp/connections/${row.id}/tools`)).data)
    if (current(ticket) && sequence === toolsSequence && selected.value?.id === row.id) {
      tools.value = value
      toolsError.value = ''
    }
  } catch (value) {
    if (current(ticket) && sequence === toolsSequence && selected.value?.id === row.id) {
      tools.value = []
      toolsError.value = errorText(value)
    }
  }
}
async function loadHistory(row = selected.value) {
  const ticket = epoch,
    sequence = ++historySequence
  if (!row) return
  try {
    const value = parsePage(
      await request(
        `/mcp/connections/${row.id}/actions?${new URLSearchParams({ page: String(historyPage.value), pageSize: String(historySize.value), sort: '-createdAt' })}`,
      ),
      parseAction,
    )
    if (current(ticket) && sequence === historySequence && selected.value?.id === row.id) {
      actions.value = value.data
      historyTotal.value = value.meta.total
      historyError.value = ''
    }
  } catch (value) {
    if (current(ticket) && sequence === historySequence && selected.value?.id === row.id) {
      actions.value = []
      historyError.value = errorText(value)
    }
  }
}
function profilesFrom(value: unknown): Profile[] {
  if (!Array.isArray(value)) throw Error('HTTP 配置目录无效')
  return value.map((row) => {
    if (
      !row ||
      typeof row !== 'object' ||
      !objectId(row.id) ||
      typeof row.name !== 'string' ||
      typeof row.code !== 'string' ||
      !Number.isSafeInteger(row.version) ||
      row.version < 1 ||
      typeof row.enabled !== 'boolean' ||
      row.configuration?.kind !== 'http'
    )
      throw Error('HTTP 配置目录无效')
    return {
      id: row.id,
      name: row.name,
      code: row.code,
      version: row.version,
      enabled: row.enabled,
    }
  })
}
async function options(pinned: string | null = null) {
  const ticket = epoch
  optionsError.value = ''
  try {
    const directory = parseCapabilities((await request('/mcp/capabilities')).data)
    let http: Profile[] = []
    if (canBind.value) {
      http = profilesFrom(
        (await request('/settings/profiles?kind=http&pageSize=100&sort=code')).data,
      )
      if (pinned && !http.some((row) => row.id === pinned))
        http.unshift(...profilesFrom([(await request(`/settings/profiles/${pinned}`)).data]))
    }
    if (current(ticket)) {
      caps.value = directory
      profiles.value = http
    }
  } catch (value) {
    if (current(ticket)) optionsError.value = errorText(value)
  }
}
watch(
  () => form.profileId,
  () => {
    if (!configuring && selectedProfile.value) form.profileVersion = selectedProfile.value.version
  },
  { flush: 'sync' },
)
async function edit(row: McpConnection | null) {
  const ticket = epoch
  formError.value = ''
  editing.value = null
  await options(row?.transport.kind === 'http' ? row.transport.profileId : null)
  if (!current(ticket)) return
  if (row) {
    try {
      row = parseConnection((await request(`/mcp/connections/${row.id}`)).data)
      if (!current(ticket)) return
    } catch (value) {
      error.value = errorText(value)
      return
    }
  }
  configuring = true
  editing.value = row
  form.name = row?.name ?? ''
  form.kind = row?.transport.kind ?? 'stdio'
  form.capabilityId = row?.transport.kind === 'stdio' ? row.transport.capabilityId : ''
  form.profileId = row?.transport.kind === 'http' ? row.transport.profileId : ''
  form.profileVersion = row?.transport.kind === 'http' ? row.transport.profileVersion : 1
  form.allowedTools = row?.transport.kind === 'http' ? row.transport.allowedTools.join('\n') : ''
  form.extensionId = row?.transport.kind === 'plugin' ? row.transport.extensionId : ''
  form.revisionId = row?.transport.kind === 'plugin' ? row.transport.revisionId : ''
  form.contributionId = row?.transport.kind === 'plugin' ? row.transport.contributionId : ''
  selectedSource.value = null
  sourcePickerOpen.value = false
  form.enabled = row?.enabled ?? false
  configuring = false
  editorOpen.value = true
}
function operation(
  kind: McpOperation['kind'],
  row: McpConnection | null,
  targetActionId: string | null = null,
): McpOperation {
  return {
    requestId: crypto.randomUUID(),
    kind,
    connectionId: row?.id ?? null,
    targetActionId,
    version: kind === 'create' || kind === 'cancel' ? null : row!.version,
    createdAt: new Date().toISOString(),
  }
}
async function submit(op: McpOperation, send: () => Promise<unknown>) {
  const ticket = epoch,
    saved = store,
    actor = session.user?.id
  if (!saved || !actor) throw Error('当前恢复记录不可用')
  saved.add(op)
  readPending()
  writing.value = true
  try {
    const value = await send()
    if (!current(ticket) || store !== saved) return null
    const receipt = verifyMcpReceipt(op, value, actor)
    saved.remove(op.requestId)
    readPending()
    return receipt
  } catch (value) {
    if (current(ticket) && store === saved) {
      if (value instanceof ApiError && value.status >= 400 && value.status < 500) {
        saved.remove(op.requestId)
        readPending()
      } else readPending()
    }
    throw value
  } finally {
    if (current(ticket)) writing.value = false
  }
}
function transport() {
  if (form.kind === 'plugin')
    return parseTransport({
      kind: 'plugin',
      extensionId: form.extensionId,
      revisionId: form.revisionId,
      contributionId: form.contributionId,
    })
  return parseTransport(
    form.kind === 'stdio'
      ? { kind: 'stdio', capabilityId: form.capabilityId }
      : {
          kind: 'http',
          profileId: form.profileId,
          profileVersion: form.profileVersion,
          allowedTools: parseToolLines(form.allowedTools),
        },
  )
}
async function save() {
  const ticket = epoch
  formError.value = ''
  try {
    if (!form.name.trim()) throw Error('请填写连接名称')
    if (form.kind === 'plugin') {
      if (!session.can('skills.read')) throw Error('当前账号缺少读取插件来源的权限')
      if (editing.value?.capabilityCurrent === false && !selectedSource.value)
        throw Error('请重新选择当前插件来源')
    }
    const body = { name: form.name.trim(), enabled: form.enabled, transport: transport() }
    if (editing.value) {
      writing.value = true
      parseConnection(
        (
          await request(`/mcp/connections/${editing.value.id}`, {
            method: 'PATCH',
            body: { version: editing.value.version, ...body },
          })
        ).data,
      )
    } else {
      const op = operation('create', null)
      const receipt = await submit(
        op,
        async () =>
          (
            await request('/mcp/connections', {
              method: 'POST',
              body: { requestId: op.requestId, ...body },
            })
          ).data,
      )
      if (!receipt) return
    }
    if (current(ticket)) {
      editorOpen.value = false
      await load()
      ElMessage.success('连接已保存')
    }
  } catch (value) {
    if (current(ticket)) formError.value = errorText(value)
  } finally {
    if (current(ticket)) writing.value = false
  }
}
async function remove(row: McpConnection) {
  const ticket = epoch
  try {
    await ElMessageBox.confirm(`删除 ${row.name} 后保留原操作历史。`, '删除 MCP 连接', {
      confirmButtonText: '删除连接',
      cancelButtonText: '保留',
      type: 'warning',
    })
    if (!current(ticket)) return
    writing.value = true
    await request(`/mcp/connections/${row.id}?version=${row.version}`, { method: 'DELETE' })
    if (current(ticket)) {
      if (selected.value?.id === row.id) managerOpen.value = false
      await load()
    }
  } catch (value) {
    if (current(ticket) && value !== 'cancel' && value !== 'close') error.value = errorText(value)
  } finally {
    if (current(ticket)) writing.value = false
  }
}
async function control(kind: 'start' | 'stop' | 'restart' | 'refresh') {
  const row = selected.value
  if (!row) return
  const ticket = epoch
  error.value = ''
  try {
    const op = operation(kind, row)
    const receipt = await submit(
      op,
      async () =>
        (
          await request(`/mcp/connections/${row.id}/actions`, {
            method: 'POST',
            body: { requestId: op.requestId, version: row.version, kind },
          })
        ).data,
    )
    if (current(ticket) && receipt) {
      await details(row.id, false)
      await load(false)
    }
  } catch (value) {
    if (current(ticket)) error.value = errorText(value)
  }
}
function openCall(tool: McpTool) {
  if (!canCall.value) return
  callTool.value = tool
  argumentsJson.value = '{}'
  callError.value = ''
  callOpen.value = true
}
async function invoke() {
  const row = selected.value,
    tool = callTool.value
  if (!row || !tool || !canCall.value) return
  const ticket = epoch
  callError.value = ''
  try {
    validateArguments(argumentsJson.value)
    const op = operation('call', row),
      raw = argumentsJson.value
    const receipt = await submit(op, async () => {
      const controller = new AbortController()
      controls.add(controller)
      try {
        return (
          await session.callMcpTool(row.id, op.requestId, row.version, tool.name, raw, {
            signal: controller.signal,
          })
        ).data
      } finally {
        controls.delete(controller)
      }
    })
    if (current(ticket) && receipt) {
      callOpen.value = false
      tab.value = 'history'
      await details(row.id, false)
    }
  } catch (value) {
    if (current(ticket)) callError.value = errorText(value)
  }
}
async function cancel(row: McpAction) {
  const connection = selected.value
  if (!connection) return
  const ticket = epoch
  try {
    const op = operation('cancel', connection, row.id)
    const receipt = await submit(
      op,
      async () =>
        (
          await request(`/mcp/actions/${row.id}/cancel`, {
            method: 'POST',
            body: { requestId: op.requestId },
          })
        ).data,
    )
    if (current(ticket) && receipt) await loadHistory()
  } catch (value) {
    if (current(ticket)) error.value = errorText(value)
  }
}
async function showResult(row: McpAction) {
  const ticket = epoch,
    sequence = ++resultSequence,
    isCurrentResult = () => current(ticket) && sequence === resultSequence && resultOpen.value
  resultOpen.value = true
  result.value = null
  resultError.value = ''
  resultLoading.value = true
  try {
    const value = parseResult((await request(`/mcp/actions/${row.id}/result`)).data)
    await verifyResult(value)
    if (
      value.kind !== row.resultKind ||
      value.size !== row.resultSize ||
      value.sha256 !== row.resultSha256
    )
      throw Error('结果与原操作摘要不一致')
    if (isCurrentResult()) result.value = value
  } catch (value) {
    if (isCurrentResult()) resultError.value = errorText(value)
  } finally {
    if (isCurrentResult()) resultLoading.value = false
  }
}
async function confirmOrphan() {
  const row = selected.value
  if (!row) return
  const ticket = epoch
  orphanError.value = ''
  try {
    if (!orphan.confirmed || !orphan.reason.trim()) throw Error('请确认宿主清理并填写说明')
    const op = operation('orphan.confirm', row)
    const receipt = await submit(
      op,
      async () =>
        (
          await request(`/mcp/connections/${row.id}/confirm-orphan`, {
            method: 'POST',
            body: {
              requestId: op.requestId,
              version: row.version,
              confirmed: true,
              reason: orphan.reason.trim(),
            },
          })
        ).data,
    )
    if (current(ticket) && receipt) {
      orphanOpen.value = false
      await details(row.id, false)
      await load(false)
    }
  } catch (value) {
    if (current(ticket)) orphanError.value = errorText(value)
  }
}
async function recover(op: McpOperation) {
  const ticket = epoch,
    saved = store,
    actor = session.user?.id
  if (!saved || !actor) return
  recovering.value = op.requestId
  recoveryError.value = ''
  try {
    const path =
      op.kind === 'create'
        ? `/mcp/connections/by-request/${op.requestId}`
        : `/mcp/actions/by-request/${op.requestId}`
    const value = verifyMcpReceipt(op, (await request(path)).data, actor)
    if (!current(ticket) || saved !== store) return
    saved.remove(op.requestId)
    readPending()
    await load(false)
    if ('connectionId' in value) {
      await details(value.connectionId)
      tab.value = 'history'
    } else if (!value.deletedAt) await details(value.id)
    ElMessage.success('已找到原请求，没有重新执行')
  } catch (value) {
    if (current(ticket)) recoveryError.value = errorText(value)
  } finally {
    if (current(ticket)) recovering.value = ''
  }
}
async function forget(op: McpOperation) {
  const ticket = epoch
  try {
    await ElMessageBox.confirm(
      '请先核对服务器操作历史。移除本机标记不会撤销或重发操作。',
      '移除恢复标记',
      { confirmButtonText: '移除标记', cancelButtonText: '保留' },
    )
    if (current(ticket)) {
      store?.remove(op.requestId)
      readPending()
    }
  } catch (value) {
    if (current(ticket) && value !== 'cancel' && value !== 'close')
      recoveryError.value = errorText(value)
  }
}
async function clearLocal() {
  const ticket = epoch
  try {
    await ElMessageBox.confirm(
      '请先核对服务器操作历史。清除本机记录不会撤销或重发任何操作。',
      '清除本机恢复记录',
      { confirmButtonText: '清除本机记录', cancelButtonText: '保留' },
    )
    if (current(ticket)) {
      store?.clear()
      readPending()
    }
  } catch (value) {
    if (current(ticket) && value !== 'cancel' && value !== 'close')
      recoveryError.value = errorText(value)
  }
}
function actionLabel(row: McpAction) {
  if (row.status === 'inputRequired') return '需要后续输入'
  if (row.status === 'remoteTask') return '收到远端任务'
  if (row.status === 'succeeded')
    return row.kind === 'call'
      ? row.resultKind === 'task'
        ? '收到远端任务'
        : row.resultKind === 'inputRequired'
          ? '需要后续输入'
          : '已返回'
      : '已完成'
  return (
    {
      pending: '待处理',
      running: '处理中',
      failed: '失败',
      cancelled: '已取消',
      unknown: '效果未确认',
    }[row.status] ?? row.status
  )
}
function searchRows() {
  page.value = 1
  void load()
}
function openOrphan() {
  orphan.confirmed = false
  orphan.reason = ''
  orphanError.value = ''
  orphanOpen.value = true
}
function closeResult() {
  resultOpen.value = false
  result.value = null
}
watch(
  resultOpen,
  (value) => {
    if (!value) {
      resultSequence++
      result.value = null
      resultError.value = ''
      resultLoading.value = false
    }
  },
  { flush: 'sync' },
)
watch(
  identity,
  () => {
    reset()
    readPending()
    void load()
  },
  { immediate: true },
)
watch(status, () => {
  page.value = 1
  void load()
})
watch(canCall, (value) => {
  if (!value) {
    callOpen.value = false
    argumentsJson.value = '{}'
  }
})
watch(canInvoke, (value) => {
  if (!value) {
    callOpen.value = false
    resultOpen.value = false
    result.value = null
    argumentsJson.value = '{}'
  }
})
onBeforeUnmount(() => {
  reset()
  alive = false
})
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">TOOL CONNECTIONS</div>
      <h1>MCP连接</h1>
      <p>管理批准的工具连接，查看真实调用与原请求记录。</p>
    </div>
    <el-button v-if="canWrite" type="primary" :disabled="blocked" @click="edit(null)"
      >新建连接</el-button
    >
  </div>
  <el-alert v-if="error" :title="error" type="error" :closable="false" />
  <section v-if="pending.length || recoveryError" class="surface recovery">
    <h3>待确认 MCP 操作</h3>
    <p class="subtle">先查询原请求；恢复查询不会重新执行工具。</p>
    <el-alert v-if="recoveryError" :title="recoveryError" type="error" :closable="false" />
    <div v-for="row in pending" :key="row.requestId" class="pending-row">
      <div>
        <strong>{{ kindLabels[row.kind] }}</strong
        ><code class="table-secondary">{{ row.requestId }}</code>
      </div>
      <el-button :loading="recovering === row.requestId" @click="recover(row)">恢复查询</el-button
      ><el-button text @click="forget(row)">移除标记</el-button>
    </div>
    <el-button text @click="readPending">重读本机记录</el-button
    ><el-button v-if="recoveryError" text type="danger" @click="clearLocal">清除本机记录</el-button>
  </section>
  <section class="surface table-surface">
    <el-tabs v-model="status"
      ><el-tab-pane label="有效连接" name="active" /><el-tab-pane label="已删除" name="deleted"
    /></el-tabs>
    <div class="table-toolbar">
      <el-input
        v-model="search"
        placeholder="按连接名称前缀搜索"
        clearable
        @keyup.enter="searchRows"
      /><el-button :loading="loading" @click="load()">刷新列表</el-button>
    </div>
    <el-table :data="rows" row-key="id" class="mcp-connections" v-loading="loading"
      ><el-table-column label="连接" min-width="190"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><small class="table-secondary">{{
            transportText(row as McpConnection)
          }}</small></template
        ></el-table-column
      ><el-table-column label="状态" min-width="170"
        ><template #default="{ row }"
          ><el-tag :type="row.runtime?.state === 'running' ? 'success' : 'info'">{{
            row.runtime ? stateLabels[row.runtime.state] : '未启动'
          }}</el-tag
          ><small v-if="!row.capabilityCurrent" class="field-error">绑定已变更</small
          ><small v-else class="table-secondary"
            >{{ row.enabled ? '已启用' : '已停用' }} · v{{ row.version }}</small
          ></template
        ></el-table-column
      ><el-table-column label="更新时间" min-width="170"
        ><template #default="{ row }">{{ dateText(row.updatedAt) }}</template></el-table-column
      ><el-table-column label="操作" width="240"
        ><template #default="{ row }"
          ><el-button text @click="details(row.id)">管理</el-button
          ><el-button
            v-if="canWrite && !row.deletedAt"
            text
            :disabled="blocked || !releasable(row as McpConnection)"
            @click="edit(row as McpConnection)"
            >编辑</el-button
          ><el-button
            v-if="canWrite && !row.deletedAt"
            text
            type="danger"
            :disabled="blocked || !releasable(row as McpConnection)"
            @click="remove(row as McpConnection)"
            >删除</el-button
          ></template
        ></el-table-column
      ></el-table
    ><el-empty v-if="!loading && !rows.length" description="暂无连接" /><el-pagination
      v-model:current-page="page"
      v-model:page-size="pageSize"
      :total="total"
      :page-sizes="[20, 50, 100]"
      layout="total, sizes, prev, pager, next"
      @current-change="load()"
      @size-change="searchRows"
    />
  </section>
  <el-dialog
    v-model="editorOpen"
    class="mcp-dialog"
    :title="editing ? '编辑 MCP 连接' : '新建 MCP 连接'"
    width="650px"
    :close-on-click-modal="false"
    ><el-alert v-if="formError" :title="formError" type="error" :closable="false" /><el-alert
      v-if="optionsError"
      :title="optionsError"
      type="error"
      :closable="false"
    /><el-form label-position="top"
      ><el-form-item label="连接名称"
        ><el-input v-model="form.name" aria-label="连接名称" maxlength="64" /></el-form-item
      ><el-form-item label="传输方式"
        ><el-select
          v-model="form.kind"
          aria-label="传输方式"
          :disabled="!!editing && editing.transport.kind === 'http' && !canBind"
          ><el-option value="stdio" label="STDIO" /><el-option
            value="http"
            label="HTTP"
            :disabled="!canBind" /><el-option
            value="plugin"
            label="插件 MCP"
            :disabled="!session.can('skills.read')" /></el-select></el-form-item
      ><template v-if="form.kind === 'stdio'"
        ><el-form-item label="批准程序能力"
          ><el-select v-model="form.capabilityId" aria-label="批准程序能力"
            ><el-option
              v-for="cap in caps"
              :key="cap.id"
              :value="cap.id"
              :label="cap.label + ' · ' + cap.id" /></el-select
        ></el-form-item>
        <p v-if="!caps.length" class="subtle">暂无部署批准的 STDIO 能力。</p></template
      ><template v-else-if="form.kind === 'plugin'">
        <el-form-item label="插件来源">
          <div class="plugin-binding">
            <strong v-if="selectedSource"
              >{{ selectedSource.extensionName }} · 修订 {{ selectedSource.revision }} · 版本
              {{ selectedSource.extensionVersion }}</strong
            >
            <span v-else-if="form.extensionId"
              >扩展 {{ form.extensionId.slice(0, 8) }} · {{ form.contributionId }}</span
            >
            <span v-else class="subtle">尚未选择来源</span>
            <code v-if="form.revisionId">修订 ID {{ form.revisionId }}</code>
            <small v-if="selectedSource"
              >部署能力 {{ selectedSource.capabilityId }} · 批准工具
              {{ selectedSource.allowedTools.join(' · ') || '无' }}</small
            >
            <el-button
              :disabled="!session.can('skills.read') || blocked"
              @click="sourcePickerOpen = true"
              >选择插件来源</el-button
            >
          </div>
        </el-form-item>
        <p class="subtle">
          使用插件中固定修订的批准来源。插件启停版本或修订改变后，旧连接需重新选择并确认来源。
        </p> </template
      ><template v-else
        ><el-form-item label="HTTP配置"
          ><el-select v-model="form.profileId" aria-label="HTTP配置" filterable :disabled="!canBind"
            ><el-option
              v-for="profile in profiles"
              :key="profile.id"
              :value="profile.id"
              :label="profile.name + ' · v' + profile.version"
              :disabled="!profile.enabled" /></el-select
          ><code v-if="!canBind">{{ form.profileId }}</code></el-form-item
        >
        <p class="subtle">
          绑定版本 v{{ form.profileVersion
          }}<el-button
            v-if="canBind && selectedProfile && selectedProfile.version !== form.profileVersion"
            text
            @click="form.profileVersion = selectedProfile.version"
            >使用配置当前版本 v{{ selectedProfile.version }}</el-button
          >
        </p>
        <el-form-item label="批准工具"
          ><el-input
            v-model="form.allowedTools"
            aria-label="批准工具"
            type="textarea"
            :rows="4"
            :disabled="!canBind"
            placeholder="每行一个实际工具名" /></el-form-item></template
      ><el-form-item label="允许连接"
        ><el-switch v-model="form.enabled" aria-label="允许连接"
      /></el-form-item>
      <p class="subtle">保存和启用不会自动连接，需在管理中显式启动。</p></el-form
    ><template #footer
      ><el-button @click="editorOpen = false">关闭编辑</el-button
      ><el-button v-if="editing" :disabled="writing" @click="edit(editing)">重新载入记录</el-button
      ><el-button
        type="primary"
        :loading="writing"
        :disabled="blocked || (form.kind === 'plugin' && !session.can('skills.read'))"
        @click="save"
        >保存连接</el-button
      ></template
    ></el-dialog
  >
  <el-drawer
    v-model="managerOpen"
    title="连接管理"
    size="min(980px, 96vw)"
    :close-on-click-modal="false"
    ><template v-if="selected"
      ><el-alert v-if="error" :title="error" type="error" :closable="false" />
      <h2>{{ selected.name }}</h2>
      <div class="mcp-summary">
        <el-tag>{{ selected.transport.kind === 'http' ? 'HTTP' : 'STDIO' }}</el-tag
        ><el-tag v-if="selected.transport.kind === 'plugin'"
          >插件来源 · {{ selected.transport.contributionId }}</el-tag
        ><el-tag :type="selected.runtime?.state === 'running' ? 'success' : 'info'">{{
          selected.runtime ? stateLabels[selected.runtime.state] : '未启动'
        }}</el-tag
        ><span class="subtle">连接版本 {{ selected.version }}</span
        ><span v-if="selected.runtime?.pid" class="subtle">PID {{ selected.runtime.pid }}</span>
      </div>
      <el-alert
        v-if="!selected.capabilityCurrent && !selected.deletedAt"
        title="绑定已变更；确认旧资源释放后重新绑定配置。"
        type="warning"
        :closable="false" />
      <div v-if="canWrite && !selected.deletedAt" class="mcp-controls">
        <el-button
          type="primary"
          :disabled="
            blocked ||
            !canUseSource ||
            !selected.enabled ||
            !selected.capabilityCurrent ||
            !releasable(selected)
          "
          @click="control('start')"
          >启动</el-button
        ><el-button
          :disabled="
            blocked ||
            releasable(selected) ||
            selected.runtime?.state === 'orphaned' ||
            selected.runtime?.state === 'unconfirmed'
          "
          @click="control('stop')"
          >停止</el-button
        ><el-button :disabled="blocked || !active || !canUseSource" @click="control('restart')"
          >重启</el-button
        ><el-button :disabled="blocked || !active || !canUseSource" @click="control('refresh')"
          >刷新工具</el-button
        ><el-button v-if="canConfirm" type="warning" :disabled="blocked" @click="openOrphan"
          >确认遗留程序清理</el-button
        >
      </div>
      <el-tabs v-model="tab"
        ><el-tab-pane v-if="!selected.deletedAt" label="工具清单" name="tools"
          ><p class="subtle">清单来自最后一次实际发现；刷新工具会重新查询批准的上游。</p>
          <el-alert v-if="toolsError" :title="toolsError" type="error" :closable="false" /><el-table
            :data="tools"
            row-key="name"
            class="mcp-tools"
            ><el-table-column prop="name" label="工具" min-width="180" /><el-table-column
              prop="description"
              label="说明"
              min-width="240"
            /><el-table-column label="操作" width="100"
              ><template #default="{ row }"
                ><el-button
                  v-if="canCall"
                  text
                  :disabled="blocked || !active"
                  @click="openCall(row as McpTool)"
                  >调用</el-button
                ></template
              ></el-table-column
            ></el-table
          ><el-empty v-if="!tools.length" description="暂无实际批准工具" /></el-tab-pane
        ><el-tab-pane label="操作历史" name="history"
          ><el-alert
            v-if="historyError"
            :title="historyError"
            type="error"
            :closable="false" /><el-table :data="actions" row-key="id" class="mcp-history"
            ><el-table-column label="操作" min-width="160"
              ><template #default="{ row }"
                ><strong>{{ row.name ?? kindLabels[row.kind] }}</strong
                ><small class="table-secondary"
                  >{{ kindLabels[row.kind] }} · {{ dateText(row.createdAt) }}</small
                ></template
              ></el-table-column
            ><el-table-column label="状态" min-width="150"
              ><template #default="{ row }"
                ><el-tag
                  :type="
                    row.status === 'unknown'
                      ? 'warning'
                      : row.status === 'failed'
                        ? 'danger'
                        : 'info'
                  "
                  >{{ actionLabel(row as McpAction) }}</el-tag
                ><small v-if="row.errorCode" class="table-secondary">{{
                  row.errorCode
                }}</small></template
              ></el-table-column
            ><el-table-column label="请求标识" min-width="230"
              ><template #default="{ row }"
                ><code>{{ row.requestId }}</code></template
              ></el-table-column
            ><el-table-column label="操作" width="145"
              ><template #default="{ row }"
                ><el-button
                  v-if="canInvoke && row.actorId === session.user?.id && row.resultKind"
                  text
                  @click="showResult(row as McpAction)"
                  >结果</el-button
                ><el-button
                  v-if="
                    canInvoke &&
                    row.kind === 'call' &&
                    ['pending', 'running'].includes(row.status) &&
                    (row.actorId === session.user?.id || session.user?.dataScope === 'all')
                  "
                  text
                  :disabled="blocked || row.cancelRequested"
                  @click="cancel(row as McpAction)"
                  >{{ row.cancelRequested ? '已请求取消' : '取消' }}</el-button
                ></template
              ></el-table-column
            ></el-table
          ><el-pagination
            v-model:current-page="historyPage"
            v-model:page-size="historySize"
            :total="historyTotal"
            layout="total, prev, pager, next"
            @current-change="loadHistory()" /></el-tab-pane></el-tabs></template
    ><template #footer
      ><el-button @click="managerOpen = false">关闭管理</el-button></template
    ></el-drawer
  >
  <el-dialog
    v-model="callOpen"
    class="mcp-dialog"
    title="调用 MCP 工具"
    width="720px"
    :close-on-click-modal="false"
    ><template v-if="callTool"
      ><h3>{{ callTool.name }}</h3>
      <p>{{ callTool.description }}</p>
      <details>
        <summary>输入格式</summary>
        <pre class="mcp-json">{{ JSON.stringify(callTool.inputSchema, null, 2) }}</pre>
      </details>
      <el-alert v-if="callError" :title="callError" type="error" :closable="false" /><el-alert
        v-if="actions.some((row) => row.name === callTool?.name && row.status === 'unknown')"
        title="已有同名工具操作效果未确认，请先核对远端状态。"
        type="warning"
        :closable="false" /><el-form label-position="top"
        ><el-form-item label="参数 JSON"
          ><el-input
            v-model="argumentsJson"
            aria-label="参数 JSON"
            type="textarea"
            :rows="9" /></el-form-item></el-form></template
    ><template #footer
      ><el-button @click="callOpen = false">关闭调用</el-button
      ><el-button
        type="primary"
        :loading="writing"
        :disabled="blocked || !active || !canCall"
        @click="invoke"
        >发送调用</el-button
      ></template
    ></el-dialog
  >
  <el-dialog v-model="resultOpen" class="mcp-dialog" title="工具结果" width="780px"
    ><div v-loading="resultLoading">
      <el-alert v-if="resultError" :title="resultError" type="error" :closable="false" /><template
        v-if="result"
        ><el-tag :type="resultLabel(result) === '远端效果未确认' ? 'warning' : 'info'">{{
          resultLabel(result)
        }}</el-tag>
        <p class="subtle">{{ result.size }} 字节 · SHA-256 已核验</p>
        <pre class="mcp-json">{{ result.content }}</pre>
      </template>
    </div>
    <template #footer><el-button @click="closeResult">关闭结果</el-button></template></el-dialog
  >
  <el-dialog
    v-model="orphanOpen"
    class="mcp-dialog"
    title="确认遗留程序清理"
    width="580px"
    :close-on-click-modal="false"
    ><p>
      请先在宿主核对并清理原程序。该操作只记录运维声明，不会根据旧 PID 终止进程或重发未知调用。
    </p>
    <el-checkbox v-model="orphan.confirmed">我已在宿主确认遗留程序清理</el-checkbox
    ><el-input
      v-model="orphan.reason"
      type="textarea"
      :rows="3"
      maxlength="200"
      placeholder="填写核对说明"
    /><el-alert v-if="orphanError" :title="orphanError" type="error" :closable="false" /><template
      #footer
      ><el-button @click="orphanOpen = false">保留当前状态</el-button
      ><el-button
        type="warning"
        :disabled="blocked || !orphan.confirmed || !orphan.reason.trim()"
        @click="confirmOrphan"
        >记录清理声明</el-button
      ></template
    ></el-dialog
  >
  <PluginSourcePicker v-model="sourcePickerOpen" @select="chooseSource" />
</template>
<style scoped>
:global(.mcp-dialog h3) {
  overflow-wrap: anywhere;
}
.plugin-binding {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 10px;
  overflow-wrap: anywhere;
}
:global(.mcp-dialog) {
  max-width: calc(100vw - 32px);
}
.mcp-summary,
.mcp-controls {
  display: flex;
  gap: 12px;
  align-items: center;
  flex-wrap: wrap;
  margin: 18px 0;
}
.mcp-json {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  max-height: 50vh;
  overflow: auto;
  padding: 18px;
  border-radius: 12px;
  background: var(--el-fill-color-light);
  font-size: 12px;
  line-height: 1.7;
}
.recovery {
  padding: 20px;
  margin-bottom: 18px;
}
.pending-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 0;
  border-top: 1px solid var(--el-border-color-lighter);
}
.pending-row > div {
  flex: 1;
  min-width: 0;
}
.table-secondary {
  display: block;
  margin-top: 6px;
  font-size: 12px;
}
.table-toolbar .el-input {
  max-width: 340px;
}
.mcp-history code {
  font-size: 11px;
  overflow-wrap: anywhere;
}
.mcp-controls .el-button {
  margin-left: 0;
}
.el-pagination {
  margin-top: 18px;
}
.el-select {
  width: 100%;
}
@media (max-width: 700px) {
  .pending-row {
    flex-wrap: wrap;
  }
  .pending-row > div {
    flex-basis: 100%;
  }
  .page-heading {
    align-items: flex-start;
    gap: 16px;
  }
  .mcp-controls {
    gap: 6px;
  }
}
</style>
