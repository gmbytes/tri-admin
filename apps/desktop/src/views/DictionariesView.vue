<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Plus, Refresh } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import type { DictionaryRecord, DictionaryItem } from '../core/types'
import { dateText, errorText, notifyError } from '../ui'

const session = useSession()
const rows = ref<DictionaryRecord[]>([]),
  loading = ref(false),
  loadError = ref('')
const page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  sort = ref('-createdAt')
const dictionaryOpen = ref(false),
  dictionarySaving = ref(false),
  dictionaryError = ref('')
const editing = ref<DictionaryRecord | null>(null)
const dictionaryForm = reactive({ name: '', key: '', description: '', enabled: true })
const drawerOpen = ref(false),
  selected = ref<DictionaryRecord | null>(null)
const items = ref<DictionaryItem[]>([]),
  itemsLoading = ref(false),
  itemsError = ref('')
const itemPage = ref(1),
  itemPageSize = ref(20),
  itemTotal = ref(0),
  itemSearch = ref('')
const itemOpen = ref(false),
  itemSaving = ref(false),
  itemError = ref('')
const editingItem = ref<DictionaryItem | null>(null)
const itemForm = reactive({ label: '', value: '', sort: 0, enabled: true })
let listSequence = 0,
  itemSequence = 0

async function load() {
  const sequence = ++listSequence
  loading.value = true
  loadError.value = ''
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
      search: search.value,
      sort: sort.value,
    })
    const response = await session.request<DictionaryRecord[]>(`/dictionaries?${query}`)
    if (sequence !== listSequence) return
    rows.value = response.data
    total.value = response.meta?.total ?? 0
  } catch (failure) {
    if (sequence === listSequence) loadError.value = errorText(failure)
  } finally {
    if (sequence === listSequence) loading.value = false
  }
}
function queryDictionaries() {
  page.value = 1
  void load()
}
function edit(row: DictionaryRecord | null) {
  editing.value = row
  Object.assign(dictionaryForm, {
    name: row?.name ?? '',
    key: row?.key ?? '',
    description: row?.description ?? '',
    enabled: row?.enabled ?? true,
  })
  dictionaryError.value = ''
  dictionaryOpen.value = true
}
async function saveDictionary() {
  dictionaryError.value = ''
  if (!dictionaryForm.name.trim() || !dictionaryForm.key) {
    dictionaryError.value = '请填写字典名称和标识'
    return
  }
  dictionarySaving.value = true
  try {
    await session.request(editing.value ? `/dictionaries/${editing.value.id}` : '/dictionaries', {
      method: editing.value ? 'PATCH' : 'POST',
      body: { ...dictionaryForm, ...(editing.value ? { version: editing.value.version } : {}) },
    })
    dictionaryOpen.value = false
    ElMessage.success('字典已保存')
    await load()
  } catch (failure) {
    dictionaryError.value = errorText(failure)
  } finally {
    dictionarySaving.value = false
  }
}
async function removeDictionary(row: DictionaryRecord) {
  try {
    await ElMessageBox.confirm(`确认删除字典“${row.name}”？请先删除其中的活动明细。`, '删除字典', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    })
    await session.request(`/dictionaries/${row.id}?version=${row.version}`, { method: 'DELETE' })
    ElMessage.success('字典已删除')
    if (rows.value.length === 1 && page.value > 1) page.value--
    await load()
  } catch (failure) {
    if (failure !== 'cancel' && failure !== 'close') notifyError(failure)
  }
}
function showItems(row: DictionaryRecord) {
  selected.value = row
  items.value = []
  itemTotal.value = 0
  itemPage.value = 1
  itemSearch.value = ''
  drawerOpen.value = true
  void loadItems()
}
async function loadItems() {
  const parent = selected.value?.id
  if (!parent) return
  const sequence = ++itemSequence
  itemsLoading.value = true
  itemsError.value = ''
  try {
    const query = new URLSearchParams({
      page: String(itemPage.value),
      pageSize: String(itemPageSize.value),
      search: itemSearch.value,
      sort: 'sort',
    })
    const response = await session.request<DictionaryItem[]>(
      `/dictionaries/${parent}/items?${query}`,
    )
    if (sequence !== itemSequence || selected.value?.id !== parent) return
    items.value = response.data
    itemTotal.value = response.meta?.total ?? 0
  } catch (failure) {
    if (sequence === itemSequence) itemsError.value = errorText(failure)
  } finally {
    if (sequence === itemSequence) itemsLoading.value = false
  }
}
function queryItems() {
  itemPage.value = 1
  void loadItems()
}
function editItem(row: DictionaryItem | null) {
  editingItem.value = row
  Object.assign(itemForm, {
    label: row?.label ?? '',
    value: row?.value ?? '',
    sort: row?.sort ?? 0,
    enabled: row?.enabled ?? true,
  })
  itemError.value = ''
  itemOpen.value = true
}
async function saveItem() {
  const parent = selected.value?.id
  if (!parent) return
  itemError.value = ''
  if (!itemForm.label.trim() || !itemForm.value.trim()) {
    itemError.value = '请填写明细名称和值'
    return
  }
  itemSaving.value = true
  try {
    await session.request(
      `/dictionaries/${parent}/items${editingItem.value ? `/${editingItem.value.id}` : ''}`,
      {
        method: editingItem.value ? 'PATCH' : 'POST',
        body: { ...itemForm, ...(editingItem.value ? { version: editingItem.value.version } : {}) },
      },
    )
    itemOpen.value = false
    ElMessage.success('明细已保存')
    await loadItems()
  } catch (failure) {
    itemError.value = errorText(failure)
  } finally {
    itemSaving.value = false
  }
}
async function removeItem(row: DictionaryItem) {
  try {
    await ElMessageBox.confirm(`确认删除明细“${row.label}”？`, '删除明细', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    })
    await session.request(
      `/dictionaries/${row.dictionaryId}/items/${row.id}?version=${row.version}`,
      { method: 'DELETE' },
    )
    ElMessage.success('明细已删除')
    if (items.value.length === 1 && itemPage.value > 1) itemPage.value--
    await loadItems()
  } catch (failure) {
    if (failure !== 'cancel' && failure !== 'close') notifyError(failure)
  }
}
onMounted(() => void load())
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">REFERENCE DATA</div>
      <h1>字典管理</h1>
      <p>维护统一的业务选项、名称与排序。</p>
    </div>
    <el-button
      v-if="session.can('dictionaries.write')"
      type="primary"
      :icon="Plus"
      @click="edit(null)"
      >新增字典</el-button
    >
  </div>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <el-input
        v-model="search"
        class="search-input"
        clearable
        placeholder="按字典标识前缀搜索"
        @keyup.enter="queryDictionaries"
      />
      <el-button @click="queryDictionaries">查询</el-button>
      <el-select
        v-model="sort"
        aria-label="字典排序"
        class="sort-select"
        @change="queryDictionaries"
      >
        <el-option label="最近创建" value="-createdAt" /><el-option
          label="标识升序"
          value="key"
        /><el-option label="名称升序" value="name" />
      </el-select>
      <div class="toolbar-spacer" />
      <el-button text :icon="Refresh" @click="load">刷新</el-button>
    </div>
    <el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-table v-loading="loading" :data="rows" row-key="id">
      <el-table-column label="字典" min-width="200"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><small class="table-secondary">{{ row.key }}</small></template
        ></el-table-column
      >
      <el-table-column prop="description" label="描述" min-width="200" show-overflow-tooltip />
      <el-table-column label="状态" width="100"
        ><template #default="{ row }"
          ><el-tag :type="row.enabled ? 'success' : 'info'">{{
            row.enabled ? '启用' : '停用'
          }}</el-tag></template
        ></el-table-column
      >
      <el-table-column label="更新时间" min-width="170"
        ><template #default="{ row }">{{ dateText(row.updatedAt) }}</template></el-table-column
      >
      <el-table-column label="操作" width="180" fixed="right"
        ><template #default="{ row }">
          <el-button link type="primary" @click="showItems(row as DictionaryRecord)"
            >明细</el-button
          >
          <el-button
            v-if="session.can('dictionaries.write')"
            link
            type="primary"
            @click="edit(row as DictionaryRecord)"
            >编辑</el-button
          >
          <el-button
            v-if="session.can('dictionaries.write')"
            link
            type="danger"
            @click="removeDictionary(row as DictionaryRecord)"
            >删除</el-button
          >
        </template></el-table-column
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
    v-model="dictionaryOpen"
    :title="editing ? '编辑字典' : '新增字典'"
    width="540px"
    :close-on-click-modal="false"
    :show-close="!dictionarySaving"
    :close-on-press-escape="!dictionarySaving"
  >
    <el-form label-position="top" class="editor-form" :disabled="dictionarySaving">
      <el-form-item label="字典名称"
        ><el-input v-model="dictionaryForm.name" maxlength="80"
      /></el-form-item>
      <el-form-item label="字典标识"
        ><el-input
          v-model="dictionaryForm.key"
          maxlength="64"
          placeholder="如 region、order.status"
      /></el-form-item>
      <el-form-item label="描述"
        ><el-input
          v-model="dictionaryForm.description"
          type="textarea"
          maxlength="1000"
          :rows="3"
          show-word-limit
      /></el-form-item>
      <el-form-item label="状态"
        ><el-switch v-model="dictionaryForm.enabled" active-text="启用" inactive-text="停用"
      /></el-form-item>
      <el-alert v-if="dictionaryError" :title="dictionaryError" type="error" :closable="false" />
    </el-form>
    <template #footer
      ><el-button :disabled="dictionarySaving" @click="dictionaryOpen = false">取消</el-button
      ><el-button type="primary" :loading="dictionarySaving" @click="saveDictionary"
        >保存字典</el-button
      ></template
    >
  </el-dialog>
  <el-drawer
    v-model="drawerOpen"
    :title="`${selected?.name ?? ''} · 明细`"
    size="760px"
    :close-on-click-modal="false"
    destroy-on-close
  >
    <div class="dictionary-detail-key">{{ selected?.key }}</div>
    <div class="table-toolbar">
      <el-input
        v-model="itemSearch"
        class="search-input"
        clearable
        placeholder="按明细值前缀搜索"
        @keyup.enter="queryItems"
      />
      <el-button @click="queryItems">查询</el-button>
      <div class="toolbar-spacer" />
      <el-button
        v-if="session.can('dictionaries.write')"
        type="primary"
        :icon="Plus"
        @click="editItem(null)"
        >新增明细</el-button
      >
    </div>
    <el-alert v-if="itemsError" :title="itemsError" type="error" :closable="false" />
    <el-table v-loading="itemsLoading" :data="items" row-key="id">
      <el-table-column prop="label" label="名称" min-width="150" /><el-table-column
        prop="value"
        label="值"
        min-width="130"
      />
      <el-table-column prop="sort" label="排序" width="80" />
      <el-table-column label="状态" width="85"
        ><template #default="{ row }"
          ><el-tag :type="row.enabled ? 'success' : 'info'">{{
            row.enabled ? '启用' : '停用'
          }}</el-tag></template
        ></el-table-column
      >
      <el-table-column v-if="session.can('dictionaries.write')" label="操作" width="120"
        ><template #default="{ row }"
          ><el-button link type="primary" @click="editItem(row as DictionaryItem)">编辑</el-button
          ><el-button link type="danger" @click="removeItem(row as DictionaryItem)"
            >删除</el-button
          ></template
        ></el-table-column
      >
    </el-table>
    <div class="pagination">
      <el-pagination
        v-model:current-page="itemPage"
        v-model:page-size="itemPageSize"
        :page-sizes="[10, 20, 50, 100]"
        layout="total,sizes,prev,pager,next"
        :total="itemTotal"
        @change="loadItems"
      />
    </div>
  </el-drawer>
  <el-dialog
    v-model="itemOpen"
    :title="editingItem ? '编辑明细' : '新增明细'"
    width="500px"
    append-to-body
    :close-on-click-modal="false"
    :show-close="!itemSaving"
    :close-on-press-escape="!itemSaving"
  >
    <el-form label-position="top" class="editor-form" :disabled="itemSaving">
      <el-form-item label="明细名称"
        ><el-input v-model="itemForm.label" maxlength="100"
      /></el-form-item>
      <el-form-item label="明细值"
        ><el-input v-model="itemForm.value" maxlength="200"
      /></el-form-item>
      <el-form-item label="排序"
        ><el-input-number
          v-model="itemForm.sort"
          :min="-2147483648"
          :max="2147483647"
          :precision="0"
      /></el-form-item>
      <el-form-item label="状态"
        ><el-switch v-model="itemForm.enabled" active-text="启用" inactive-text="停用"
      /></el-form-item>
      <el-alert v-if="itemError" :title="itemError" type="error" :closable="false" />
    </el-form>
    <template #footer
      ><el-button :disabled="itemSaving" @click="itemOpen = false">取消</el-button
      ><el-button type="primary" :loading="itemSaving" @click="saveItem"
        >保存明细</el-button
      ></template
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
.dictionary-detail-key {
  color: var(--muted);
  margin-bottom: 14px;
}
</style>
