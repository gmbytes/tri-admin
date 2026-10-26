<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { useSession } from '../stores/session'
import { ApiError } from '../core/client'
import { createBrowserFileSink } from '../core/files'
import { parsePage } from '../core/open-access'
import {
  ExtensionRecoveryStore,
  parseExtension,
  parseRevision,
  parseConstraints,
  parsePreview,
  verifyPreview,
  verifyReceipt,
  extensionName,
  extensionPath,
  encodeBytes,
  type Extension,
  type ExtensionRevision,
  type Constraints,
  type ExtensionOperation,
} from '../core/extensions'
import { errorText, dateText } from '../ui'
const session = useSession(),
  identity = computed(() => `${session.settings.apiBaseUrl}|${session.user?.id ?? ''}`),
  canWrite = computed(() => session.can('skills.write')),
  canConstraints = computed(
    () => session.can('skills.constraints.write') && session.user?.dataScope === 'all',
  )
const rows = ref<Extension[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  status = ref('active'),
  loading = ref(false),
  error = ref(''),
  writing = ref(false),
  pending = ref<ExtensionOperation[]>([]),
  recoveryError = ref(''),
  recovering = ref(''),
  tab = ref('library')
const createOpen = ref(false),
  createError = ref(''),
  skill = reactive({
    name: '',
    description: '',
    markdown: '',
    license: '',
    compatibility: '',
    allowedTools: '',
    metadata: '{}',
  }),
  zipOpen = ref(false),
  zipError = ref(''),
  zipName = ref(''),
  zipBytes = ref(''),
  zipReading = ref(false),
  zipTarget = ref<Extension | null>(null)
const managerOpen = ref(false),
  selected = ref<Extension | null>(null),
  revision = ref<ExtensionRevision | null>(null),
  history = ref<ExtensionRevision[]>([]),
  historyPage = ref(1),
  historyTotal = ref(0),
  filePath = ref(''),
  content = ref(''),
  fileLoading = ref(false),
  fileVerified = ref(false),
  managerError = ref(''),
  downloading = ref('')
const fileOpen = ref(false),
  fileError = ref(''),
  fileForm = reactive({ path: '', content: '', mode: 420, kind: 'text', base64: '', filename: '' }),
  fileReading = ref(false),
  replacing = ref(false)
const constraint = ref<Constraints | null>(null),
  constraintDraft = ref(''),
  constraintError = ref(''),
  constraintHistory = ref<Constraints[]>([]),
  constraintPage = ref(1),
  constraintTotal = ref(0),
  constraintLoading = ref(false)
const blocked = computed(() => writing.value || pending.value.length > 0 || !!recoveryError.value),
  selectedFile = computed(() =>
    revision.value?.package.files.find((f) => f.path === filePath.value),
  ),
  editable = computed(
    () =>
      canWrite.value &&
      selected.value?.deletedAt === null &&
      selected.value.revisionId === revision.value?.id,
  )
let alive = true,
  epoch = 0,
  listSequence = 0,
  fileSequence = 0,
  managerSequence = 0,
  constraintSequence = 0,
  store: ExtensionRecoveryStore | null = null,
  fileControl: AbortController | null = null
const controls = new Set<AbortController>(),
  ticket = () => epoch,
  current = (expected: number) => alive && expected === epoch
function readPending() {
  try {
    pending.value = store?.read() ?? []
    recoveryError.value = ''
  } catch {
    pending.value = []
    recoveryError.value = '本机恢复记录损坏或不可用，暂时不能写入扩展'
  }
}
function reset() {
  epoch++
  listSequence++
  fileSequence++
  managerSequence++
  constraintSequence++
  for (const control of controls) control.abort()
  controls.clear()
  rows.value = []
  total.value = 0
  page.value = 1
  search.value = ''
  status.value = 'active'
  pending.value = []
  store = null
  recoveryError.value = ''
  createOpen.value = false
  zipOpen.value = false
  managerOpen.value = false
  fileOpen.value = false
  selected.value = null
  revision.value = null
  history.value = []
  content.value = ''
  zipBytes.value = ''
  zipName.value = ''
  fileForm.content = ''
  fileForm.base64 = ''
  constraint.value = null
  constraintDraft.value = ''
  constraintHistory.value = []
  writing.value = false
  loading.value = false
  error.value = ''
  managerError.value = ''
  constraintError.value = ''
  ElMessageBox.close()
  if (session.user) {
    try {
      store = new ExtensionRecoveryStore(localStorage, session.settings.apiBaseUrl, session.user.id)
      readPending()
    } catch {
      recoveryError.value = '无法保存本机恢复记录，未发送写入请求'
    }
  }
}
watch(
  identity,
  () => {
    reset()
    void Promise.all([load(), loadConstraints()])
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  alive = false
  reset()
})
async function request(
  path: string,
  options: { method?: string; body?: unknown } = {},
  control = new AbortController(),
) {
  const expected = ticket()
  controls.add(control)
  try {
    const value = await session.request<unknown>(path, { ...options, signal: control.signal })
    if (!current(expected)) throw new ApiError('REQUEST_CANCELLED', '账号或服务已变化')
    return value
  } finally {
    controls.delete(control)
  }
}
async function load() {
  const expected = ticket(),
    sequence = ++listSequence
  loading.value = true
  error.value = ''
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
      search: search.value,
      status: status.value,
    })
    const result = parsePage(await request(`/extensions?${query}`), parseExtension)
    if (current(expected) && sequence === listSequence) {
      rows.value = result.data
      total.value = result.meta.total
    }
  } catch (failure) {
    if (current(expected) && sequence === listSequence) error.value = errorText(failure)
  } finally {
    if (current(expected) && sequence === listSequence) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
function setStatus(value: string) {
  status.value = value
  query()
}
function clearZip() {
  zipBytes.value = ''
  zipName.value = ''
}
function clearFileDraft() {
  fileForm.base64 = ''
  fileForm.content = ''
}
function confirmed(id: string, expected: number) {
  if (!current(expected)) return
  try {
    store?.remove(id)
    readPending()
  } catch {
    recoveryError.value = '结果已确认，但本机标记移除失败，请恢复查询'
  }
}
async function recover(op: ExtensionOperation) {
  const expected = ticket()
  recovering.value = op.requestId
  try {
    const value = (await request(`/extensions/by-request/${op.requestId}`)).data,
      row = parseExtension(value),
      snapshot = (await request(`/extensions/${row.id}/revisions/${row.revisionId}`)).data
    verifyReceipt(value, snapshot, op, session.user?.id ?? '')
    confirmed(op.requestId, expected)
    if (current(expected)) {
      await load()
      ElMessage.success('原请求已确认，没有重新提交')
    }
  } catch (failure) {
    if (current(expected)) error.value = errorText(failure)
  } finally {
    if (current(expected)) recovering.value = ''
  }
}
async function forget(op?: ExtensionOperation) {
  const expected = ticket()
  try {
    await ElMessageBox.confirm(
      '先核对服务器中的扩展及修订历史。移除本机标记不会撤销或重新发送任何请求。',
      '移除扩展恢复标记',
      { type: 'warning', confirmButtonText: '已核对，移除标记', cancelButtonText: '保留' },
    )
    if (!current(expected)) return
    if (op) confirmed(op.requestId, expected)
    else {
      store?.clear()
      readPending()
    }
  } catch (failure) {
    if (current(expected) && failure !== 'cancel' && failure !== 'close')
      recoveryError.value = errorText(failure)
  }
}
async function sourceWrite(source: unknown, target: Extension | null, preferredPath?: string) {
  if (blocked.value || !canWrite.value) throw new Error('请先处理待确认操作')
  const expected = ticket()
  if (!store) throw new Error('本机恢复记录不可用')
  const op: ExtensionOperation = {
    requestId: crypto.randomUUID(),
    kind: target ? 'revise' : 'install',
    extensionId: target?.id ?? null,
    version: target?.version ?? null,
    createdAt: new Date().toISOString(),
  }
  store.put(op)
  readPending()
  writing.value = true
  let accepted = false
  try {
    const path = target ? `/extensions/${target.id}/revisions` : '/extensions',
      body = target
        ? { requestId: op.requestId, version: target.version, source }
        : { requestId: op.requestId, source }
    const response = await request(path, { method: 'POST', body })
    accepted = true
    const raw = response.data,
      row = parseExtension(raw),
      snapshot = (await request(`/extensions/${row.id}/revisions/${row.revisionId}`)).data
    verifyReceipt(raw, snapshot, op, session.user?.id ?? '')
    confirmed(op.requestId, expected)
    if (!current(expected)) return
    createOpen.value = false
    zipOpen.value = false
    fileOpen.value = false
    zipBytes.value = ''
    fileForm.base64 = ''
    await load()
    if (target && managerOpen.value && selected.value?.id === target.id)
      await openManager(row, preferredPath)
    ElMessage.success('内容已保存为独立修订，扩展当前停用')
  } catch (failure) {
    if (
      current(expected) &&
      !accepted &&
      failure instanceof ApiError &&
      [400, 401, 403, 404, 409, 413, 415, 422, 429].includes(failure.status)
    )
      confirmed(op.requestId, expected)
    throw failure
  } finally {
    if (current(expected)) writing.value = false
  }
}
function newSkill() {
  Object.assign(skill, {
    name: '',
    description: '',
    markdown: '',
    license: '',
    compatibility: '',
    allowedTools: '',
    metadata: '{}',
  })
  createError.value = ''
  createOpen.value = true
}
async function saveSkill() {
  createError.value = ''
  try {
    if (!extensionName(skill.name) || !skill.description.trim())
      throw new Error('请填写规范名称和描述')
    const metadata: unknown = JSON.parse(skill.metadata)
    if (
      !metadata ||
      typeof metadata !== 'object' ||
      Array.isArray(metadata) ||
      Object.values(metadata).some((v) => typeof v !== 'string')
    )
      throw new Error('metadata 必须是字符串映射')
    await sourceWrite(
      {
        kind: 'skill',
        meta: {
          name: skill.name,
          description: skill.description,
          license: skill.license || null,
          compatibility: skill.compatibility || null,
          allowedTools: skill.allowedTools || null,
          metadata,
        },
        markdown: skill.markdown,
        files: [],
      },
      null,
    )
  } catch (failure) {
    createError.value = errorText(failure)
  }
}
function openZip(target: Extension | null) {
  zipTarget.value = target
  zipName.value = ''
  zipBytes.value = ''
  zipError.value = ''
  zipOpen.value = true
}
async function readZip(event: Event) {
  const expected = ticket(),
    input = event.target as HTMLInputElement,
    file = input.files?.[0]
  zipBytes.value = ''
  zipName.value = ''
  zipError.value = ''
  if (!file) return
  zipReading.value = true
  try {
    if (file.size === 0 || file.size > 4 * 1024 * 1024) throw new Error('ZIP 必须为 1 字节至 4 MiB')
    const encoded = encodeBytes(new Uint8Array(await file.arrayBuffer()))
    if (current(expected) && zipOpen.value) {
      zipBytes.value = encoded
      zipName.value = file.name
    }
  } catch (failure) {
    if (current(expected)) zipError.value = errorText(failure)
  } finally {
    input.value = ''
    if (current(expected)) zipReading.value = false
  }
}
async function installZip() {
  zipError.value = ''
  try {
    if (!zipBytes.value || zipReading.value) throw new Error('请先选择 ZIP')
    await sourceWrite({ kind: 'archive', contentBase64: zipBytes.value }, zipTarget.value)
  } catch (failure) {
    zipError.value = errorText(failure)
  }
}
async function toggle(row: Extension) {
  if (blocked.value) return
  const expected = ticket()
  writing.value = true
  try {
    parseExtension(
      (
        await request(`/extensions/${row.id}`, {
          method: 'PATCH',
          body: { version: row.version, enabled: !row.enabled },
        })
      ).data,
    )
    await load()
  } catch (failure) {
    if (current(expected)) error.value = errorText(failure) + '；请刷新核对当前状态'
  } finally {
    if (current(expected)) writing.value = false
  }
}
async function uninstall(row: Extension) {
  const expected = ticket()
  try {
    await ElMessageBox.confirm(`卸载“${row.name}”后停用扩展，历史修订仍保留。`, '卸载扩展', {
      type: 'warning',
      confirmButtonText: '卸载',
      cancelButtonText: '取消',
    })
    if (!current(expected) || blocked.value) return
    writing.value = true
    await request(`/extensions/${row.id}?version=${row.version}`, { method: 'DELETE' })
    if (current(expected)) await load()
  } catch (failure) {
    if (current(expected) && failure !== 'cancel' && failure !== 'close')
      error.value = errorText(failure) + '；请刷新核对当前状态'
  } finally {
    if (current(expected)) writing.value = false
  }
}
async function openManager(row: Extension, preferredPath?: string) {
  const expected = ticket(),
    sequence = ++managerSequence
  selected.value = row
  revision.value = null
  content.value = ''
  filePath.value = ''
  history.value = []
  historyPage.value = 1
  managerError.value = ''
  managerOpen.value = true
  try {
    const rev = parseRevision(
      (await request(`/extensions/${row.id}/revisions/${row.revisionId}`)).data,
    )
    if (!current(expected) || sequence !== managerSequence) return
    revision.value = rev
    filePath.value =
      preferredPath && rev.package.files.some((f) => f.path === preferredPath)
        ? preferredPath
        : (rev.package.files.find((f) => f.path === 'SKILL.md')?.path ??
          rev.package.files[0]?.path ??
          '')
    await Promise.all([loadFile(), loadHistory()])
  } catch (failure) {
    if (current(expected) && sequence === managerSequence) managerError.value = errorText(failure)
  }
}
async function loadHistory() {
  const expected = ticket(),
    row = selected.value,
    sequence = managerSequence
  if (!row) return
  try {
    const result = parsePage(
      await request(`/extensions/${row.id}/revisions?page=${historyPage.value}&pageSize=20`),
      parseRevision,
    )
    if (current(expected) && sequence === managerSequence) {
      if (result.data.some((r) => r.extensionId !== row.id)) throw new Error('修订归属不匹配')
      history.value = result.data
      historyTotal.value = result.meta.total
    }
  } catch (failure) {
    if (current(expected) && sequence === managerSequence) managerError.value = errorText(failure)
  }
}
async function selectRevision(row: ExtensionRevision) {
  if (row.extensionId !== selected.value?.id) return
  revision.value = row
  filePath.value =
    row.package.files.find((f) => f.path === 'SKILL.md')?.path ?? row.package.files[0]?.path ?? ''
  await loadFile()
}
async function loadFile() {
  fileControl?.abort()
  const control = new AbortController()
  fileControl = control
  const expected = ticket(),
    sequence = ++fileSequence,
    row = selected.value,
    rev = revision.value,
    file = selectedFile.value
  content.value = ''
  fileVerified.value = false
  managerError.value = ''
  fileLoading.value = false
  if (!row || !rev || !file || !file.text) return
  fileLoading.value = true
  try {
    const value = parsePreview(
      (
        await request(
          `/extensions/${row.id}/revisions/${rev.id}/preview?${new URLSearchParams({ path: file.path })}`,
          {},
          control,
        )
      ).data,
      file,
    )
    await verifyPreview(value, file)
    if (current(expected) && sequence === fileSequence && managerOpen.value) {
      content.value = value.content
      fileVerified.value = true
    }
  } catch (failure) {
    if (current(expected) && sequence === fileSequence) managerError.value = errorText(failure)
  } finally {
    if (current(expected) && sequence === fileSequence) fileLoading.value = false
  }
}
function closeManager() {
  managerSequence++
  fileSequence++
  fileControl?.abort()
  revision.value = null
  selected.value = null
  content.value = ''
  history.value = []
  fileOpen.value = false
}
async function saveFile() {
  if (!editable.value || !selectedFile.value || !fileVerified.value) return
  const file = selectedFile.value
  try {
    await sourceWrite(
      {
        kind: 'files',
        write: [
          {
            path: file.path,
            mode: file.mode,
            contentBase64: encodeBytes(new TextEncoder().encode(content.value)),
          },
        ],
        remove: [],
      },
      selected.value,
      file.path,
    )
  } catch (failure) {
    managerError.value = errorText(failure)
  }
}
function addFile(replace = false) {
  if (!editable.value) return
  const file = selectedFile.value
  replacing.value = replace
  Object.assign(fileForm, {
    path: replace ? (file?.path ?? '') : 'templates/',
    content: '',
    mode: replace ? (file?.mode ?? 420) : 420,
    kind: replace ? 'upload' : 'text',
    base64: '',
    filename: '',
  })
  fileError.value = ''
  fileOpen.value = true
}
async function readResource(event: Event) {
  const expected = ticket(),
    input = event.target as HTMLInputElement,
    file = input.files?.[0]
  fileForm.base64 = ''
  fileForm.filename = ''
  if (!file) return
  fileReading.value = true
  try {
    if (file.size > 4 * 1024 * 1024) throw new Error('资源文件不能超过 4 MiB')
    const value = encodeBytes(new Uint8Array(await file.arrayBuffer()))
    if (current(expected) && fileOpen.value) {
      fileForm.base64 = value
      fileForm.filename = file.name
    }
  } catch (failure) {
    if (current(expected)) fileError.value = errorText(failure)
  } finally {
    input.value = ''
    if (current(expected)) fileReading.value = false
  }
}
async function saveAddedFile() {
  fileError.value = ''
  try {
    if (!editable.value || !extensionPath(fileForm.path)) throw new Error('文件路径无效')
    if (
      !replacing.value &&
      revision.value?.package.files.some(
        (f) => f.path.toLowerCase() === fileForm.path.toLowerCase(),
      )
    )
      throw new Error('文件已存在，请选择该文件编辑或替换')
    if (fileReading.value || (fileForm.kind === 'upload' && !fileForm.filename))
      throw new Error('请先选择资源文件')
    const bytes =
      fileForm.kind === 'upload'
        ? fileForm.base64
        : encodeBytes(new TextEncoder().encode(fileForm.content))
    await sourceWrite(
      {
        kind: 'files',
        write: [{ path: fileForm.path, mode: fileForm.mode, contentBase64: bytes }],
        remove: [],
      },
      selected.value,
      fileForm.path,
    )
  } catch (failure) {
    fileError.value = errorText(failure)
  }
}
async function removeFile() {
  const row = selected.value,
    file = selectedFile.value,
    expected = ticket()
  if (!editable.value || !row || !file) return
  try {
    await ElMessageBox.confirm(`从新修订中移除 ${file.path}，原修订仍保留。`, '移除扩展文件', {
      confirmButtonText: '移除文件',
      cancelButtonText: '取消',
      type: 'warning',
    })
    if (current(expected)) await sourceWrite({ kind: 'files', write: [], remove: [file.path] }, row)
  } catch (failure) {
    if (current(expected) && failure !== 'cancel' && failure !== 'close')
      managerError.value = errorText(failure)
  }
}
async function reloadCurrent() {
  const row = selected.value,
    expected = ticket()
  if (!row || row.deletedAt) return
  try {
    await ElMessageBox.confirm('重新载入会放弃未保存的文件内容。', '重新载入扩展', {
      confirmButtonText: '重新载入',
      cancelButtonText: '保留编辑',
    })
    const value = parseExtension((await request(`/extensions/${row.id}`)).data)
    if (current(expected)) await openManager(value)
  } catch (failure) {
    if (current(expected) && failure !== 'cancel' && failure !== 'close')
      managerError.value = errorText(failure)
  }
}
async function download(row: ExtensionRevision) {
  const expected = ticket(),
    control = new AbortController()
  controls.add(control)
  downloading.value = row.id
  try {
    const spec = { size: row.package.archiveSize, sha256: row.package.archiveSha256 }
    const blob = await session.downloadExtension(
      row.extensionId,
      row.id,
      spec,
      createBrowserFileSink(spec),
      { signal: control.signal },
    )
    if (!current(expected)) return
    const url = URL.createObjectURL(new Blob([blob], { type: 'application/zip' })),
      anchor = document.createElement('a')
    anchor.href = url
    anchor.download = `extension-${row.extensionId}-${row.number}.zip`
    document.body.append(anchor)
    anchor.click()
    anchor.remove()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
    ElMessage.success('原包摘要已核对，下载已开始')
  } catch (failure) {
    if (current(expected)) managerError.value = errorText(failure)
  } finally {
    controls.delete(control)
    if (current(expected)) downloading.value = ''
  }
}
async function loadConstraints() {
  const expected = ticket(),
    sequence = ++constraintSequence
  constraintLoading.value = true
  constraintError.value = ''
  try {
    const value = parseConstraints((await request('/skill-constraints')).data),
      history = parsePage(
        await request(`/skill-constraints/revisions?page=${constraintPage.value}&pageSize=20`),
        parseConstraints,
      )
    if (current(expected) && sequence === constraintSequence) {
      constraint.value = value
      constraintDraft.value = value.content
      constraintHistory.value = history.data
      constraintTotal.value = history.meta.total
    }
  } catch (failure) {
    if (current(expected) && sequence === constraintSequence)
      constraintError.value = errorText(failure)
  } finally {
    if (current(expected) && sequence === constraintSequence) constraintLoading.value = false
  }
}
async function saveConstraints() {
  if (!constraint.value || !canConstraints.value || blocked.value) return
  const expected = ticket()
  writing.value = true
  constraintError.value = ''
  try {
    parseConstraints(
      (
        await request('/skill-constraints', {
          method: 'PATCH',
          body: { version: constraint.value.version, content: constraintDraft.value },
        })
      ).data,
    )
    await loadConstraints()
    if (current(expected)) ElMessage.success('全局约束已保存，新版本已记录')
  } catch (failure) {
    if (current(expected))
      constraintError.value = errorText(failure) + '；请载入当前约束核对后再提交'
  } finally {
    if (current(expected)) writing.value = false
  }
}
onMounted(() => {
  reset()
  void Promise.all([load(), loadConstraints()])
})
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">EXTENSION LIBRARY</div>
      <h1>Skills 与插件</h1>
      <p>维护说明、脚本、资源和模板；每次内容修改保存独立修订，检查后再启用。</p>
    </div>
    <div class="heading-actions">
      <el-button v-if="canWrite" :disabled="blocked" @click="openZip(null)">安装 ZIP</el-button
      ><el-button v-if="canWrite" type="primary" :disabled="blocked" @click="newSkill"
        >新建 Skill</el-button
      >
    </div>
  </div>
  <el-alert v-if="error" :title="error" type="error" :closable="false" />
  <section v-if="pending.length || recoveryError" class="surface recovery">
    <h3 v-if="pending.length">待确认扩展操作</h3>
    <el-alert v-if="recoveryError" :title="recoveryError" type="error" :closable="false" />
    <p>响应丢失时查询原请求与修订，不重新安装或保存；未查到时可稍后重查。</p>
    <div v-for="op in pending" :key="op.requestId" class="pending-row">
      <code>{{ op.requestId }}</code
      ><el-button :loading="recovering === op.requestId" @click="recover(op)">恢复查询</el-button
      ><el-button text @click="forget(op)">移除标记</el-button>
    </div>
    <el-button v-if="recoveryError" @click="readPending">重读本机记录</el-button
    ><el-button v-if="recoveryError" @click="forget()">清除本机标记</el-button>
  </section>
  <el-tabs v-model="tab"
    ><el-tab-pane label="扩展库" name="library"
      ><section class="surface table-surface">
        <div class="table-toolbar">
          <el-button
            :type="status === 'active' ? 'primary' : 'default'"
            @click="setStatus('active')"
            >已安装</el-button
          ><el-button
            :type="status === 'uninstalled' ? 'primary' : 'default'"
            @click="setStatus('uninstalled')"
            >已卸载</el-button
          ><el-input
            v-model="search"
            placeholder="按扩展名称前缀搜索"
            class="search-input"
            clearable
            @keyup.enter="query"
          /><el-button @click="query">查询</el-button>
          <div class="toolbar-spacer" />
          <el-button @click="load">刷新扩展</el-button>
        </div>
        <el-table :data="rows" row-key="id" v-loading="loading" class="extensions-table"
          ><el-table-column label="扩展" min-width="230"
            ><template #default="{ row }"
              ><strong>{{ row.name }}</strong
              ><span class="table-secondary">{{ row.package.description }}</span
              ><code class="table-secondary">{{ row.id }}</code></template
            ></el-table-column
          ><el-table-column label="类型" width="95"
            ><template #default="{ row }">{{
              row.kind === 'skill' ? 'Skill' : '插件'
            }}</template></el-table-column
          ><el-table-column label="状态" width="90"
            ><template #default="{ row }"
              ><el-tag :type="row.enabled ? 'success' : 'info'">{{
                row.deletedAt ? '已卸载' : row.enabled ? '启用' : '停用'
              }}</el-tag></template
            ></el-table-column
          ><el-table-column label="版本" width="125"
            ><template #default="{ row }"
              >版本 {{ row.version }} / 修订 {{ row.revision }}</template
            ></el-table-column
          ><el-table-column label="更新时间" min-width="165"
            ><template #default="{ row }">{{ dateText(row.updatedAt) }}</template></el-table-column
          ><el-table-column label="操作" width="325" fixed="right"
            ><template #default="{ row }"
              ><el-button text @click="openManager(row as Extension)">{{
                row.deletedAt ? '查看历史' : '管理文件'
              }}</el-button
              ><template v-if="canWrite && !row.deletedAt"
                ><el-button text :disabled="blocked" @click="toggle(row as Extension)">{{
                  row.enabled ? '停用' : '启用'
                }}</el-button
                ><el-button text :disabled="blocked" @click="openZip(row as Extension)"
                  >更新 ZIP</el-button
                ><el-button
                  text
                  type="danger"
                  :disabled="blocked"
                  @click="uninstall(row as Extension)"
                  >卸载</el-button
                ></template
              ></template
            ></el-table-column
          ></el-table
        ><el-pagination
          v-model:current-page="page"
          v-model:page-size="pageSize"
          :total="total"
          :page-sizes="[10, 20, 50]"
          layout="total, sizes, prev, pager, next"
          @current-change="load"
          @size-change="query"
        /></section
    ></el-tab-pane>
    <el-tab-pane label="全局约束" name="constraints"
      ><section class="surface table-surface" v-loading="constraintLoading">
        <div class="table-toolbar">
          <h3>全局约束 · 版本 {{ constraint?.version ?? '—' }}</h3>
          <div class="toolbar-spacer" />
          <el-button @click="loadConstraints">重新载入约束</el-button
          ><el-button
            v-if="canConstraints"
            type="primary"
            :disabled="blocked || !constraint"
            @click="saveConstraints"
            >保存全局约束</el-button
          >
        </div>
        <p class="subtle">
          所有扩展共用的约束说明。修改需要全局约束权限及全部数据范围；文本作为规则数据保存。
        </p>
        <el-alert
          v-if="constraintError"
          :title="constraintError"
          type="error"
          :closable="false"
        /><el-input
          v-model="constraintDraft"
          type="textarea"
          :rows="10"
          :readonly="!canConstraints"
          aria-label="全局约束内容"
        />
        <h3>约束修订历史</h3>
        <el-table :data="constraintHistory"
          ><el-table-column prop="version" label="版本" width="90" /><el-table-column
            label="时间"
            width="190"
            ><template #default="{ row }">{{ dateText(row.updatedAt) }}</template></el-table-column
          ><el-table-column label="内容"
            ><template #default="{ row }">
              <pre class="constraint-content">{{ row.content || '（空）' }}</pre>
            </template></el-table-column
          ></el-table
        ><el-pagination
          v-model:current-page="constraintPage"
          :page-size="20"
          :total="constraintTotal"
          layout="total, prev, pager, next"
          @current-change="loadConstraints"
        /></section></el-tab-pane
  ></el-tabs>
  <el-dialog
    class="extension-dialog"
    v-model="createOpen"
    title="新建 Skill"
    width="720px"
    destroy-on-close
    :close-on-click-modal="false"
    ><el-alert v-if="createError" :title="createError" type="error" :closable="false" /><el-form
      label-position="top"
      @submit.prevent="saveSkill"
      ><el-form-item label="名称"
        ><el-input
          v-model="skill.name"
          aria-label="名称"
          maxlength="64"
          placeholder="record-helper" /></el-form-item
      ><el-form-item label="描述"
        ><el-input
          v-model="skill.description"
          aria-label="描述"
          type="textarea"
          :rows="2" /></el-form-item
      ><el-form-item label="Markdown 正文"
        ><el-input v-model="skill.markdown" aria-label="Markdown 正文" type="textarea" :rows="6"
      /></el-form-item>
      <div class="extension-grid">
        <el-form-item label="license"><el-input v-model="skill.license" /></el-form-item
        ><el-form-item label="compatibility"
          ><el-input v-model="skill.compatibility"
        /></el-form-item>
      </div>
      <el-form-item label="allowed-tools"
        ><el-input v-model="skill.allowedTools" placeholder="此字段不授予后台权限" /></el-form-item
      ><el-form-item label="metadata（JSON 字符串映射）"
        ><el-input v-model="skill.metadata" type="textarea" :rows="2" /></el-form-item></el-form
    ><template #footer
      ><el-button @click="createOpen = false">取消</el-button
      ><el-button type="primary" :loading="writing" :disabled="blocked" @click="saveSkill"
        >创建 Skill</el-button
      ></template
    ></el-dialog
  >
  <el-dialog
    class="extension-dialog"
    v-model="zipOpen"
    :title="zipTarget ? '更新扩展 ZIP' : '安装扩展 ZIP'"
    width="580px"
    destroy-on-close
    :close-on-click-modal="false"
    @closed="clearZip"
    ><p>选择包含单一根目录的 Skill 或 TRI 插件 ZIP，最大 4 MiB。新内容默认停用，不自动执行脚本。</p>
    <el-alert v-if="zipError" :title="zipError" type="error" :closable="false" /><input
      type="file"
      accept=".zip,application/zip"
      aria-label="选择扩展 ZIP"
      :disabled="writing"
      @change="readZip"
    />
    <p>{{ zipReading ? '正在读取…' : zipName }}</p>
    <template #footer
      ><el-button @click="zipOpen = false">取消</el-button
      ><el-button
        type="primary"
        :disabled="blocked || zipReading || !zipName"
        @click="installZip"
        >{{ zipTarget ? '保存 ZIP 修订' : '安装扩展' }}</el-button
      ></template
    ></el-dialog
  >
  <el-dialog
    class="extension-dialog"
    v-model="managerOpen"
    title="扩展文件与历史"
    width="min(1080px, 94vw)"
    destroy-on-close
    :close-on-click-modal="false"
    @closed="closeManager"
    ><template v-if="selected"
      ><div class="table-toolbar">
        <h3>{{ selected.name }}</h3>
        <el-tag>修订 {{ revision?.number ?? '—' }}</el-tag
        ><span class="subtle"
          >记录版本 {{ selected.version }}{{ selected.deletedAt ? ' · 已卸载' : '' }}</span
        >
        <div class="toolbar-spacer" />
        <el-button v-if="!selected.deletedAt" @click="reloadCurrent">重新载入当前版本</el-button>
      </div>
      <el-alert v-if="managerError" :title="managerError" type="error" :closable="false" /><el-alert
        v-if="!editable"
        title="当前修订只读，原始内容和历史仍可导出。"
        type="info"
        :closable="false" />
      <div class="extension-filebar">
        <el-select v-model="filePath" aria-label="修订文件" filterable @change="loadFile"
          ><el-option
            v-for="file in revision?.package.files ?? []"
            :key="file.path"
            :label="file.path"
            :value="file.path" /></el-select
        ><el-button v-if="editable" :disabled="blocked" @click="addFile(false)">新增文件</el-button
        ><el-button v-if="editable" :disabled="blocked || !selectedFile" @click="addFile(true)"
          >替换资源</el-button
        ><el-button
          v-if="editable"
          :disabled="blocked || !selectedFile"
          type="danger"
          @click="removeFile"
          >移除文件</el-button
        >
      </div>
      <p v-if="selectedFile" class="subtle">
        {{ selectedFile.size }} 字节 · SHA-256 {{ selectedFile.sha256 }}
      </p>
      <el-input
        v-if="selectedFile?.text"
        v-model="content"
        type="textarea"
        :rows="12"
        aria-label="文件内容"
        :readonly="!editable || fileLoading"
        v-loading="fileLoading"
        class="extension-source" /><el-empty
        v-else
        description="二进制或大文件不作文本预览，可替换资源或导出原包。" />
      <div class="extension-save">
        <el-button
          v-if="editable && selectedFile?.text"
          type="primary"
          :disabled="blocked || fileLoading || !fileVerified"
          @click="saveFile"
          >保存新修订</el-button
        >
      </div>
      <h3>修订历史</h3>
      <el-table :data="history" class="extension-revisions" row-key="id"
        ><el-table-column label="修订" width="90"
          ><template #default="{ row }">{{ row.number }}</template></el-table-column
        ><el-table-column label="标识" min-width="230"
          ><template #default="{ row }"
            ><code>{{ row.id }}</code></template
          ></el-table-column
        ><el-table-column label="时间" min-width="180"
          ><template #default="{ row }">{{ dateText(row.createdAt) }}</template></el-table-column
        ><el-table-column label="操作" width="195"
          ><template #default="{ row }"
            ><el-button text @click="selectRevision(row as ExtensionRevision)">查看文件</el-button
            ><el-button
              text
              :loading="downloading === row.id"
              @click="download(row as ExtensionRevision)"
              >导出 ZIP</el-button
            ></template
          ></el-table-column
        ></el-table
      ><el-pagination
        v-model:current-page="historyPage"
        :page-size="20"
        :total="historyTotal"
        layout="total, prev, pager, next"
        @current-change="loadHistory" /></template
  ></el-dialog>
  <el-dialog
    class="extension-dialog"
    v-model="fileOpen"
    title="新增扩展文件"
    width="640px"
    destroy-on-close
    :close-on-click-modal="false"
    @closed="clearFileDraft"
    ><el-alert v-if="fileError" :title="fileError" type="error" :closable="false" /><el-form
      label-position="top"
      ><el-form-item label="文件路径"
        ><el-input
          v-model="fileForm.path"
          aria-label="文件路径"
          :readonly="replacing"
          placeholder="scripts/、resources/、references/、templates/ 或 assets/" /></el-form-item
      ><el-radio-group v-model="fileForm.kind"
        ><el-radio-button value="text">文本内容</el-radio-button
        ><el-radio-button value="upload">本机文件</el-radio-button></el-radio-group
      ><el-form-item v-if="fileForm.kind === 'text'" label="新增文件内容"
        ><el-input v-model="fileForm.content" type="textarea" :rows="8" aria-label="新增文件内容"
      /></el-form-item>
      <div v-else class="resource-upload">
        <input type="file" aria-label="选择资源文件" @change="readResource" />
        <p>{{ fileReading ? '正在读取…' : fileForm.filename }}</p>
      </div>
      <p class="subtle">资源作为数据保存；脚本不会在安装或编辑时执行。</p></el-form
    ><template #footer
      ><el-button @click="fileOpen = false">取消</el-button
      ><el-button type="primary" :disabled="blocked || fileReading" @click="saveAddedFile"
        >保存文件修订</el-button
      ></template
    ></el-dialog
  >
