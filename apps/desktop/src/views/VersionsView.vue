<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Plus, Refresh, Upload } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import { ApiError } from '../core/client'
import {
  MAX_CONFIGURATION_BYTES,
  decodeConfigurationFile,
  parseConfigurationPackage,
} from '../core/configuration'
import type {
  ApiCatalog,
  ApiEndpoint,
  DictionaryRecord,
  ImportCounts,
  ImportPreview,
  ImportResult,
  MenuRecord,
  ParameterRecord,
  VersionRecord,
} from '../core/types'
import { dateText, errorText, notifyError } from '../ui'

const session = useSession()
const rows = ref<VersionRecord[]>([]),
  loading = ref(false),
  loadError = ref('')
const page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  sort = ref('-createdAt')
const detail = ref<VersionRecord | null>(null),
  detailOpen = ref(false),
  downloading = ref('')
const captureOpen = ref(false),
  capturing = ref(false),
  captureError = ref(''),
  choicesLoading = ref(false)
const form = reactive({
  name: '',
  code: '',
  description: '',
  menuIds: [] as string[],
  dictionaryIds: [] as string[],
  parameterIds: [] as string[],
  apiOperationIds: [] as string[],
})
const menuChoices = ref<MenuRecord[]>([]),
  apiChoices = ref<ApiEndpoint[]>([])
type Choice = { id: string; name: string; key: string }
const dictionaryChoices = ref<Choice[]>([]),
  parameterChoices = ref<Choice[]>([])
const importOpen = ref(false),
  rawPackage = ref(''),
  strategy = ref<'createOnly' | 'merge'>('createOnly')
const importPreview = ref<ImportPreview | null>(null),
  importResult = ref<ImportResult | null>(null),
  importError = ref('')
const previewing = ref(false),
  applying = ref(false),
  fileReading = ref(false),
  clock = ref(Date.now())
const importBusy = computed(() => previewing.value || applying.value || fileReading.value)
const expired = computed(
  () => !!importPreview.value && new Date(importPreview.value.expiresAt).getTime() <= clock.value,
)
const canApply = computed(
  () =>
    !!importPreview.value?.canApply && !expired.value && !importResult.value && !importBusy.value,
)
const resources: Array<{ key: keyof ImportCounts; label: string }> = [
  { key: 'menus', label: '菜单' },
  { key: 'dictionaries', label: '字典' },
  { key: 'dictionaryItems', label: '字典明细' },
  { key: 'parameters', label: '参数' },
]
const actions: Record<string, string> = {
  create: '新增',
  update: '更新',
  unchanged: '保持',
  conflict: '冲突',
  reuse: '复用',
}
const summary = computed(() =>
  importPreview.value
    ? resources.map((resource) => ({
        name: resource.label,
        ...importPreview.value!.counts[resource.key],
      }))
    : [],
)
let loadSequence = 0,
  dictionarySequence = 0,
  parameterSequence = 0
const ticker = setInterval(() => {
  clock.value = Date.now()
}, 1000)
onUnmounted(() => {
  clearInterval(ticker)
  loadSequence++
})
watch([rawPackage, strategy], () => {
  importPreview.value = null
  importResult.value = null
})

