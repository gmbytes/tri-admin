<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { useSession } from '../stores/session'
import type { DataResource } from '../core/data-tools'
import {
  parseTemplate,
  parseQuerySchema,
  validateFilters,
  type ExportTemplate,
  type QuerySchema,
  type TemplateColumn,
  type DataFilter,
  type DataSort,
} from '../core/export-templates'
import TemplateFilterEditor from './TemplateFilterEditor.vue'
import { dateText, errorText } from '../ui'
const props = defineProps<{ resources: DataResource[]; disabled: boolean }>(),
  emit = defineEmits<{ select: [ExportTemplate]; removed: [string] }>(),
  session = useSession()
const rows = ref<ExportTemplate[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  filter = ref(''),
  loading = ref(false),
  error = ref('')
const open = ref(false),
  saving = ref(false),
  dialogError = ref(''),
  editing = ref<ExportTemplate | null>(null),
  schema = ref<QuerySchema | null>(null),
  schemaLoading = ref(false)
const form = reactive({
  name: '',
  code: '',
  resource: 'parameters',
  columns: [] as TemplateColumn[],
  search: '',
  filters: [] as DataFilter[],
  sort: null as DataSort | null,
  maxRows: 2000,
  allowImport: false,
  enabled: true,
})
const definition = computed(() => props.resources.find((value) => value.key === form.resource))
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
  createdAt: '创建时间',
}
function cloneFilters(filters: DataFilter[]): DataFilter[] {
  return filters.map((filter) => ({
    ...filter,
    value: Array.isArray(filter.value) ? [...filter.value] : filter.value,
  }))
}
const copyOpen = ref(false),
  copying = ref(false),
  copySource = ref<ExportTemplate | null>(null),
  copyForm = reactive({ name: '', code: '' }),
  copyError = ref('')
let alive = true,
  sequence = 0,
  schemaSequence = 0,
  current: AbortController | null = null,
  schemaRequest: AbortController | null = null