</template>
<style scoped>
.heading-actions,
.pending-row,
.extension-filebar {
  display: flex;
  gap: 10px;
  align-items: center;
}
.pending-row {
  margin: 12px 0;
  flex-wrap: wrap;
}
.recovery {
  padding: 18px;
  margin-bottom: 20px;
}
.extension-filebar {
  margin: 16px 0;
  flex-wrap: wrap;
}
.extension-filebar .el-select {
  min-width: 250px;
  flex: 1;
}
.extension-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 16px;
}
.extension-save {
  display: flex;
  justify-content: flex-end;
  margin: 12px 0;
}
.extension-source :deep(textarea) {
  font-family: Consolas, monospace;
}
.constraint-content {
  white-space: pre-wrap;
  max-height: 180px;
  overflow: auto;
  word-break: break-word;
}
.resource-upload {
  padding: 20px 0;
}
.subtle {
  overflow-wrap: anywhere;
}
:global(.extension-dialog.el-dialog) {
  max-height: 92vh;
  display: flex;
  flex-direction: column;
  margin-top: 4vh !important;
}
:global(.extension-dialog .el-dialog__body) {
  overflow: auto;
  flex: 1;
}
:global(.extension-dialog .el-dialog__header),
:global(.extension-dialog .el-dialog__footer) {
  flex-shrink: 0;
}
@media (max-width: 720px) {
  .extension-grid {
    grid-template-columns: 1fr;
  }
  .heading-actions {
    flex-wrap: wrap;
  }
  .extension-filebar .el-select {
    min-width: 180px;
  }
}
</style>