async function load() {
  const sequence = ++loadSequence
  loading.value = true
  loadError.value = ''
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
      search: search.value,
      sort: sort.value,
    })
    const response = await session.request<VersionRecord[]>(`/versions?${query}`)
    if (sequence !== loadSequence) return
    rows.value = response.data
    total.value = response.meta?.total ?? 0
  } catch (error) {
    if (sequence === loadSequence) loadError.value = errorText(error)
  } finally {
    if (sequence === loadSequence) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
function showDetail(row: VersionRecord) {
  detail.value = row
  detailOpen.value = true
}
async function loadChoices(kind: 'dictionaries' | 'parameters', search = '') {
  const sequence = kind === 'dictionaries' ? ++dictionarySequence : ++parameterSequence
  const response = await session.request<Array<DictionaryRecord | ParameterRecord>>(
    `/${kind}?${new URLSearchParams({ pageSize: '100', sort: 'key', search })}`,
  )
  if (sequence !== (kind === 'dictionaries' ? dictionarySequence : parameterSequence)) return
  const choices = kind === 'dictionaries' ? dictionaryChoices : parameterChoices
  const selected = kind === 'dictionaries' ? form.dictionaryIds : form.parameterIds
  choices.value = [
    ...new Map(
      [...choices.value.filter((row) => selected.includes(row.id)), ...response.data].map((row) => [
        row.id,
        { id: row.id, name: row.name, key: row.key },
      ]),
    ).values(),
  ]
}
function searchChoices(kind: 'dictionaries' | 'parameters', value: string) {
  void loadChoices(kind, value).catch((error) => {
    captureError.value = errorText(error)
  })
}
function searchDictionaries(value: string) {
  searchChoices('dictionaries', value)
}
function searchParameters(value: string) {
  searchChoices('parameters', value)
}
async function openCapture() {
  Object.assign(form, {
    name: '',
    code: '',
    description: '',
    menuIds: [],
    dictionaryIds: [],
    parameterIds: [],
    apiOperationIds: [],
  })
  captureError.value = ''
  captureOpen.value = true
  choicesLoading.value = true
  const tasks: Promise<unknown>[] = []
  if (session.can('menus.read'))
    tasks.push(
      session.request<MenuRecord[]>('/menus').then((response) => {
        menuChoices.value = response.data
      }),
    )
  if (session.can('dictionaries.read')) tasks.push(loadChoices('dictionaries'))
  if (session.can('parameters.read')) tasks.push(loadChoices('parameters'))
  if (session.can('permissions.read'))
    tasks.push(
      session.request<ApiCatalog>('/api-catalog').then((response) => {
        apiChoices.value = response.data.endpoints
      }),
    )
  const results = await Promise.allSettled(tasks)
  const failed = results.find((result) => result.status === 'rejected')
  if (failed?.status === 'rejected') captureError.value = errorText(failed.reason)
  choicesLoading.value = false
}
async function capture() {
  captureError.value = ''
  if (!form.name.trim() || !form.code.trim()) {
    captureError.value = '请填写版本名称和编号'
    return
  }
  if (
    !form.menuIds.length &&
    !form.dictionaryIds.length &&
    !form.parameterIds.length &&
    !form.apiOperationIds.length
  ) {
    captureError.value = '请至少选择一项配置或接口依赖'
    return
  }
  capturing.value = true
  try {
    await session.request('/versions', { method: 'POST', body: form })
    captureOpen.value = false
    ElMessage.success('配置快照已保存')
    await load()
  } catch (error) {
    captureError.value = errorText(error)
  } finally {
    capturing.value = false
  }
}
async function download(row: VersionRecord) {
  downloading.value = row.id
  try {
    const file = await session.downloadConfiguration(`/versions/${row.id}/download`)
    const url = URL.createObjectURL(
      new Blob([new Uint8Array(file.bytes)], { type: 'application/json' }),
    )
    const link = document.createElement('a')
    link.href = url
    link.download = file.filename
    document.body.append(link)
    link.click()
    link.remove()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
    ElMessage.success('校验通过，已开始下载')
  } catch (error) {
    notifyError(error)
  } finally {
    downloading.value = ''
  }
}
async function remove(row: VersionRecord) {
  try {
    await ElMessageBox.confirm(`确认删除版本“${row.name}”？历史编号仍不能复用。`, '删除版本', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    })
    await session.request(`/versions/${row.id}?version=${row.version}`, { method: 'DELETE' })
    if (rows.value.length === 1 && page.value > 1) page.value--
    await load()
  } catch (error) {
    if (error !== 'cancel' && error !== 'close') notifyError(error)
  }
}
function openImport() {
  rawPackage.value = ''
  strategy.value = 'createOnly'
  importPreview.value = null
  importResult.value = null
  importError.value = ''
  importOpen.value = true
}
async function readFile(event: Event) {
  const input = event.target as HTMLInputElement,
    file = input.files?.[0]
  if (!file) return
  fileReading.value = true
  importError.value = ''
  rawPackage.value = ''
  importPreview.value = null
  importResult.value = null
  try {
    if (file.size > MAX_CONFIGURATION_BYTES) throw new Error('配置包最多 8 MiB')
    rawPackage.value = decodeConfigurationFile(await file.arrayBuffer())
  } catch (error) {
    importError.value = errorText(error)
  } finally {
    fileReading.value = false
    input.value = ''
  }
}
async function previewImport() {
  importError.value = ''
  importResult.value = null
  importPreview.value = null
  let bundle: unknown
  try {
    bundle = parseConfigurationPackage(rawPackage.value)
  } catch (error) {
    importError.value = errorText(error)
    return
  }
  previewing.value = true
  try {
    importPreview.value = (
      await session.request<ImportPreview>('/versions/import-previews', {
        method: 'POST',
        body: { bundle, strategy: strategy.value },
      })
    ).data
    clock.value = Date.now()
  } catch (error) {
    importError.value = errorText(error)
  } finally {
    previewing.value = false
  }
}
async function applyImport() {
  if (!canApply.value || !importPreview.value) return
  applying.value = true
  importError.value = ''
  try {
    importResult.value = (
      await session.request<ImportResult>(
        `/versions/import-previews/${importPreview.value.id}/apply`,
        { method: 'POST', body: { checksum: importPreview.value.checksum } },
      )
    ).data
    await load()
    try {
      await session.loadNavigation()
    } catch {
      ElMessage.warning('配置已应用，菜单刷新失败，请稍后刷新')
    }
  } catch (error) {
    importError.value = errorText(error)
    if (error instanceof ApiError && [403, 404, 409].includes(error.status))
      importPreview.value = null
  } finally {
    applying.value = false
  }
}
onMounted(() => void load())
</script>

