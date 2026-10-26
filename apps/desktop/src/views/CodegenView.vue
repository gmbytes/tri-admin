<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { useSession } from '../stores/session'
import { ApiError } from '../core/client'
import { createBrowserFileSink } from '../core/files'
import { parsePage } from '../core/open-access'
import {
  CodegenRecoveryStore,
  fieldKinds,
  parseSchema,
  parseDefinition,
  parseGeneration,
  parseSource,
  verifySource,
  type CodeDefinition,
  type CodeGeneration,
  type CodeField,
  type CodegenOperation,
  type GeneratedFile,
} from '../core/codegen'
import { errorText, dateText } from '../ui'
const session = useSession(),
  identity = computed(() => `${session.settings.apiBaseUrl}|${session.user?.id ?? ''}`),
  canWrite = computed(() => session.can('codegen.write'))
const definitions = ref<CodeDefinition[]>([]),
  history = ref<CodeGeneration[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  historyPage = ref(1),
  historySize = ref(20),
  historyTotal = ref(0),
  search = ref(''),
  historySearch = ref(''),
  loading = ref(false),
  historyLoading = ref(false),
  writing = ref(false),
  error = ref(''),
  formError = ref(''),
  open = ref(false),
  editing = ref<CodeDefinition | null>(null),
  previewOpen = ref(false),
  selected = ref<CodeGeneration | null>(null),
  filePath = ref(''),
  source = ref(''),
  sourceLoading = ref(false),
  sourceError = ref(''),
  downloading = ref(''),
  pending = ref<CodegenOperation[]>([]),
  recoveryError = ref(''),
  recovering = ref('')
type FieldDraft = CodeField & { key: string }
const form = reactive({ resource: '', label: '', fields: [] as FieldDraft[] }),
  blocked = computed(() => writing.value || !!pending.value.length || !!recoveryError.value)
const labels = {
  text: '文本',
  integer: '整数',
  number: '小数',
  boolean: '布尔',
  datetime: '时间',
  textList: '文本列表',
}
let epoch = 0,
  alive = true,
  definitionSequence = 0,
  historySequence = 0,
  sourceSequence = 0,
  store: CodegenRecoveryStore | null = null,
  sourceController: AbortController | null = null
const controls = new Set<AbortController>(),
  ticket = () => epoch,
  current = (expected: number) => alive && epoch === expected
function readPending() {
  try {
    if (!store && session.user)
      store = new CodegenRecoveryStore(localStorage, session.settings.apiBaseUrl, session.user.id)
    pending.value = store?.read() ?? []
    recoveryError.value = ''
  } catch {
    pending.value = []
    recoveryError.value = '本机恢复记录不可用，请核对历史结果后处理本机记录'
  }
}
function reset() {
  epoch++
  definitionSequence++
  historySequence++
  sourceSequence++
  for (const control of controls) control.abort()
  controls.clear()
  definitions.value = []
  history.value = []
  total.value = 0
  historyTotal.value = 0
  open.value = false
  editing.value = null
  form.resource = ''
  form.label = ''
  form.fields = []
  search.value = ''
  historySearch.value = ''
  page.value = 1
  historyPage.value = 1
  filePath.value = ''
  sourceLoading.value = false
  recoveryError.value = ''
  previewOpen.value = false
  selected.value = null
  source.value = ''
  error.value = ''
  formError.value = ''
  sourceError.value = ''
  writing.value = false
  loading.value = false
  historyLoading.value = false
  downloading.value = ''
  pending.value = []
  recovering.value = ''
  store = null
  if (session.user) {
    try {
      store = new CodegenRecoveryStore(localStorage, session.settings.apiBaseUrl, session.user.id)
      readPending()
    } catch {
      recoveryError.value = '无法保存本机恢复记录，暂时不能创建或生成'
    }
  }
  ElMessageBox.close()
}
watch(identity, reset, { immediate: true, flush: 'sync' })
onBeforeUnmount(() => {
  alive = false
  epoch++
  for (const control of controls) control.abort()
  controls.clear()
  source.value = ''
  store = null
})
async function request(
  path: string,
  options: { method?: string; body?: unknown } = {},
  external?: AbortController,
) {
  const expected = ticket(),
    control = external ?? new AbortController()
  controls.add(control)
  try {
    const result = await session.request(path, { ...options, signal: control.signal })
    if (!current(expected)) throw new Error('当前账号或服务已变化')
    return result
  } finally {
    controls.delete(control)
  }
}
async function loadDefinitions() {
  const expected = ticket(),
    requestId = ++definitionSequence
  loading.value = true
  error.value = ''
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
      search: search.value,
    })
    const result = parsePage(await request(`/codegen/definitions?${query}`), parseDefinition)
    if (!current(expected) || requestId !== definitionSequence) return
    definitions.value = result.data
    total.value = result.meta.total
  } catch (value) {
    if (current(expected) && requestId === definitionSequence) error.value = errorText(value)
  } finally {
    if (current(expected) && requestId === definitionSequence) loading.value = false
  }
}
async function loadHistory() {
  const expected = ticket(),
    requestId = ++historySequence
  historyLoading.value = true
  try {
    const query = new URLSearchParams({
      page: String(historyPage.value),
      pageSize: String(historySize.value),
      search: historySearch.value,
    })
    const result = parsePage(await request(`/codegen/generations?${query}`), parseGeneration)
    if (!current(expected) || requestId !== historySequence) return
    history.value = result.data
    historyTotal.value = result.meta.total
  } catch (value) {
    if (current(expected) && requestId === historySequence) error.value = errorText(value)
  } finally {
    if (current(expected) && requestId === historySequence) historyLoading.value = false
  }
}
function queryDefinitions() {
  page.value = 1
  void loadDefinitions()
}
function queryHistory() {
  historyPage.value = 1
  void loadHistory()
}
function begin(kind: CodegenOperation['kind'], definition: CodeDefinition | null = null) {
  if (!store || recoveryError.value) throw new Error('本机恢复记录不可用，未发送请求')
  const row: CodegenOperation = {
    requestId: crypto.randomUUID(),
    kind,
    definitionId: definition?.id ?? null,
    definitionVersion: definition?.version ?? null,
    createdAt: new Date().toISOString(),
  }
  store.put(row)
  readPending()
  return row
}
function confirmed(id: string, expected: number) {
  if (!current(expected)) return
  try {
    store?.remove(id)
    readPending()
  } catch {
    recoveryError.value = '结果已确认，但本机标记未能移除，请恢复查询'
  }
}
function rejected(id: string, expected: number, value: unknown) {
  if (value instanceof ApiError && [400, 401, 403, 404, 413, 415, 422, 429].includes(value.status))
    confirmed(id, expected)
}
function newField(): FieldDraft {
  return {
    key: crypto.randomUUID(),
    name: '',
    label: '',
    kind: 'text',
    required: false,
    searchable: false,
    unique: false,
  }
}
function edit(row: CodeDefinition | null) {
  editing.value = row
  formError.value = ''
  form.resource = row?.schema.resource ?? ''
  form.label = row?.schema.label ?? ''
  form.fields = row
    ? row.schema.fields.map((field) => ({ ...field, key: crypto.randomUUID() }))
    : [{ ...newField(), name: 'name', label: '名称', required: true, searchable: true }]
  open.value = true
}
function typeChanged(field: FieldDraft) {
  if (field.kind !== 'text') {
    field.searchable = false
    field.unique = false
  }
  if (!field.required) field.unique = false
}
function moveField(index: number, direction: number) {
  const target = index + direction
  if (target < 0 || target >= form.fields.length) return
  const row = form.fields.splice(index, 1)[0]
  if (row) form.fields.splice(target, 0, row)
}
function schema() {
  try {
    return parseSchema({
      resource: form.resource.trim(),
      label: form.label.trim(),
      fields: form.fields.map(({ key, ...field }) => ({
        ...field,
        name: field.name.trim(),
        label: field.label.trim(),
      })),
    })
  } catch {
    throw new Error(
      '请检查资源标识、字段名称、类型和索引设置；最多 40 个字段、3 个搜索字段及 5 个唯一字段',
    )
  }
}
async function save() {
  const expected = ticket()
  let operation: string | null = null
  writing.value = true
  formError.value = ''
  try {
    const value = schema()
    if (editing.value) {
      parseDefinition(
        (
          await request(`/codegen/definitions/${editing.value.id}`, {
            method: 'PATCH',
            body: { version: editing.value.version, schema: value },
          })
        ).data,
      )
    } else {
      const marker = begin('definition.create')
      operation = marker.requestId
      const row = parseDefinition(
        (
          await request('/codegen/definitions', {
            method: 'POST',
            body: { requestId: marker.requestId, schema: value },
          })
        ).data,
      )
      if (row.requestId !== marker.requestId) throw new Error('创建结果与请求不匹配')
      confirmed(marker.requestId, expected)
    }
    if (current(expected)) {
      open.value = false
      ElMessage.success('定义已保存')
      await loadDefinitions()
    }
  } catch (value) {
    if (current(expected)) {
      if (operation) rejected(operation, expected, value)
      formError.value = errorText(value)
      readPending()
    }
  } finally {
    if (current(expected)) writing.value = false
  }
}
async function reloadDefinition() {
  if (!editing.value) return
  const expected = ticket()
  try {
    const row = parseDefinition((await request(`/codegen/definitions/${editing.value.id}`)).data)
    if (current(expected)) edit(row)
  } catch (value) {
    if (current(expected)) formError.value = errorText(value)
  }
}
async function remove(row: CodeDefinition) {
  const expected = ticket()
  try {
    await ElMessageBox.confirm(
      '删除定义后不能再以该定义生成；已保存的历史源码包继续保留。',
      '删除生成定义',
      { confirmButtonText: '删除', cancelButtonText: '取消', type: 'warning' },
    )
    if (!current(expected)) return
    writing.value = true
    await request(`/codegen/definitions/${row.id}?version=${row.version}`, { method: 'DELETE' })
    if (current(expected)) await loadDefinitions()
  } catch (value) {
    if (current(expected) && value !== 'cancel' && value !== 'close') error.value = errorText(value)
  } finally {
    if (current(expected)) writing.value = false
  }
}
async function generate(row: CodeDefinition) {
  const expected = ticket()
  let operation: string | null = null
  writing.value = true
  error.value = ''
  try {
    const marker = begin('generation.create', row)
    operation = marker.requestId
    const result = parseGeneration(
      (
        await request('/codegen/generations', {
          method: 'POST',
          body: {
            requestId: marker.requestId,
            definitionId: row.id,
            definitionVersion: row.version,
          },
        })
      ).data,
    )
    if (
      result.requestId !== marker.requestId ||
      result.definitionId !== row.id ||
      result.definitionVersion !== row.version
    )
      throw new Error('生成结果与请求不匹配')
    confirmed(marker.requestId, expected)
    if (current(expected)) {
      await loadHistory()
      await showPreview(result)
    }
  } catch (value) {
    if (current(expected)) {
      if (operation) rejected(operation, expected, value)
      error.value = errorText(value)
      readPending()
    }
  } finally {
    if (current(expected)) writing.value = false
  }
}
async function showPreview(row: CodeGeneration) {
  selected.value = row
  filePath.value =
    row.files.find((file) => file.path === 'README.md')?.path ?? row.files[0]?.path ?? ''
  source.value = ''
  sourceError.value = ''
  previewOpen.value = true
  await loadSource()
}
async function loadSource() {
  sourceController?.abort()
  const control = new AbortController()
  sourceController = control
  const expected = ticket(),
    sequence = ++sourceSequence,
    row = selected.value,
    file = row?.files.find((file) => file.path === filePath.value)
  source.value = ''
  sourceError.value = ''
  if (!row || !file) return
  sourceLoading.value = true
  try {
    const result = parseSource(
      (
        await request(
          `/codegen/generations/${row.id}/preview?${new URLSearchParams({ path: file.path })}`,
          {},
          control,
        )
      ).data,
      file,
    )
    await verifySource(result, file)
    if (current(expected) && sequence === sourceSequence && previewOpen.value)
      source.value = result.content
  } catch (value) {
    if (current(expected) && sequence === sourceSequence) sourceError.value = errorText(value)
  } finally {
    if (current(expected) && sequence === sourceSequence) sourceLoading.value = false
  }
}
function closePreview() {
  sourceSequence++
  sourceController?.abort()
  source.value = ''
  sourceError.value = ''
  selected.value = null
}
async function download(row: CodeGeneration) {
  const expected = ticket(),
    control = new AbortController()
  controls.add(control)
  downloading.value = row.id
  try {
    const spec = { size: row.size, sha256: row.sha256 }
    const blob = await session.downloadGeneratedProject(row.id, spec, createBrowserFileSink(spec), {
      signal: control.signal,
    })
    if (!current(expected)) return
    const url = URL.createObjectURL(new Blob([blob], { type: 'application/zip' }))
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = `codegen-${row.id}.zip`
    document.body.append(anchor)
    anchor.click()
    anchor.remove()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
    ElMessage.success('生成包完整性校验通过，已开始下载')
  } catch (value) {
    if (current(expected)) error.value = errorText(value)
  } finally {
    controls.delete(control)
    if (current(expected)) downloading.value = ''
  }
}
async function recover(row: CodegenOperation) {
  const expected = ticket()
  recovering.value = row.requestId
  try {
    if (row.kind === 'definition.create') {
      const value = parseDefinition(
        (await request(`/codegen/definitions/by-request/${row.requestId}`)).data,
      )
      if (value.requestId !== row.requestId) throw new Error('定义回执不匹配')
    } else {
      const value = parseGeneration(
        (await request(`/codegen/generations/by-request/${row.requestId}`)).data,
      )
      if (
        value.requestId !== row.requestId ||
        value.definitionId !== row.definitionId ||
        value.definitionVersion !== row.definitionVersion
      )
        throw new Error('生成回执不匹配')
    }
    confirmed(row.requestId, expected)
    if (current(expected)) {
      await Promise.all([loadDefinitions(), loadHistory()])
      ElMessage.success('已确认原请求结果，没有重新执行')
    }
  } catch (value) {
    if (current(expected)) error.value = errorText(value)
  } finally {
    if (current(expected)) recovering.value = ''
  }
}
async function forget(row: CodegenOperation) {
  const expected = ticket()
  try {
    await ElMessageBox.confirm(
      '仅移除本机请求标记，不会取消、删除或重新执行服务器操作。请先核对定义及历史结果。',
      '移除本机标记',
      { confirmButtonText: '移除标记', cancelButtonText: '取消', type: 'warning' },
    )
    confirmed(row.requestId, expected)
  } catch {}
}
async function clearLocal() {
  const expected = ticket()
  try {
    await ElMessageBox.confirm(
      '清除当前账号在此服务器的本机标记，不会取消或删除服务器结果。请先核对定义与历史记录。',
      '清除本机恢复记录',
      { confirmButtonText: '清除本机记录', cancelButtonText: '取消', type: 'warning' },
    )
    if (!current(expected)) return
    if (!store && session.user)
      store = new CodegenRecoveryStore(localStorage, session.settings.apiBaseUrl, session.user.id)
    if (!store) throw new Error('本机存储不可用')
    store.clear()
    readPending()
  } catch (value) {
    if (current(expected) && value !== 'cancel' && value !== 'close')
      recoveryError.value = errorText(value)
  }
}
onMounted(() => void Promise.all([loadDefinitions(), loadHistory()]))
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">DEVELOPMENT WORKSPACE</div>
      <h1>代码生成</h1>
      <p>配置资源字段，生成独立 Rust / MongoDB / Vue 工程；源码包保留精确版本与摘要。</p>
    </div>
    <el-button v-if="canWrite" type="primary" :disabled="blocked" @click="edit(null)"
      >新建定义</el-button
    >
  </div>
  <el-alert v-if="error" :title="error" type="error" :closable="false" />
  <section v-if="pending.length || recoveryError" class="surface recovery">
    <el-alert v-if="recoveryError" :title="recoveryError" type="error" :closable="false" />
    <h3 v-if="pending.length">待确认生成操作</h3>
    <p class="subtle">响应丢失时先查询原请求，恢复不会重新创建或生成。未查到结果时可稍后再查。</p>
    <div v-for="row in pending" :key="row.requestId" class="pending-row">
      <div>
        <strong>{{ row.kind === 'definition.create' ? '创建定义' : '生成工程' }}</strong
        ><code class="table-secondary">{{ row.requestId }}</code>
      </div>
      <el-button :loading="recovering === row.requestId" @click="recover(row)">恢复查询</el-button
      ><el-button text @click="forget(row)">移除标记</el-button>
    </div>
    <el-button v-if="recoveryError" text @click="readPending">重读本机记录</el-button
    ><el-button v-if="recoveryError" text type="danger" @click="clearLocal">清除本机记录</el-button>
  </section>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <h3>生成定义</h3>
      <el-input
        v-model="search"
        placeholder="按资源标识前缀搜索"
        clearable
        class="search-input"
        @keyup.enter="queryDefinitions"
      /><el-button @click="queryDefinitions">查询定义</el-button>
      <div class="toolbar-spacer" />
      <el-button text @click="loadDefinitions">刷新定义</el-button>
    </div>
    <el-table :data="definitions" row-key="id" v-loading="loading" class="codegen-definitions"
      ><el-table-column label="资源定义" min-width="230"
        ><template #default="{ row }"
          ><strong>{{ row.schema.label }}</strong
          ><code class="table-secondary">{{ row.schema.resource }}</code
          ><small class="table-secondary">{{ row.id }}</small></template
        ></el-table-column
      ><el-table-column label="字段 / 版本" width="130"
        ><template #default="{ row }"
          >{{ row.schema.fields.length }} 个字段<small class="table-secondary"
            >版本 {{ row.version }}</small
          ></template
        ></el-table-column
      ><el-table-column label="更新时间" min-width="175"
        ><template #default="{ row }">{{ dateText(row.updatedAt) }}</template></el-table-column
      ><el-table-column v-if="canWrite" label="操作" width="235" fixed="right"
        ><template #default="{ row }"
          ><el-button
            link
            type="primary"
            :disabled="blocked"
            @click="generate(row as CodeDefinition)"
            >生成工程</el-button
          ><el-button link :disabled="blocked" @click="edit(row as CodeDefinition)">编辑</el-button
          ><el-button link type="danger" :disabled="blocked" @click="remove(row as CodeDefinition)"
            >删除</el-button
          ></template
        ></el-table-column
      ></el-table
    >
    <div class="table-footer">
      <span class="subtle">共 {{ total }} 个定义</span
      ><el-pagination
        v-model:current-page="page"
        v-model:page-size="pageSize"
        :total="total"
        :page-sizes="[20, 50, 100]"
        layout="sizes,prev,pager,next"
        @current-change="loadDefinitions"
        @size-change="queryDefinitions"
      />
    </div>
  </section>
  <section class="surface table-surface history-section">
    <div class="table-toolbar">
      <h3>生成历史</h3>
      <el-input
        v-model="historySearch"
        placeholder="按历史资源标识前缀搜索"
        clearable
        class="search-input"
        @keyup.enter="queryHistory"
      /><el-button @click="queryHistory">查询历史</el-button>
      <div class="toolbar-spacer" />
      <el-button text @click="loadHistory">刷新历史</el-button>
    </div>
    <el-table :data="history" row-key="id" v-loading="historyLoading" class="codegen-history"
      ><el-table-column label="生成工程" min-width="240"
        ><template #default="{ row }"
          ><strong>{{ row.schema.label }}</strong
          ><code class="table-secondary">{{ row.schema.resource }}</code
          ><small class="table-secondary">{{ row.id }}</small></template
        ></el-table-column
      ><el-table-column label="来源版本" width="150"
        ><template #default="{ row }"
          >定义 {{ row.definitionVersion
          }}<small class="table-secondary">模板 {{ row.templateVersion }}</small></template
        ></el-table-column
      ><el-table-column label="文件 / 长度" width="150"
        ><template #default="{ row }"
          >{{ row.files.length }} 个文件<small class="table-secondary"
            >{{ row.size.toLocaleString() }} 字节</small
          ></template
        ></el-table-column
      ><el-table-column label="生成时间" min-width="175"
        ><template #default="{ row }">{{ dateText(row.createdAt) }}</template></el-table-column
      ><el-table-column label="操作" width="175" fixed="right"
        ><template #default="{ row }"
          ><el-button link @click="showPreview(row as CodeGeneration)">预览</el-button
          ><el-button
            link
            type="primary"
            :loading="downloading === row.id"
            @click="download(row as CodeGeneration)"
            >下载 ZIP</el-button
          ></template
        ></el-table-column
      ></el-table
    >
    <div class="table-footer">
      <span class="subtle">共 {{ historyTotal }} 次生成 · 原始源码包不随定义编辑改变</span
      ><el-pagination
        v-model:current-page="historyPage"
        v-model:page-size="historySize"
        :total="historyTotal"
        :page-sizes="[20, 50, 100]"
        layout="sizes,prev,pager,next"
        @current-change="loadHistory"
        @size-change="queryHistory"
      />
    </div>
  </section>
  <el-dialog
    v-model="open"
    :title="editing ? '编辑生成定义' : '新建生成定义'"
    width="min(1100px,96vw)"
    :close-on-click-modal="false"
    :show-close="!writing"
    :close-on-press-escape="!writing"
    destroy-on-close
    ><el-alert v-if="formError" :title="formError" type="error" :closable="false" /><el-form
      label-position="top"
      :disabled="writing"
      @submit.prevent="save"
      ><div class="form-grid">
        <el-form-item label="资源标识"
          ><el-input
            v-model="form.resource"
            aria-label="资源标识"
            maxlength="48"
            placeholder="如 inventory_items" /></el-form-item
        ><el-form-item label="显示名称"
          ><el-input v-model="form.label" aria-label="显示名称" maxlength="80"
        /></el-form-item>
      </div>
      <p class="subtle">
        字段按普通业务数据保存，不生成密码或密钥字段。唯一字段在产物内按所有者区分；标识使用小写字母开头的英文驼峰。
      </p>
      <div v-for="(field, index) in form.fields" :key="field.key" class="codegen-field">
        <el-input
          v-model="field.name"
          :aria-label="`字段标识 ${index + 1}`"
          placeholder="字段标识"
          maxlength="48"
        /><el-input
          v-model="field.label"
          :aria-label="`字段名称 ${index + 1}`"
          placeholder="字段名称"
          maxlength="80"
        /><el-select
          v-model="field.kind"
          :aria-label="`字段类型 ${index + 1}`"
          @change="typeChanged(field)"
          ><el-option
            v-for="kind in fieldKinds"
            :key="kind"
            :value="kind"
            :label="labels[kind]" /></el-select
        ><el-checkbox v-model="field.required" @change="typeChanged(field)">必填</el-checkbox
        ><el-checkbox v-model="field.searchable" :disabled="field.kind !== 'text'">搜索</el-checkbox
        ><el-checkbox v-model="field.unique" :disabled="field.kind !== 'text' || !field.required"
          >唯一</el-checkbox
        ><el-button text :disabled="index === 0" @click="moveField(index, -1)">上移</el-button
        ><el-button text :disabled="index === form.fields.length - 1" @click="moveField(index, 1)"
          >下移</el-button
        ><el-button
          text
          type="danger"
          :disabled="form.fields.length === 1"
          @click="form.fields.splice(index, 1)"
          >移除</el-button
        >
      </div>
      <el-button
        :disabled="form.fields.length >= 40 || writing"
        @click="form.fields.push(newField())"
        >添加字段</el-button
      >
      <p v-if="editing" class="subtle">
        按版本 {{ editing.version }} 保存。版本冲突时重新读取后再修改。
      </p></el-form
    ><template #footer
      ><el-button v-if="editing" :disabled="writing" @click="reloadDefinition">重新读取</el-button
      ><el-button :disabled="writing" @click="open = false">取消</el-button
      ><el-button type="primary" :loading="writing" :disabled="blocked" @click="save"
        >保存定义</el-button
      ></template
    ></el-dialog
  >
  <el-dialog
    v-model="previewOpen"
    title="生成工程预览"
    width="min(1100px,96vw)"
    @closed="closePreview"
    ><template v-if="selected"
      ><p>
        <strong>{{ selected.schema.label }}</strong> · 定义 {{ selected.definitionVersion }} · 模板
        {{ selected.templateVersion }}
      </p>
      <code class="table-secondary digest">SHA-256 {{ selected.sha256 }}</code
      ><el-select
        v-model="filePath"
        aria-label="预览文件"
        filterable
        style="width: 100%"
        @change="loadSource"
        ><el-option
          v-for="file in selected.files"
          :key="file.path"
          :value="file.path"
          :label="file.path" /></el-select
      ><el-alert v-if="sourceError" :title="sourceError" type="error" :closable="false" />
      <div v-loading="sourceLoading" class="source-box">
        <el-input
          :model-value="source"
          type="textarea"
          :rows="18"
          readonly
          aria-label="源码预览"
        /></div></template
    ><template #footer
      ><el-button
        v-if="selected"
        type="primary"
        :loading="downloading === selected.id"
        @click="download(selected)"
        >下载 ZIP</el-button
      ></template
    ></el-dialog
  >
</template>
<style scoped>
.history-section {
  margin-top: 24px;
}
.recovery {
  padding: 20px;
  margin-bottom: 20px;
}
.pending-row {
  display: flex;
  gap: 12px;
  align-items: center;
  padding: 12px 0;
}
.pending-row > div {
  flex: 1;
  min-width: 0;
}
.codegen-field {
  display: grid;
  grid-template-columns: minmax(100px, 1fr) minmax(110px, 1fr) 120px repeat(3, 58px) repeat(
      3,
      42px
    );
  gap: 8px;
  align-items: center;
  margin: 14px 0;
}
.codegen-field .el-checkbox {
  margin-right: 0;
}
.source-box {
  margin-top: 18px;
}
.source-box :deep(textarea) {
  font-family: Consolas, monospace;
  font-size: 12px;
  line-height: 1.6;
}
.digest {
  overflow-wrap: anywhere;
  margin: 10px 0 20px;
}
@media (max-width: 900px) {
  .codegen-field {
    grid-template-columns: 1fr 1fr 110px;
    gap: 10px;
  }
  .pending-row {
    flex-wrap: wrap;
  }
}
</style>