async function load() {
  if (!props.resources.length) return
  const request = ++sequence
  current?.abort()
  const control = new AbortController()
  current = control
  loading.value = true
  error.value = ''
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
      search: search.value,
      sort: '-createdAt',
    })
    if (filter.value) query.set('resource', filter.value)
    const result = await session.request<unknown[]>('/export-templates?' + query, {
      signal: control.signal,
    })
    if (!alive || request !== sequence || control.signal.aborted) return
    rows.value = result.data.map((value) => parseTemplate(value, props.resources))
    total.value = result.meta?.total ?? 0
  } catch (failure) {
    if (alive && request === sequence && !control.signal.aborted) error.value = errorText(failure)
  } finally {
    if (alive && request === sequence) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
async function loadSchema() {
  const token = ++schemaSequence
  schemaRequest?.abort()
  const control = new AbortController()
  schemaRequest = control
  schema.value = null
  schemaLoading.value = true
  try {
    const result = await session.request<unknown>(
      `/data-tools/resources/${form.resource}/query-schema`,
      { signal: control.signal },
    )
    if (alive && token === schemaSequence && !control.signal.aborted)
      schema.value = parseQuerySchema(result.data)
  } catch (failure) {
    if (alive && token === schemaSequence && !control.signal.aborted)
      dialogError.value = errorText(failure)
  } finally {
    if (alive && token === schemaSequence) schemaLoading.value = false
  }
}
function defaults() {
  form.columns = (definition.value?.fields ?? []).map((field) => ({ field, header: field }))
  form.filters = []
  form.sort = null
  form.allowImport = false
  void loadSchema()
}
function edit(record: ExportTemplate | null) {
  editing.value = record
  dialogError.value = ''
  if (record)
    Object.assign(form, {
      name: record.name,
      code: record.code,
      resource: record.resource,
      columns: record.columns.map((column) => ({ ...column })),
      search: record.search,
      filters: cloneFilters(record.filters),
      sort: record.sort ? { ...record.sort } : null,
      maxRows: record.maxRows,
      allowImport: record.allowImport,
      enabled: record.enabled,
    })
  else {
    Object.assign(form, {
      name: '',
      code: '',
      resource: props.resources[0]?.key ?? '',
      search: '',
      maxRows: 2000,
      enabled: true,
    })
    form.columns = (definition.value?.fields ?? []).map((field) => ({ field, header: field }))
    form.filters = []
    form.sort = null
    form.allowImport = false
  }
  open.value = true
  void loadSchema()
}
function selectFields(values: unknown) {
  const fields = Array.isArray(values) ? values.map(String) : []
  form.columns = fields.map(
    (field) => form.columns.find((column) => column.field === field) ?? { field, header: field },
  )
}
function move(index: number, offset: number) {
  const other = index + offset
  if (other < 0 || other >= form.columns.length) return
  const next = [...form.columns]
  ;[next[index], next[other]] = [next[other], next[index]]
  form.columns = next
}
async function save() {
  dialogError.value = ''
  if (!form.name.trim() || !form.code || !form.columns.length) {
    dialogError.value = '请填写名称、标识并选择字段'
    return
  }
  if (!schema.value) {
    dialogError.value = '资源查询定义尚未加载'
    return
  }
  if (!Number.isInteger(form.maxRows) || form.maxRows < 1 || form.maxRows > 2000) {
    dialogError.value = '同步行数上限须为 1–2000'
    return
  }
  const invalid = validateFilters(form.filters, schema.value)
  if (invalid) {
    dialogError.value = invalid
    return
  }
  if (
    form.allowImport &&
    definition.value?.requiredFields.some(
      (field) => !form.columns.some((column) => column.field === field),
    )
  ) {
    dialogError.value = '导入模板需要包含全部必填字段'
    return
  }
  saving.value = true
  try {
    const data = {
      name: form.name,
      columns: form.columns.map((column) => ({ ...column })),
      search: form.search,
      filters: cloneFilters(form.filters),
      sort: form.sort,
      maxRows: form.maxRows,
      allowImport: form.allowImport,
      enabled: form.enabled,
    }
    const result = await session.request<unknown>(
      editing.value ? `/export-templates/${editing.value.id}` : '/export-templates',
      {
        method: editing.value ? 'PATCH' : 'POST',
        body: editing.value
          ? { ...data, version: editing.value.version }
          : { ...data, code: form.code, resource: form.resource },
      },
    )
    parseTemplate(result.data, props.resources)
    open.value = false
    ElMessage.success('模板已保存')
    await load()
  } catch (failure) {
    dialogError.value = errorText(failure)
  } finally {
    saving.value = false
  }
}
async function remove(record: ExportTemplate) {
  try {
    await ElMessageBox.confirm(`删除模板“${record.name}”？已有导入回执会保留。`, '删除模板', {
      confirmButtonText: '删除',
      cancelButtonText: '取消',
      type: 'warning',
    })
    await session.request(`/export-templates/${record.id}?version=${record.version}`, {
      method: 'DELETE',
    })
    emit('removed', record.id)
    await load()
  } catch (failure) {
    if (failure !== 'cancel' && failure !== 'close') error.value = errorText(failure)
  }
}
function copy(record: ExportTemplate) {
  copySource.value = record
  Object.assign(copyForm, { name: record.name + '（复制）', code: '' })
  copyError.value = ''
  copyOpen.value = true
}
async function saveCopy() {
  if (!copySource.value) return
  copying.value = true
  copyError.value = ''
  try {
    await session.request(`/export-templates/${copySource.value.id}/copy`, {
      method: 'POST',
      body: { version: copySource.value.version, name: copyForm.name, code: copyForm.code },
    })
    copyOpen.value = false
    await load()
  } catch (failure) {
    copyError.value = errorText(failure)
  } finally {
    copying.value = false
  }
}
watch(
  () => props.resources,
  () => void load(),
)
onMounted(() => void load())
onBeforeUnmount(() => {
  alive = false
  sequence++
  schemaSequence++
  current?.abort()
  schemaRequest?.abort()
})
</script>
<template>
  <section class="surface export-templates-panel">
    <div class="template-heading">
      <div>
        <h2>命名模板</h2>
        <p class="tiny subtle">模板是共享配置；数据读取仍按当前账号权限和范围执行。</p>
      </div>
      <el-button
        v-if="session.can('exports.write')"
        type="primary"
        :disabled="disabled || !resources.length"
        @click="edit(null)"
        >新建模板</el-button
      >
    </div>
    <div class="template-toolbar">
      <el-input
        v-model="search"
        placeholder="按模板标识前缀搜索"
        clearable
        @keyup.enter="query"
      /><el-select v-model="filter" aria-label="模板资源筛选" placeholder="全部资源" @change="query"
        ><el-option label="全部资源" value="" /><el-option
          v-for="item in resources"
          :key="item.key"
          :label="item.label"
          :value="item.key" /></el-select
      ><el-button @click="query">查询模板</el-button>
    </div>
    <el-alert v-if="error" :title="error" type="error" :closable="false" /><el-table
      v-loading="loading"
      :data="rows"
      row-key="id"
      ><el-table-column label="模板" min-width="180"
        ><template #default="{ row }"
          ><div>{{ row.name }}</div>
          <div class="tiny subtle">{{ row.code }}</div></template
        ></el-table-column
      ><el-table-column label="资源" width="130"
        ><template #default="{ row }">{{
          resources.find((value) => value.key === row.resource)?.label ?? row.resource
        }}</template></el-table-column
      ><el-table-column label="版本 / 状态" width="130"
        ><template #default="{ row }"
          >v{{ row.version }} · {{ row.enabled ? '启用' : '停用' }}</template
        ></el-table-column
      ><el-table-column label="更新时间" min-width="160"
        ><template #default="{ row }">{{ dateText(row.updatedAt) }}</template></el-table-column
      ><el-table-column label="操作" min-width="285"
        ><template #default="{ row }"
          ><el-button text :disabled="disabled || !row.enabled" @click="emit('select', row)"
            >使用模板</el-button
          ><template v-if="session.can('exports.write')"
            ><el-button text :disabled="disabled" @click="edit(row as ExportTemplate)"
              >编辑</el-button
            ><el-button text :disabled="disabled" @click="copy(row as ExportTemplate)"
              >复制</el-button
            ><el-button
              text
              type="danger"
              :disabled="disabled"
              @click="remove(row as ExportTemplate)"
              >删除</el-button
            ></template
          ></template
        ></el-table-column
      ></el-table
    ><el-pagination
      v-model:current-page="page"
      :page-size="pageSize"
      :total="total"
      layout="total, prev, pager, next"
      @current-change="load"
    />
  </section>
  <el-dialog
    v-model="open"
    class="tri-template-editor"
    :title="editing ? '编辑模板' : '新建模板'"
    width="min(900px, calc(100vw - 32px))"
    top="5vh"
    :close-on-click-modal="false"
    ><el-alert v-if="dialogError" :title="dialogError" type="error" :closable="false" /><el-form
      label-position="top"
      ><div class="template-form-grid">
        <el-form-item label="模板名称"
          ><el-input
            v-model="form.name"
            aria-label="模板名称"
            maxlength="80"
            :disabled="saving" /></el-form-item
        ><el-form-item label="模板标识"
          ><el-input
            v-model="form.code"
            aria-label="模板标识"
            maxlength="64"
            :disabled="saving || !!editing" /></el-form-item
        ><el-form-item label="模板资源"
          ><el-select
            v-model="form.resource"
            aria-label="模板资源"
            :disabled="saving || !!editing"
            @change="defaults"
            ><el-option
              v-for="item in resources"
              :key="item.key"
              :label="item.label"
              :value="item.key" /></el-select></el-form-item
        ><el-form-item label="同步最大行数"
          ><el-input-number
            v-model="form.maxRows"
            :min="1"
            :max="2000"
            :precision="0"
            :disabled="saving"
        /></el-form-item>
      </div>
      <el-form-item label="模板字段"
        ><el-select
          :model-value="form.columns.map((column) => column.field)"
          multiple
          aria-label="模板字段"
          :disabled="saving"
          @update:model-value="selectFields"
          ><el-option
            v-for="field in definition?.fields ?? []"
            :key="field"
            :label="labels[field] ?? field"
            :value="field" /></el-select
      ></el-form-item>
      <div v-for="(column, index) in form.columns" :key="column.field" class="template-column-row">
        <span
          >{{ labels[column.field] ?? column.field }}
          <span class="tiny subtle">{{ column.field }}</span></span
        ><el-input
          v-model="column.header"
          :aria-label="`${column.field} 表头`"
          :disabled="saving"
        /><el-button
          text
          :disabled="saving || index === 0"
          :aria-label="`上移 ${column.field}`"
          @click="move(index, -1)"
          >上移</el-button
        ><el-button
          text
          :disabled="saving || index === form.columns.length - 1"
          :aria-label="`下移 ${column.field}`"
          @click="move(index, 1)"
          >下移</el-button
        >
      </div>
      <div class="template-flags">
        <el-checkbox
          v-model="form.allowImport"
          :disabled="saving || !definition?.importable"
          aria-label="允许导入"
          >允许导入</el-checkbox
        ><el-checkbox v-model="form.enabled" :disabled="saving">启用模板</el-checkbox>
      </div>
      <p class="tiny subtle">导入只创建新标识，仍需资源写入权限；以下条件只用于导出。</p>
      <el-form-item label="导出标识前缀"
        ><el-input
          v-model="form.search"
          aria-label="导出标识前缀"
          maxlength="100"
          :disabled="saving" /></el-form-item
      ><el-form-item label="导出排序"
        ><el-select
          :model-value="form.sort?.field ?? ''"
          aria-label="导出排序字段"
          placeholder="资源默认顺序"
          :disabled="saving || schemaLoading"
          @update:model-value="
            (value) =>
              (form.sort = value
                ? { field: String(value), direction: form.sort?.direction ?? 'asc' }
                : null)
          "
          ><el-option label="资源默认顺序" value="" /><el-option
            v-for="field in schema?.sortFields ?? []"
            :key="field"
            :label="labels[field] ?? field"
            :value="field" /></el-select
        ><el-select
          v-if="form.sort"
          v-model="form.sort.direction"
          aria-label="排序方向"
          :disabled="saving"
          ><el-option label="升序" value="asc" /><el-option
            label="降序"
            value="desc" /></el-select></el-form-item
      ><el-form-item label="导出筛选"
        ><TemplateFilterEditor
          v-model="form.filters"
          :schema="schema"
          :disabled="saving || schemaLoading" /></el-form-item></el-form
    ><template #footer
      ><el-button :disabled="saving" @click="open = false">取消</el-button
      ><el-button
        type="primary"
        :loading="saving"
        :disabled="schemaLoading || !schema"
        @click="save"
        >保存模板</el-button
      ></template
    ></el-dialog
  >
  <el-dialog v-model="copyOpen" title="复制模板" width="500px" :close-on-click-modal="false"
    ><el-alert v-if="copyError" :title="copyError" type="error" :closable="false" /><el-form
      label-position="top"
      ><el-form-item label="模板名称"
        ><el-input
          v-model="copyForm.name"
          aria-label="模板名称"
          :disabled="copying" /></el-form-item
      ><el-form-item label="模板标识"
        ><el-input
          v-model="copyForm.code"
          aria-label="模板标识"
          :disabled="copying" /></el-form-item></el-form
    ><template #footer
      ><el-button @click="copyOpen = false">取消</el-button
      ><el-button type="primary" :loading="copying" @click="saveCopy">复制模板</el-button></template
    ></el-dialog
  >