<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">CONFIGURATION RELEASES</div>
      <h1>版本管理</h1>
      <p>捕获配置快照，在应用前审阅差异，保留每次发版的可验证记录。</p>
    </div>
    <div v-if="session.can('versions.write')" class="version-actions">
      <el-button :icon="Upload" @click="openImport">导入配置包</el-button
      ><el-button type="primary" :icon="Plus" @click="openCapture">捕获配置</el-button>
    </div>
  </div>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <el-input
        v-model="search"
        class="search-input"
        placeholder="按版本编号前缀搜索"
        clearable
        @keyup.enter="query"
      /><el-button @click="query">查询</el-button
      ><el-select v-model="sort" class="sort-select" aria-label="版本排序" @change="query"
        ><el-option label="最近创建" value="-createdAt" /><el-option
          label="编号升序"
          value="code" /><el-option label="名称升序" value="name"
      /></el-select>
      <div class="toolbar-spacer" />
      <el-button :icon="Refresh" text @click="load">刷新</el-button>
    </div>
    <el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-table v-loading="loading" :data="rows" row-key="id">
      <el-table-column label="配置版本" min-width="220"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><small class="table-secondary">{{ row.code }}</small></template
        ></el-table-column
      >
      <el-table-column label="配置数量" min-width="250"
        ><template #default="{ row }"
          >菜单 {{ row.counts.menus }} · 字典 {{ row.counts.dictionaries }} · 参数
          {{ row.counts.parameters }} · 接口 {{ row.counts.apis }}</template
        ></el-table-column
      >
      <el-table-column label="来源" width="90"
        ><template #default="{ row }"
          ><el-tag type="info">{{ row.origin === 'capture' ? '捕获' : '导入' }}</el-tag></template
        ></el-table-column
      >
      <el-table-column label="创建时间" min-width="170"
        ><template #default="{ row }">{{ dateText(row.createdAt) }}</template></el-table-column
      >
      <el-table-column label="操作" width="180" fixed="right"
        ><template #default="{ row }"
          ><el-button link type="primary" @click="showDetail(row as VersionRecord)">详情</el-button
          ><el-button
            link
            type="primary"
            :loading="downloading === row.id"
            :disabled="!!downloading"
            @click="download(row as VersionRecord)"
            >下载</el-button
          ><el-button
            v-if="session.can('versions.write')"
            link
            type="danger"
            @click="remove(row as VersionRecord)"
            >删除</el-button
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

  <el-dialog
    v-model="captureOpen"
    title="捕获配置"
    width="680px"
    :close-on-click-modal="false"
    :show-close="!capturing"
    :close-on-press-escape="!capturing"
  >
    <el-form label-position="top" :disabled="capturing" class="editor-form">
      <div class="form-two">
        <el-form-item label="版本名称"><el-input v-model="form.name" maxlength="80" /></el-form-item
        ><el-form-item label="版本编号"
          ><el-input v-model="form.code" maxlength="64" placeholder="如 release-2026-01"
        /></el-form-item>
      </div>
      <el-form-item label="描述"
        ><el-input v-model="form.description" type="textarea" maxlength="1000" :rows="2"
      /></el-form-item>
      <p class="subtle">只展示你有权读取的配置。菜单会包含必要父级，字典会包含全部活动明细。</p>
      <el-form-item v-if="session.can('menus.read')" label="菜单"
        ><el-select
          v-model="form.menuIds"
          multiple
          filterable
          collapse-tags
          collapse-tags-tooltip
          :multiple-limit="1000"
          :loading="choicesLoading"
          placeholder="选择菜单"
          ><el-option
            v-for="row in menuChoices"
            :key="row.id"
            :value="row.id"
            :label="`${row.label} · ${row.path}`" /></el-select
      ></el-form-item>
      <el-form-item v-if="session.can('dictionaries.read')" label="字典"
        ><el-select
          v-model="form.dictionaryIds"
          multiple
          filterable
          remote
          :remote-method="searchDictionaries"
          :multiple-limit="100"
          :loading="choicesLoading"
          placeholder="按字典标识搜索"
          ><el-option
            v-for="row in dictionaryChoices"
            :key="row.id"
            :value="row.id"
            :label="`${row.name} · ${row.key}`" /></el-select
      ></el-form-item>
      <el-form-item v-if="session.can('parameters.read')" label="参数"
        ><el-select
          v-model="form.parameterIds"
          multiple
          filterable
          remote
          :remote-method="searchParameters"
          :multiple-limit="100"
          :loading="choicesLoading"
          placeholder="按参数标识搜索"
          ><el-option
            v-for="row in parameterChoices"
            :key="row.id"
            :value="row.id"
            :label="`${row.name} · ${row.key}`" /></el-select
      ></el-form-item>
      <el-form-item v-if="session.can('permissions.read')" label="接口依赖"
        ><el-select
          v-model="form.apiOperationIds"
          multiple
          filterable
          collapse-tags
          collapse-tags-tooltip
          :multiple-limit="1000"
          :loading="choicesLoading"
          placeholder="选择目标服务需要具备的接口"
          ><el-option
            v-for="row in apiChoices"
            :key="row.operationId"
            :value="row.operationId"
            :label="`${row.label} · ${row.operationId}`" /></el-select
      ></el-form-item>
      <el-alert v-if="captureError" :title="captureError" type="error" :closable="false" />
    </el-form>
    <template #footer
      ><el-button :disabled="capturing" @click="captureOpen = false">取消</el-button
      ><el-button type="primary" :loading="capturing" :disabled="choicesLoading" @click="capture"
        >保存快照</el-button
      ></template
    >
  </el-dialog>

  <el-dialog
    v-model="importOpen"
    title="导入配置包"
    width="900px"
    :close-on-click-modal="false"
    :show-close="!importBusy"
    :close-on-press-escape="!importBusy"
  >
    <el-form label-position="top" :disabled="importBusy || !!importResult" class="editor-form">
      <el-form-item label="选择配置包" for="configuration-package-file"
        ><input
          type="file"
          id="configuration-package-file"
          accept=".json,application/json"
          :disabled="importBusy || !!importResult"
          @change="readFile"
        /><small class="subtle">新格式 JSON，最多 8 MiB，也可直接粘贴。</small></el-form-item
      >
      <el-form-item label="配置包 JSON"
        ><el-input v-model="rawPackage" type="textarea" :rows="6" placeholder="粘贴配置包内容"
      /></el-form-item>
      <el-form-item label="应用方式"
        ><el-radio-group v-model="strategy"
          ><el-radio value="createOnly">仅新增</el-radio
          ><el-radio value="merge">合并更新</el-radio></el-radio-group
        ><small class="subtle"
          >合并会更新同键配置，保留包外条目。已有历史版本编号不能被其他内容覆盖。</small
        ></el-form-item
      >
    </el-form>
    <el-alert v-if="importError" :title="importError" type="error" :closable="false" />
    <section v-if="importPreview" class="import-summary">
      <div class="preview-heading">
        <strong>导入预览</strong
        ><el-tag :type="importPreview.canApply && !expired ? 'success' : 'warning'">{{
          expired ? '预览已过期' : importPreview.canApply ? '可以应用' : '需要处理冲突或权限'
        }}</el-tag>
      </div>
      <p class="subtle">
        版本记录：{{ actions[importPreview.versionAction] }} · 有效至
        {{ dateText(importPreview.expiresAt) }}
      </p>
      <el-table :data="summary" size="small"
        ><el-table-column prop="name" label="资源" /><el-table-column
          prop="create"
          label="新增" /><el-table-column prop="update" label="更新" /><el-table-column
          prop="unchanged"
          label="保持" /><el-table-column prop="conflict" label="冲突"
      /></el-table>
      <el-alert
        v-if="importPreview.missingWritePermissions.length"
        :title="`缺少写入权限：${importPreview.missingWritePermissions.join('、')}`"
        type="warning"
        :closable="false"
      />
      <el-collapse
        ><el-collapse-item title="查看变更详情" name="changes"
          ><el-table :data="importPreview.changes" size="small" max-height="260"
            ><el-table-column label="资源" width="90"
              ><template #default="{ row }">{{
                resources.find((item) => item.key === row.resource)?.label
              }}</template></el-table-column
            ><el-table-column prop="key" label="标识" min-width="170" /><el-table-column
              label="操作"
              width="70"
              ><template #default="{ row }">{{ actions[row.action] }}</template></el-table-column
            ><el-table-column label="变更字段" min-width="180"
              ><template #default="{ row }">{{
                row.fields.join('、') || '—'
              }}</template></el-table-column
            ></el-table
          ><small v-if="importPreview.detailsTruncated" class="subtle"
            >仅显示前 100 项详情，汇总数量包含全部配置。</small
          ></el-collapse-item
        ></el-collapse
      >
    </section>
    <el-alert
      v-if="importResult"
      title="配置已应用"
      :description="`应用时间：${dateText(importResult.appliedAt)}。版本记录${importResult.versionReused ? '已复用' : '已保存'}。`"
      type="success"
      :closable="false"
    />
    <template #footer
      ><el-button :disabled="importBusy" @click="importOpen = false">关闭</el-button
      ><el-button
        :loading="previewing"
        :disabled="importBusy || !!importResult"
        @click="previewImport"
        >生成预览</el-button
      ><el-button type="primary" :loading="applying" :disabled="!canApply" @click="applyImport"
        >确认应用</el-button
      ></template
    >
  </el-dialog>

  <el-drawer v-model="detailOpen" title="版本详情" size="520px"
    ><template v-if="detail"
      ><h2>{{ detail.name }}</h2>
      <p>{{ detail.code }}</p>
      <p class="subtle">{{ detail.description || '暂无描述' }}</p>
      <el-descriptions :column="1" border
        ><el-descriptions-item label="创建者">{{ detail.createdBy.name }}</el-descriptions-item
        ><el-descriptions-item label="创建时间">{{
          dateText(detail.createdAt)
        }}</el-descriptions-item
        ><el-descriptions-item label="包体大小"
          >{{ (detail.byteLength / 1024).toFixed(1) }} KiB</el-descriptions-item
        ><el-descriptions-item label="必要父菜单">{{
          detail.counts.ancestors
        }}</el-descriptions-item
        ><el-descriptions-item label="字典明细">{{
          detail.counts.dictionaryItems
        }}</el-descriptions-item
        ><el-descriptions-item label="SHA-256"
          ><code class="checksum">{{ detail.checksum }}</code></el-descriptions-item
        ><el-descriptions-item label="下载所需权限">{{
          detail.readPermissions.join('、') || '无额外资源权限'
        }}</el-descriptions-item></el-descriptions
      ></template
    ></el-drawer
  >
</template>

<style scoped>
.version-actions {
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
}
.search-input {
  width: 260px;
}
.sort-select {
  width: 145px;
}
.editor-form :deep(.el-select) {
  width: 100%;
}
.editor-form small {
  display: block;
  width: 100%;
  margin-top: 6px;
}
.import-summary {
  margin-top: 20px;
  border-top: 1px solid var(--el-border-color);
  padding-top: 18px;
}
.preview-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.import-summary :deep(.el-collapse) {
  margin-top: 12px;
}
.checksum {
  overflow-wrap: anywhere;
  font-size: 12px;
}
</style>
