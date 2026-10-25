<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { useSession } from '../stores/session'
import { platform } from '../platform'
import { browserUploadSource } from '../core/file-source'
import { createBrowserFileSink, type FileSink, type UploadSource } from '../core/files'
import type { NativeSaveResult } from '../core/file-native'
import {
  DATA_FILE_LIMIT,
  dataSourceBase64,
  parseDataDownload,
  parseImportView,
  saveDataDownload,
  type DataResource,
  type DataFormat,
  type ImportView,
} from '../core/data-tools'
import { errorText } from '../ui'
const session = useSession(),
  resources = ref<DataResource[]>([]),
  resource = ref('parameters'),
  format = ref<DataFormat>('csv'),
  fields = ref<string[]>([]),
  search = ref('')
const selected = computed(() => resources.value.find((r) => r.key === resource.value)),
  busy = ref(false),
  error = ref(''),
  fileInput = ref<HTMLInputElement | null>(null),
  fileName = ref(''),
  content = ref(''),
  mapping = ref('{}'),
  preview = ref<ImportView | null>(null),
  clock = ref(Date.now())
const history = ref<Array<{ id: string; requestId: string; resource: string; expiresAt: string }>>(
  [],
)
const canImport = computed(
  () =>
    selected.value?.importable &&
    session.can('exports.write') &&
    !!selected.value.writePermission &&
    session.can(selected.value.writePermission),
)
const previewResource = computed(() =>
  resources.value.find((r) => r.key === preview.value?.resource),
)
const canApply = computed(
  () =>
    previewResource.value?.importable &&
    session.can('exports.write') &&
    !!previewResource.value.writePermission &&
    session.can(previewResource.value.writePermission),
)
const ready = computed(
  () => preview.value?.rows.filter((row) => row.status === 'ready').length ?? 0,
)
const expired = computed(
  () => !!preview.value && Date.parse(preview.value.expiresAt) <= clock.value,
)
const page = ref(1),
  pageRows = computed(() => preview.value?.rows.slice((page.value - 1) * 20, page.value * 20) ?? [])
const labels: Record<string, string> = {
  name: '名称',
  key: '标识',
  group: '分组',
  description: '描述',
  valueType: '值类型',
  value: '值',
  id: 'ID',
  username: '账号',
  displayName: '显示名',
  status: '状态',
  version: '版本',
  dictionaryKey: '父字典标识',
  label: '明细名称',
  sort: '排序',
  enabled: '启用',
}
const statuses: Record<string, string> = {
  ready: '可导入',
  applied: '已创建',
  invalid: '无效',
  duplicate: '重复',
  conflict: '冲突',
}
const problems: Record<string, string> = {
  INVALID_FIELDS: '字段无效',
  DUPLICATE_IN_FILE: '文件内标识重复',
  KEY_EXISTS: '标识已存在',
  PARENT_NOT_FOUND: '父字典不存在',
  PARENT_CHANGED: '父字典已改变，请重新预览',
}
let alive = true,
  sequence = 0,
  historySequence = 0,
  requestId = crypto.randomUUID()
const controls = new Set<AbortController>(),
  ticker = setInterval(() => (clock.value = Date.now()), 1000)