</template>
<style scoped>
.export-templates-panel {
  padding: 24px;
  margin-bottom: 20px;
}
.template-heading {
  display: flex;
  justify-content: space-between;
  gap: 20px;
}
.template-heading h2 {
  margin: 0 0 12px;
  font-size: 18px;
}
.template-toolbar {
  display: flex;
  gap: 12px;
  margin: 16px 0;
}
.template-toolbar > .el-input {
  width: 260px;
}
.template-toolbar > .el-select {
  width: 180px;
}
.el-pagination {
  margin-top: 18px;
  justify-content: flex-end;
}
.template-form-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 0 20px;
}
.template-form-grid .el-select,
.el-form-item > .el-select {
  width: 100%;
}
.template-column-row {
  display: grid;
  grid-template-columns: 160px 1fr 60px 60px;
  gap: 10px;
  align-items: center;
  margin: 10px 0;
}
.template-flags {
  display: flex;
  gap: 20px;
  margin-top: 18px;
}
</style>
<style>
.tri-template-editor.el-dialog {
  display: flex;
  flex-direction: column;
  max-height: 90vh;
  margin-bottom: 5vh;
}
.tri-template-editor > .el-dialog__header,
.tri-template-editor > .el-dialog__footer {
  flex-shrink: 0;
}
.tri-template-editor > .el-dialog__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}
</style>
