<script setup lang="ts">
import { ref, reactive, onMounted, onBeforeUnmount } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Refresh, Plus } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import type { FileCategory } from '../core/file-models'
import { dateText, errorText, notifyError } from '../ui'
const emit = defineEmits<{ changed: [] }>(),
  session = useSession(),
  rows = ref<FileCategory[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  sort = ref('code'),
  loading = ref(false),
  loadError = ref(''),
  open = ref(false),
  saving = ref(false),
  saveError = ref(''),
  editing = ref<FileCategory | null>(null),
  form = reactive({ name: '', code: '', description: '' })
let sequence = 0
async function load() {
  const seq = ++sequence
  loading.value = true
  loadError.value = ''
  try {
    const result = await session.request<FileCategory[]>(
      `/file-categories?${new URLSearchParams({ page: String(page.value), pageSize: String(pageSize.value), sort: sort.value, search: search.value })}`,
    )
    if (seq === sequence) {
      rows.value = result.data
      total.value = result.meta?.total ?? 0
    }
  } catch (error) {
    if (seq === sequence) loadError.value = errorText(error)
  } finally {
    if (seq === sequence) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
function edit(row: FileCategory | null) {
  editing.value = row
  Object.assign(form, {
    name: row?.name ?? '',
    code: row?.code ?? '',
    description: row?.description ?? '',
  })
  saveError.value = ''
  open.value = true
}
async function save() {
  if (!form.name.trim() || !form.code) {
    saveError.value = '请填写分类名称和标识'
    return
  }
  saving.value = true
  saveError.value = ''
  try {
    await session.request(
      editing.value ? `/file-categories/${editing.value.id}` : '/file-categories',
      {
        method: editing.value ? 'PATCH' : 'POST',
        body: editing.value
          ? { version: editing.value.version, name: form.name, description: form.description }
          : { ...form },
      },
    )
    open.value = false
    ElMessage.success('分类已保存')
    emit('changed')
    await load()
  } catch (error) {
    saveError.value = errorText(error)
  } finally {
    saving.value = false
  }
}
async function remove(row: FileCategory) {
  try {
    await ElMessageBox.confirm(
      `确认删除分类“${row.name}”？仍被文件或上传任务使用的分类不能删除。`,
      '删除分类',
      { type: 'warning', confirmButtonText: '删除', cancelButtonText: '取消' },
    )
    await session.request(`/file-categories/${row.id}?version=${row.version}`, { method: 'DELETE' })
    ElMessage.success('分类已删除')
    emit('changed')
    if (rows.value.length === 1 && page.value > 1) page.value--
    await load()
  } catch (error) {
    if (error !== 'cancel' && error !== 'close') notifyError(error)
  }
}
onMounted(load)
onBeforeUnmount(() => sequence++)
</script>
<template>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <el-input
        v-model="search"
        clearable
        class="search-input"
        placeholder="按分类标识前缀搜索"
        @keyup.enter="query"
      /><el-button @click="query">查询</el-button
      ><el-select v-model="sort" class="sort-select" aria-label="分类排序" @change="query"
        ><el-option label="标识升序" value="code" /><el-option
          label="名称升序"
          value="name" /><el-option label="最近创建" value="-createdAt"
      /></el-select>
      <div class="toolbar-spacer" />
      <el-button v-if="session.can('files.write')" type="primary" :icon="Plus" @click="edit(null)"
        >新建分类</el-button
      ><el-button text :icon="Refresh" @click="load">刷新</el-button>
    </div>
    <el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-table class="file-categories-table" :data="rows" v-loading="loading" row-key="id"
      ><el-table-column label="分类" min-width="200"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><small class="table-secondary">{{ row.code }}</small></template
        ></el-table-column
      ><el-table-column
        prop="description"
        label="说明"
        min-width="240"
        show-overflow-tooltip
      /><el-table-column label="更新时间" min-width="170"
        ><template #default="{ row }">{{ dateText(row.updatedAt) }}</template></el-table-column
      ><el-table-column v-if="session.can('files.write')" label="操作" width="140"
        ><template #default="{ row }"
          ><el-button link type="primary" @click="edit(row as FileCategory)">编辑</el-button
          ><el-button link type="danger" @click="remove(row as FileCategory)"
            >删除</el-button
          ></template
        ></el-table-column
      ></el-table
    >
    <div class="pagination-row">
      <span class="subtle">共 {{ total }} 个分类</span
      ><el-pagination
        v-model:current-page="page"
        v-model:page-size="pageSize"
        :page-sizes="[20, 50, 100]"
        layout="sizes, prev, pager, next"
        :total="total"
        @current-change="load"
        @size-change="query"
      />
    </div>
  </section>
  <el-dialog
    v-model="open"
    :title="editing ? '修改分类' : '新建分类'"
    width="540px"
    :close-on-click-modal="false"
    ><el-alert v-if="saveError" :title="saveError" type="error" :closable="false" /><el-form
      label-position="top"
      ><el-form-item label="分类名称"><el-input v-model="form.name" maxlength="80" /></el-form-item
      ><el-form-item label="分类标识"
        ><el-input
          v-model="form.code"
          :disabled="!!editing"
          maxlength="64"
          placeholder="小写字母开头，如 resources" /></el-form-item
      ><el-form-item label="分类说明"
        ><el-input
          v-model="form.description"
          type="textarea"
          :rows="3"
          maxlength="1000" /></el-form-item></el-form
    ><template #footer
      ><el-button @click="open = false">取消</el-button
      ><el-button type="primary" :loading="saving" @click="save">保存分类</el-button></template
    ></el-dialog
  >
</template>
