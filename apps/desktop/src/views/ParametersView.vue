<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Plus, Refresh } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import type { ParameterRecord, ParameterType } from '../core/types'
import { dateText, errorText, notifyError } from '../ui'

const session = useSession()
const rows = ref<ParameterRecord[]>([]),
  loading = ref(false),
  loadError = ref('')
const page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  sort = ref('-createdAt')
const open = ref(false),
  saving = ref(false),
  error = ref(''),
  editing = ref<ParameterRecord | null>(null)
const form = reactive({
  name: '',
  key: '',
  group: 'general',
  description: '',
  valueType: 'string' as ParameterType,
  valueText: '',
  booleanValue: false,
})
const types: { value: ParameterType; label: string }[] = [
  { value: 'string', label: '字符串' },
  { value: 'number', label: '数字' },
  { value: 'boolean', label: '布尔' },
  { value: 'json', label: 'JSON' },
]
let sequence = 0

async function load() {
  const request = ++sequence
  loading.value = true
  loadError.value = ''
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
      search: search.value,
      sort: sort.value,
    })
    const response = await session.request<ParameterRecord[]>(`/parameters?${query}`)
    if (request !== sequence) return
    rows.value = response.data
    total.value = response.meta?.total ?? 0
  } catch (failure) {
    if (request === sequence) loadError.value = errorText(failure)
  } finally {
    if (request === sequence) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
function edit(row: ParameterRecord | null) {
  editing.value = row
  Object.assign(form, {
    name: row?.name ?? '',
    key: row?.key ?? '',
    group: row?.group ?? 'general',
    description: row?.description ?? '',
    valueType: row?.valueType ?? 'string',
    valueText: row
      ? row.valueType === 'string'
        ? String(row.value)
        : JSON.stringify(row.value, null, 2)
      : '',
    booleanValue: row?.value === true,
  })
  error.value = ''
  open.value = true
}
function safeJsonNumbers(value: unknown): boolean {
  if (typeof value === 'number')
    return Number.isFinite(value) && Math.abs(value) <= Number.MAX_SAFE_INTEGER
  if (Array.isArray(value)) return value.every(safeJsonNumbers)
  if (value && typeof value === 'object') return Object.values(value).every(safeJsonNumbers)
  return true
}
function parseValue(): unknown {
  if (form.valueType === 'string') return form.valueText
  if (form.valueType === 'boolean') return form.booleanValue
  if (form.valueType === 'number') {
    let value: unknown
    try {
      value = JSON.parse(form.valueText)
    } catch {
      throw new Error('请输入有效数字')
    }
    if (
      typeof value !== 'number' ||
      !Number.isFinite(value) ||
      Math.abs(value) > Number.MAX_SAFE_INTEGER
    )
      throw new Error('请输入安全范围内的有限数字')
    return value
  }
  let value: unknown
  try {
    value = JSON.parse(form.valueText)
  } catch {
    throw new Error('请输入有效的 JSON')
  }
  if (!safeJsonNumbers(value)) throw new Error('JSON 中的数字超出安全范围，请使用字符串表示大整数')
  return value
}
async function save() {
  error.value = ''
  if (!form.name.trim() || !form.key || !form.group.trim()) {
    error.value = '请填写参数名称、标识和分组'
    return
  }
  let value: unknown
  try {
    value = parseValue()
  } catch (failure) {
    error.value = errorText(failure)
    return
  }
  saving.value = true
  try {
    await session.request(editing.value ? `/parameters/${editing.value.id}` : '/parameters', {
      method: editing.value ? 'PATCH' : 'POST',
      body: {
        name: form.name,
        key: form.key,
        group: form.group,
        description: form.description,
        valueType: form.valueType,
        value,
        ...(editing.value ? { version: editing.value.version } : {}),
      },
    })
    open.value = false
    ElMessage.success('参数已保存')
    await load()
  } catch (failure) {
    error.value = errorText(failure)
  } finally {
    saving.value = false
  }
}
async function remove(row: ParameterRecord) {
  try {
    await ElMessageBox.confirm(`确认删除参数“${row.name}”？`, '删除参数', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    })
    await session.request(`/parameters/${row.id}?version=${row.version}`, { method: 'DELETE' })
    ElMessage.success('参数已删除')
    if (rows.value.length === 1 && page.value > 1) page.value--
    await load()
  } catch (failure) {
    if (failure !== 'cancel' && failure !== 'close') notifyError(failure)
  }
}
function preview(row: ParameterRecord) {
  const value = typeof row.value === 'string' ? row.value : JSON.stringify(row.value)
  return value.length > 160 ? value.slice(0, 160) + '…' : value
}
onMounted(() => void load())
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">BUSINESS SETTINGS</div>
      <h1>参数管理</h1>
      <p>集中维护业务开关和配置值，让每个参数都有清晰的类型。</p>
    </div>
    <el-button
      v-if="session.can('parameters.write')"
      type="primary"
      :icon="Plus"
      @click="edit(null)"
      >新增参数</el-button
    >
  </div>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <el-input
        v-model="search"
        class="search-input"
        clearable
        placeholder="按参数标识前缀搜索"
        @keyup.enter="query"
      /><el-button @click="query">查询</el-button>
      <el-select v-model="sort" class="sort-select" aria-label="参数排序" @change="query"
        ><el-option label="最近创建" value="-createdAt" /><el-option
          label="标识升序"
          value="key" /><el-option label="名称升序" value="name" /><el-option
          label="分组升序"
          value="group"
      /></el-select>
      <div class="toolbar-spacer" />
      <el-button :icon="Refresh" text @click="load">刷新</el-button>
    </div>
    <el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-table v-loading="loading" :data="rows" row-key="id">
      <el-table-column label="参数" min-width="210"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><small class="table-secondary">{{ row.key }}</small></template
        ></el-table-column
      >
      <el-table-column prop="group" label="分组" min-width="110" />
      <el-table-column label="类型" width="100"
        ><template #default="{ row }"
          ><el-tag type="info">{{
            types.find((type) => type.value === row.valueType)?.label
          }}</el-tag></template
        ></el-table-column
      >
      <el-table-column label="参数值" min-width="210"
        ><template #default="{ row }"
          ><code class="parameter-value">{{ preview(row as ParameterRecord) }}</code></template
        ></el-table-column
      >
      <el-table-column label="更新时间" min-width="170"
        ><template #default="{ row }">{{ dateText(row.updatedAt) }}</template></el-table-column
      >
      <el-table-column v-if="session.can('parameters.write')" label="操作" width="130" fixed="right"
        ><template #default="{ row }"
          ><el-button link type="primary" @click="edit(row as ParameterRecord)">编辑</el-button
          ><el-button link type="danger" @click="remove(row as ParameterRecord)"
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
    v-model="open"
    :title="editing ? '编辑参数' : '新增参数'"
    width="580px"
    :close-on-click-modal="false"
    :show-close="!saving"
    :close-on-press-escape="!saving"
  >
    <el-form label-position="top" class="editor-form" :disabled="saving">
      <div class="form-two">
        <el-form-item label="参数名称"><el-input v-model="form.name" maxlength="80" /></el-form-item
        ><el-form-item label="参数标识"
          ><el-input v-model="form.key" maxlength="128" placeholder="如 order.timeout"
        /></el-form-item>
      </div>
      <div class="form-two">
        <el-form-item label="分组"><el-input v-model="form.group" maxlength="64" /></el-form-item
        ><el-form-item label="值类型"
          ><el-select v-model="form.valueType"
            ><el-option
              v-for="type in types"
              :key="type.value"
              :value="type.value"
              :label="type.label" /></el-select
        ></el-form-item>
      </div>
      <el-form-item label="参数值">
        <el-switch
          v-if="form.valueType === 'boolean'"
          v-model="form.booleanValue"
          active-text="true"
          inactive-text="false"
        />
        <el-input
          v-else
          v-model="form.valueText"
          :type="form.valueType === 'number' ? 'text' : 'textarea'"
          :rows="form.valueType === 'json' ? 7 : 3"
          :placeholder="
            form.valueType === 'json'
              ? '对象、数组或 null 等有效 JSON'
              : form.valueType === 'number'
                ? '如 10 或 0.5'
                : '字符串允许留空'
          "
        />
      </el-form-item>
      <el-form-item label="描述"
        ><el-input v-model="form.description" type="textarea" maxlength="1000" :rows="2"
      /></el-form-item>
      <el-alert v-if="error" :title="error" type="error" :closable="false" />
    </el-form>
    <template #footer
      ><el-button :disabled="saving" @click="open = false">取消</el-button
      ><el-button type="primary" :loading="saving" @click="save">保存参数</el-button></template
    >
  </el-dialog>
</template>
<style scoped>
.search-input {
  width: 240px;
}
.sort-select {
  width: 140px;
}
.parameter-value {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  color: #475569;
  font-size: 12px;
}
</style>