watch(resource, () => {
  fields.value = [...(selected.value?.fields ?? [])]
  preview.value = null
  content.value = ''
  fileName.value = ''
  mapping.value = '{}'
  requestId = crypto.randomUUID()
})
watch([mapping, format], () => {
  preview.value = null
  requestId = crypto.randomUUID()
})
async function run(work: (signal: AbortSignal) => Promise<void>) {
  if (busy.value) return
  const control = new AbortController()
  controls.add(control)
  busy.value = true
  error.value = ''
  try {
    await work(control.signal)
  } catch (failure) {
    if (alive && !control.signal.aborted) error.value = errorText(failure)
  } finally {
    controls.delete(control)
    if (alive) busy.value = false
  }
}
async function refreshHistory(signal?: AbortSignal) {
  const token = ++historySequence,
    result = await session.request<typeof history.value>('/data-tools/imports', { signal })
  if (alive && token === historySequence) history.value = result.data
}
async function acceptSource(source: UploadSource | null, signal: AbortSignal) {
  if (!source) return
  const name = source.name
  const nextFormat = /\.xlsx$/i.test(name) ? 'xlsx' : /\.csv$/i.test(name) ? 'csv' : null
  if (!nextFormat) {
    await source.close()
    throw new Error('请选择 CSV 或 XLSX 文件')
  }
  const bytes = await dataSourceBase64(source, signal)
  if (!alive || signal.aborted) return
  content.value = bytes
  fileName.value = name
  format.value = nextFormat
  preview.value = null
  requestId = crypto.randomUUID()
}
function choose() {
  if (platform.files)
    void run(async (signal) => acceptSource(await platform.files!.openSource({ signal }), signal))
  else fileInput.value?.click()
}
async function browserSelected(event: Event) {
  const input = event.target as HTMLInputElement,
    file = input.files?.[0]
  input.value = ''
  if (!file) return
  await run(async (signal) => {
    if (file.size > DATA_FILE_LIMIT) throw new Error('数据文件最多 6 MiB')
    await acceptSource(await browserUploadSource(file, signal), signal)
  })
}
async function makePreview() {
  await run(async (signal) => {
    let columns: unknown
    try {
      columns = JSON.parse(mapping.value)
    } catch {
      throw new Error('字段映射须为 JSON 对象，例如 {"name":"名称"}')
    }
    if (
      !columns ||
      typeof columns !== 'object' ||
      Array.isArray(columns) ||
      Object.values(columns).some((v) => typeof v !== 'string')
    )
      throw new Error('字段映射须使用字段名与表头字符串')
    const token = ++sequence,
      result = await session.request<unknown>('/data-tools/imports', {
        method: 'POST',
        signal,
        body: {
          requestId,
          resource: resource.value,
          format: format.value,
          contentBase64: content.value,
          mapping: columns,
        },
      })
    if (alive && token === sequence) {
      preview.value = parseImportView(result.data)
      page.value = 1
    }
    await refreshHistory(signal)
  })
}
async function loadImport(id: string) {
  await run(async (signal) => {
    const token = ++sequence,
      result = await session.request<unknown>(`/data-tools/imports/${id}`, { signal })
    if (alive && token === sequence) {
      preview.value = parseImportView(result.data)
      page.value = 1
    }
  })
}
async function apply() {
  await run(async (signal) => {
    const id = preview.value?.id
    if (!id) return
    ++sequence
    while (!signal.aborted && alive && preview.value?.id === id) {
      const rows = preview.value.rows
        .filter((row) => row.status === 'ready')
        .slice(0, 100)
        .map((row) => row.index)
      if (!rows.length) break
      const result = await session.request<unknown>(`/data-tools/imports/${id}/apply`, {
        method: 'POST',
        body: { rows },
        signal,
      })
      if (!signal.aborted && alive) preview.value = parseImportView(result.data)
    }
    if (alive && !signal.aborted) ElMessage.success('批次已处理，可查询每行结果')
  })
}
function cancel() {
  for (const control of controls) control.abort()
  error.value = '操作已停止；如应用请求已发出，请查询最新结果确认已提交的行。'
}
async function download(template: boolean) {
  await run(async (signal) => {
    const result = await session.request<unknown>(
      template ? '/data-tools/template' : '/data-tools/export',
      {
        method: 'POST',
        signal,
        body: {
          resource: resource.value,
          format: format.value,
          fields: fields.value,
          search: search.value,
        },
      },
    )
    const file = parseDataDownload(result.data)
    const sink: FileSink<Blob | NativeSaveResult> | null = platform.files
      ? await platform.files.chooseTarget(file.name, file, { signal })
      : createBrowserFileSink(file)
    if (!sink) return
    const saved = await saveDataDownload(file, sink, signal)
    if (!alive || signal.aborted) return
    if (saved instanceof Blob) {
      const url = URL.createObjectURL(new Blob([saved], { type: file.mimeType })),
        link = document.createElement('a')
      link.href = url
      link.download = file.name
      document.body.append(link)
      link.click()
      link.remove()
      setTimeout(() => URL.revokeObjectURL(url), 1000)
    } else if (saved.cleanupPending) ElMessage.warning('文件已保存，临时清理将自动重试')
    else ElMessage.success('文件已保存')
  })
}
onMounted(
  () =>
    void run(async (signal) => {
      const result = await session.request<DataResource[]>('/data-tools/resources', { signal })
      if (!alive) return
      resources.value = result.data
      if (!resources.value.some((r) => r.key === resource.value))
        resource.value = resources.value[0]?.key ?? ''
      fields.value = [...(selected.value?.fields ?? [])]
      await refreshHistory(signal)
    }),
)
onBeforeUnmount(() => {
  alive = false
  ++sequence
  ++historySequence
  clearInterval(ticker)
  for (const control of controls) control.abort()
  content.value = ''
})
function valueText(value: unknown) {
  const text = typeof value === 'string' ? value : JSON.stringify(value)
  return text && text.length > 100 ? text.slice(0, 100) + '…' : text
}
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">DATA WORKSPACE</div>
      <h1>数据工具</h1>
      <p>使用明确的资源字段交换 CSV 和 XLSX，先检查再导入。</p>
    </div>
    <el-button v-if="busy" @click="cancel">停止当前操作</el-button>
  </div>
  <el-alert v-if="error" :title="error" type="error" :closable="false" class="data-alert" />
  <section class="surface data-options">
    <el-form label-position="top" class="data-grid">
      <el-form-item label="数据资源"
        ><el-select v-model="resource" aria-label="数据资源" :disabled="busy"
          ><el-option
            v-for="item in resources"
            :key="item.key"
            :label="item.label"
            :value="item.key" /></el-select
      ></el-form-item>
      <el-form-item label="表格格式"
        ><el-select v-model="format" aria-label="表格格式" :disabled="busy"
          ><el-option label="CSV" value="csv" /><el-option label="XLSX" value="xlsx" /></el-select
      ></el-form-item>
      <el-form-item label="导出筛选"
        ><el-input
          v-model="search"
          placeholder="按资源标识前缀筛选"
          maxlength="100"
          :disabled="busy"
          clearable
      /></el-form-item>
    </el-form>
    <div class="data-field-label">导出字段</div>
    <el-checkbox-group v-model="fields" :disabled="busy"
      ><el-checkbox v-for="field in selected?.fields ?? []" :key="field" :value="field"
        >{{ labels[field] ?? field }} <span class="subtle">{{ field }}</span></el-checkbox
      ></el-checkbox-group
    >
    <div class="data-actions">
      <el-button :disabled="busy || !resource || !fields.length" @click="download(true)"
        >下载空白模板</el-button
      ><el-button
        type="primary"
        :disabled="busy || !resource || !fields.length || !session.can('exports.write')"
        @click="download(false)"
        >导出数据</el-button
      >
    </div>
    <p class="tiny subtle">
      每次最多 2000 行、6 MiB。用户资料按当前数据范围导出。CSV 公式危险前缀会加单引号；XLSX
      字符串保留原文。
    </p>
  </section>
  <section v-if="canImport" class="surface data-options">
    <h2>导入{{ selected?.label }}</h2>
    <p class="subtle">
      仅创建新数据，现有标识不会被覆盖。明细需先有对应父字典。文件最多 6 MiB、2000 行；XLSX
      只接受一个无公式工作表。
    </p>
    <input ref="fileInput" type="file" accept=".csv,.xlsx" hidden @change="browserSelected" />
    <div class="data-actions">
      <el-button :disabled="busy" @click="choose">选择数据文件</el-button
      ><span>{{ fileName || '尚未选择文件' }}</span>
    </div>
    <el-form label-position="top"
      ><el-form-item label="字段映射"
        ><el-input
          v-model="mapping"
          aria-label="字段映射"
          type="textarea"
          :rows="3"
          :disabled="busy"
          placeholder='{"name":"名称","key":"标识","valueType":"类型","value":"值"}' /></el-form-item
    ></el-form>
    <p class="tiny subtle">
      {} 使用模板中的同名表头；必填映射：{{ selected?.requiredFields.join('、') }}。启用默认
      true，明细排序默认 0；省略参数分组列时默认 general。
    </p>
    <el-button type="primary" :disabled="busy || !content" @click="makePreview"
      >生成导入预览</el-button
    >
  </section>
  <section v-if="preview" class="surface data-options data-import-results">
    <div class="data-result-heading">
      <div>
        <h2>逐行预览与结果</h2>
        <p class="tiny subtle">
          导入标识 {{ preview.id }} · {{ preview.rows.length }} 行 · {{ ready }} 行可导入
        </p>
      </div>
      <div class="data-actions">
        <el-button :disabled="busy" @click="loadImport(preview.id)">查询最新结果</el-button
        ><el-button
          v-if="canApply"
          type="primary"
          :disabled="busy || !ready || expired"
          @click="apply"
          >应用可导入行</el-button
        >
      </div>
    </div>
    <el-alert
      v-if="expired && ready"
      title="预览已过期，请选择文件重新生成；已创建行的回执仍可查询。"
      type="warning"
      :closable="false"
    />
    <el-table :data="pageRows" row-key="index"
      ><el-table-column type="expand" width="48"
        ><template #default="{ row }"
          ><dl class="data-row-details">
            <template v-for="(value, field) in row.values" :key="field"
              ><dt>{{ labels[String(field)] ?? field }}</dt>
              <dd>
                {{ typeof value === 'string' ? value : JSON.stringify(value, null, 2) }}
              </dd></template
            >
          </dl></template
        ></el-table-column
      ><el-table-column label="行" width="65"
        ><template #default="{ row }">{{ row.index + 2 }}</template></el-table-column
      ><el-table-column
        prop="key"
        label="资源标识"
        min-width="160"
        show-overflow-tooltip
      /><el-table-column label="名称" min-width="130" show-overflow-tooltip
        ><template #default="{ row }">{{
          row.values.name ?? row.values.label
        }}</template></el-table-column
      ><el-table-column label="值" min-width="160" show-overflow-tooltip
        ><template #default="{ row }">{{
          valueText('value' in row.values ? row.values.value : row.values.enabled)
        }}</template></el-table-column
      ><el-table-column label="状态" width="100"
        ><template #default="{ row }"
          ><el-tag
            :type="
              row.status === 'applied' ? 'success' : row.status === 'ready' ? 'info' : 'warning'
            "
            >{{ statuses[row.status] }}</el-tag
          ></template
        ></el-table-column
      ><el-table-column label="问题" min-width="150"
        ><template #default="{ row }">{{
          problems[row.error] ?? row.error ?? '—'
        }}</template></el-table-column
      ></el-table
    >
    <el-pagination
      v-model:current-page="page"
      :page-size="20"
      :total="preview.rows.length"
      layout="total, prev, pager, next"
    />
  </section>
  <section class="surface data-options data-import-history">
    <h2>我的最近导入</h2>
    <p class="tiny subtle">
      重新打开页面后，可从这里查询结果。导入预览 24 小时内可应用，每批最多 100
      行；已提交行的重试返回同一回执。
    </p>
    <el-table :data="history" row-key="id"
      ><el-table-column label="资源" min-width="110"
        ><template #default="{ row }">{{
          resources.find((r) => r.key === row.resource)?.label ?? row.resource
        }}</template></el-table-column
      ><el-table-column prop="id" label="导入标识" min-width="210" /><el-table-column
        prop="requestId"
        label="请求标识"
        min-width="280"
      /><el-table-column label="操作" width="110"
        ><template #default="{ row }"
          ><el-button text :disabled="busy" @click="loadImport(row.id)"
            >查看结果</el-button
          ></template
        ></el-table-column
      ></el-table
    >
  </section>
</template>
<style scoped>
.data-options {
  padding: 24px;
  margin-bottom: 20px;
}
.data-row-details {
  display: grid;
  grid-template-columns: 120px 1fr;
  gap: 12px;
  margin: 18px 26px;
}
.data-row-details dt {
  color: var(--el-text-color-secondary);
}
.data-row-details dd {
  margin: 0;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  max-height: 320px;
  overflow: auto;
}
.data-options h2 {
  margin: 0 0 12px;
  font-size: 18px;
}
.data-grid {
  display: grid;
  grid-template-columns: 1fr 1fr 2fr;
  gap: 20px;
}
.data-grid .el-select {
  width: 100%;
}
.data-field-label {
  margin-bottom: 8px;
  font-weight: 600;
}
.data-actions {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin: 16px 0;
}
.data-result-heading {
  display: flex;
  justify-content: space-between;
  gap: 20px;
  align-items: center;
}
.data-alert {
  margin-bottom: 18px;
}
.el-pagination {
  margin-top: 18px;
  justify-content: flex-end;
}
@media (max-width: 1000px) {
  .data-grid {
    grid-template-columns: 1fr;
  }
  .data-result-heading {
    display: block;
  }
}
</style>
