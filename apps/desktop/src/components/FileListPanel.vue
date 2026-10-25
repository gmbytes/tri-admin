<script setup lang="ts">
import { ref, reactive, onMounted, onBeforeUnmount, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Refresh } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import { platform } from '../platform'
import { createBrowserFileSink, type FileSink } from '../core/files'
import type { NativeSaveResult } from '../core/file-native'
import type { StoredFile, FileCategory } from '../core/file-models'
import { dateText, errorText, notifyError, scopeText } from '../ui'
import FileCategorySelect from './FileCategorySelect.vue'
const props = defineProps<{ revision: number }>(),
  session = useSession(),
  rows = ref<StoredFile[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  sort = ref('-createdAt'),
  category = ref<string | null>(null),
  loading = ref(false),
  loadError = ref(''),
  categoryNames = ref(new Map<string, string>()),
  open = ref(false),
  editing = ref<StoredFile | null>(null),
  saving = ref(false),
  saveError = ref(''),
  form = reactive({ name: '', categoryId: null as string | null }),
  downloads = ref(new Map<string, number>()),
  controls = new Map<string, AbortController>()
let sequence = 0,
  alive = true
function bytes(value: number) {
  return value < 1024
    ? `${value} B`
    : value < 1024 * 1024
      ? `${(value / 1024).toFixed(1)} KiB`
      : value < 1024 * 1024 * 1024
        ? `${(value / 1024 / 1024).toFixed(1)} MiB`
        : `${(value / 1024 / 1024 / 1024).toFixed(2)} GiB`
}
async function load() {
  const seq = ++sequence
  loading.value = true
  loadError.value = ''
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
      sort: sort.value,
      search: search.value,
    })
    if (category.value) query.set('categoryId', category.value)
    const result = await session.request<StoredFile[]>(`/files?${query}`)
    if (seq !== sequence || !alive) return
    rows.value = result.data
    total.value = result.meta?.total ?? 0
    const ids = [
      ...new Set(
        result.data
          .map((row) => row.categoryId)
          .filter((id): id is string => !!id && !categoryNames.value.has(id)),
      ),
    ]
    await Promise.allSettled(
      ids.map(async (id) => {
        const row = (await session.request<FileCategory>(`/file-categories/${id}`)).data
        if (alive && seq === sequence) categoryNames.value.set(id, row.name)
      }),
    )
  } catch (error) {
    if (seq === sequence && alive) loadError.value = errorText(error)
  } finally {
    if (seq === sequence && alive) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
function edit(row: StoredFile) {
  editing.value = { ...row }
  Object.assign(form, { name: row.name, categoryId: row.categoryId })
  saveError.value = ''
  open.value = true
}
async function save() {
  if (!editing.value) return
  saving.value = true
  saveError.value = ''
  try {
    await session.request(`/files/${editing.value.id}`, {
      method: 'PATCH',
      body: { version: editing.value.version, name: form.name, categoryId: form.categoryId },
    })
    open.value = false
    ElMessage.success('文件资料已保存')
    await load()
  } catch (error) {
    saveError.value = errorText(error)
  } finally {
    saving.value = false
  }
}
async function remove(row: StoredFile) {
  try {
    await ElMessageBox.confirm(`确认删除文件“${row.name}”？删除后将不能继续下载。`, '删除文件', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    })
    await session.request(`/files/${row.id}?version=${row.version}`, { method: 'DELETE' })
    ElMessage.success('文件已删除')
    if (rows.value.length === 1 && page.value > 1) page.value--
    await load()
  } catch (error) {
    if (error !== 'cancel' && error !== 'close') notifyError(error)
  }
}
async function download(row: StoredFile) {
  if (controls.has(row.id)) return
  const control = new AbortController()
  controls.set(row.id, control)
  downloads.value.set(row.id, 0)
  try {
    const sink: FileSink<Blob | NativeSaveResult> | null = platform.files
      ? await platform.files.chooseTarget(row.name, row, { signal: control.signal })
      : createBrowserFileSink(row)
    if (!sink) return
    const result = await session.downloadFile<Blob | NativeSaveResult>(row.id, row, sink, {
      signal: control.signal,
      onProgress: (value) => {
        if (alive) downloads.value.set(row.id, value)
      },
    })
    if (!alive || control.signal.aborted) return
    if (result instanceof Blob) {
      const url = URL.createObjectURL(result),
        link = document.createElement('a')
      link.href = url
      link.download = row.name
      document.body.append(link)
      link.click()
      link.remove()
      setTimeout(() => URL.revokeObjectURL(url), 1000)
    } else if (result.cleanupPending) ElMessage.warning('文件已保存，临时清理将自动重试')
    else ElMessage.success('文件已保存')
  } catch (error) {
    if (!(
      error &&
      typeof error === 'object' &&
      'code' in error &&
      ['REQUEST_CANCELLED', 'SESSION_CHANGED'].includes(String(error.code))
    ))
      notifyError(error)
  } finally {
    controls.delete(row.id)
    downloads.value.delete(row.id)
  }
}
onMounted(load)
watch(() => props.revision, load)
onBeforeUnmount(() => {
  alive = false
  sequence++
  for (const control of controls.values()) control.abort()
})
</script>
<template>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <el-input
        v-model="search"
        class="search-input"
        clearable
        placeholder="按文件名称前缀搜索"
        @keyup.enter="query"
      /><FileCategorySelect v-model="category" class="file-filter" /><el-button @click="query"
        >查询</el-button
      ><el-select v-model="sort" class="sort-select" aria-label="文件排序" @change="query"
        ><el-option label="最近创建" value="-createdAt" /><el-option label="名称升序" value="name"
      /></el-select>
      <div class="toolbar-spacer" />
      <el-tag type="info">{{ scopeText(session.user?.dataScope) }}</el-tag
      ><el-button text :icon="Refresh" @click="load">刷新</el-button>
    </div>
    <el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-table class="files-table" v-loading="loading" :data="rows" row-key="id"
      ><el-table-column label="文件" min-width="250"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><small class="table-secondary">{{ row.mediaType }}</small
          ><small v-if="downloads.has(row.id)" class="table-secondary"
            >正在下载 {{ bytes(downloads.get(row.id) ?? 0) }} / {{ bytes(row.size) }}
            <el-button link @click="controls.get(row.id)?.abort()">取消下载</el-button></small
          ></template
        ></el-table-column
      ><el-table-column label="分类" min-width="160"
        ><template #default="{ row }">{{
          row.categoryId ? (categoryNames.get(row.categoryId) ?? '分类不可用') : '未分类'
        }}</template></el-table-column
      ><el-table-column label="大小" width="120"
        ><template #default="{ row }">{{ bytes(row.size) }}</template></el-table-column
      ><el-table-column label="创建时间" min-width="170"
        ><template #default="{ row }">{{ dateText(row.createdAt) }}</template></el-table-column
      ><el-table-column label="操作" width="200" fixed="right"
        ><template #default="{ row }"
          ><el-button
            link
            type="primary"
            :disabled="controls.has(row.id)"
            @click="download(row as StoredFile)"
            >下载</el-button
          ><el-button
            v-if="session.can('files.write')"
            link
            type="primary"
            @click="edit(row as StoredFile)"
            >编辑</el-button
          ><el-button
            v-if="session.can('files.write')"
            link
            type="danger"
            @click="remove(row as StoredFile)"
            >删除</el-button
          ></template
        ></el-table-column
      ></el-table
    >
    <div class="pagination-row">
      <span class="subtle">共 {{ total }} 个文件</span
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
  <el-dialog v-model="open" title="修改文件资料" width="560px" :close-on-click-modal="false"
    ><el-alert v-if="saveError" :title="saveError" type="error" :closable="false" /><el-form
      label-position="top"
      ><el-form-item label="文件名称"><el-input v-model="form.name" /></el-form-item
      ><el-form-item label="文件分类"
        ><FileCategorySelect v-model="form.categoryId" /></el-form-item></el-form
    ><template #footer
      ><el-button @click="open = false">取消</el-button
      ><el-button type="primary" :loading="saving" @click="save">保存文件</el-button></template
    ></el-dialog
  >
</template>
