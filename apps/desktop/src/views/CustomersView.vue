<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Plus, Refresh } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import type { CustomerRecord, CustomerOwner } from '../core/types'
import { dateText, errorText, notifyError } from '../ui'

const session = useSession()
const rows = ref<CustomerRecord[]>([]),
  loading = ref(false),
  loadError = ref('')
const page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  status = ref(''),
  sort = ref('-createdAt')
const open = ref(false),
  saving = ref(false),
  error = ref(''),
  editing = ref<CustomerRecord | null>(null)
const owners = ref<CustomerOwner[]>([]),
  ownersLoading = ref(false),
  selectedOwner = ref<CustomerOwner | null>(null)
const form = reactive({
  code: '',
  name: '',
  contactName: '',
  phone: '',
  email: '',
  note: '',
  status: 'active' as 'active' | 'archived',
  ownerId: '',
})
let querySequence = 0,
  ownerSequence = 0
const ownerOptions = computed(() => {
  const options = new Map<string, CustomerOwner>()
  if (selectedOwner.value?.id === form.ownerId)
    options.set(selectedOwner.value.id, selectedOwner.value)
  for (const item of owners.value) options.set(item.id, item)
  return [...options.values()]
})
function ownerLabel(owner: CustomerOwner) {
  return owner.username ? `${owner.displayName} · ${owner.username}` : owner.displayName
}
async function load() {
  const sequence = ++querySequence
  loading.value = true
  loadError.value = ''
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
      search: search.value,
      sort: sort.value,
    })
    if (status.value) query.set('status', status.value)
    const response = await session.request<CustomerRecord[]>(`/customers?${query}`)
    if (sequence !== querySequence) return
    total.value = response.meta?.total ?? 0
    const lastPage = Math.max(1, Math.ceil(total.value / pageSize.value))
    if (page.value > lastPage) {
      page.value = lastPage
      await load()
      return
    }
    rows.value = response.data
  } catch (failure) {
    if (sequence === querySequence) loadError.value = errorText(failure)
  } finally {
    if (sequence === querySequence) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
async function loadOwners(search = '') {
  if (!session.can('customers.write')) return
  const sequence = ++ownerSequence
  ownersLoading.value = true
  try {
    const response = await session.request<CustomerOwner[]>(
      `/customers/owners?pageSize=100&sort=username&search=${encodeURIComponent(search)}`,
    )
    if (sequence === ownerSequence) owners.value = response.data
  } catch (failure) {
    if (sequence === ownerSequence) notifyError(failure)
  } finally {
    if (sequence === ownerSequence) ownersLoading.value = false
  }
}
function ownerChanged(id: string) {
  selectedOwner.value = ownerOptions.value.find((item) => item.id === id) ?? null
}
function edit(row: CustomerRecord | null) {
  editing.value = row
  const own = session.user
    ? {
        id: session.user.id,
        username: session.user.username,
        displayName: session.user.displayName,
      }
    : null
  selectedOwner.value = row
    ? (row.owner ?? { id: row.ownerId, displayName: '归属人不可用', username: '' })
    : own
  Object.assign(form, {
    code: row?.code ?? '',
    name: row?.name ?? '',
    contactName: row?.contactName ?? '',
    phone: row?.phone ?? '',
    email: row?.email ?? '',
    note: row?.note ?? '',
    status: row?.status ?? 'active',
    ownerId: row?.ownerId ?? own?.id ?? '',
  })
  owners.value = []
  error.value = ''
  open.value = true
  void loadOwners()
}
async function save() {
  error.value = ''
  if (!form.code || !form.name.trim() || !form.ownerId) {
    error.value = '请填写客户编号、名称并选择归属人'
    return
  }
  saving.value = true
  try {
    await session.request(editing.value ? `/customers/${editing.value.id}` : '/customers', {
      method: editing.value ? 'PATCH' : 'POST',
      body: { ...form, ...(editing.value ? { version: editing.value.version } : {}) },
    })
    open.value = false
    ElMessage.success('客户已保存')
    await load()
  } catch (failure) {
    error.value = errorText(failure)
  } finally {
    saving.value = false
  }
}
async function remove(row: CustomerRecord) {
  try {
    await ElMessageBox.confirm(`确认删除客户“${row.name}”？`, '删除客户', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    })
    await session.request(`/customers/${row.id}?version=${row.version}`, { method: 'DELETE' })
    ElMessage.success('客户已删除')
    await load()
  } catch (failure) {
    if (failure !== 'cancel' && failure !== 'close') notifyError(failure)
  }
}
onMounted(() => void load())
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">CUSTOMER RELATIONSHIPS</div>
      <h1>客户管理</h1>
      <p>维护联系资料与客户归属，持续跟进每一段合作。</p>
    </div>
    <el-button v-if="session.can('customers.write')" type="primary" :icon="Plus" @click="edit(null)"
      >新增客户</el-button
    >
  </div>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <el-input
        v-model="search"
        class="search-input"
        clearable
        placeholder="按名称或编号前缀搜索"
        @keyup.enter="query"
      /><el-button @click="query">查询</el-button>
      <el-select v-model="status" class="filter-select" aria-label="客户状态筛选" @change="query"
        ><el-option label="全部状态" value="" /><el-option label="在用" value="active" /><el-option
          label="已归档"
          value="archived"
      /></el-select>
      <el-select v-model="sort" class="filter-select" aria-label="客户排序" @change="query"
        ><el-option label="最近创建" value="-createdAt" /><el-option
          label="编号升序"
          value="code" /><el-option label="名称升序" value="name" /><el-option
          label="最近更新"
          value="-updatedAt"
      /></el-select>
      <div class="toolbar-spacer" />
      <el-button text :icon="Refresh" @click="load">刷新</el-button>
    </div>
    <el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-table v-loading="loading" :data="rows" row-key="id">
      <el-table-column label="客户" min-width="220"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><small class="table-secondary">{{ row.code }}</small></template
        ></el-table-column
      >
      <el-table-column label="联系人" min-width="170"
        ><template #default="{ row }"
          ><span>{{ row.contactName || '—' }}</span
          ><small class="table-secondary">{{ row.phone }}</small></template
        ></el-table-column
      >
      <el-table-column label="归属人" min-width="160"
        ><template #default="{ row }"
          ><span>{{ row.owner?.displayName ?? '归属人不可用' }}</span
          ><small class="table-secondary">{{ row.owner?.username }}</small></template
        ></el-table-column
      >
      <el-table-column label="状态" width="100"
        ><template #default="{ row }"
          ><el-tag :type="row.status === 'active' ? 'success' : 'info'">{{
            row.status === 'active' ? '在用' : '已归档'
          }}</el-tag></template
        ></el-table-column
      >
      <el-table-column label="更新时间" min-width="170"
        ><template #default="{ row }">{{ dateText(row.updatedAt) }}</template></el-table-column
      >
      <el-table-column v-if="session.can('customers.write')" label="操作" width="130" fixed="right"
        ><template #default="{ row }"
          ><el-button link type="primary" @click="edit(row as CustomerRecord)">编辑</el-button
          ><el-button link type="danger" @click="remove(row as CustomerRecord)"
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
    :title="editing ? '编辑客户' : '新增客户'"
    width="640px"
    :close-on-click-modal="false"
    :show-close="!saving"
    :close-on-press-escape="!saving"
  >
    <el-form label-position="top" class="editor-form" :disabled="saving">
      <div class="form-two">
        <el-form-item label="客户编号"
          ><el-input v-model="form.code" maxlength="64" placeholder="如 CUST-001" /></el-form-item
        ><el-form-item label="客户名称"
          ><el-input v-model="form.name" maxlength="80"
        /></el-form-item>
      </div>
      <div class="form-two">
        <el-form-item label="联系人"
          ><el-input v-model="form.contactName" maxlength="80" /></el-form-item
        ><el-form-item label="电话"><el-input v-model="form.phone" maxlength="32" /></el-form-item>
      </div>
      <el-form-item label="邮箱"><el-input v-model="form.email" maxlength="254" /></el-form-item>
      <div class="form-two">
        <el-form-item label="归属人"
          ><el-select
            v-model="form.ownerId"
            filterable
            remote
            :remote-method="loadOwners"
            :loading="ownersLoading"
            :disabled="session.user?.dataScope === 'self'"
            placeholder="搜索归属人账号"
            @change="ownerChanged"
            ><el-option
              v-for="owner in ownerOptions"
              :key="owner.id"
              :value="owner.id"
              :label="ownerLabel(owner)" /></el-select></el-form-item
        ><el-form-item label="客户状态"
          ><el-select v-model="form.status"
            ><el-option label="在用" value="active" /><el-option
              label="已归档"
              value="archived" /></el-select
        ></el-form-item>
      </div>
      <p class="owner-hint">归属变更会影响成员可见范围；有客户的账号需先转移归属才能删除。</p>
      <el-form-item label="备注"
        ><el-input v-model="form.note" type="textarea" :rows="3" maxlength="2000" show-word-limit
      /></el-form-item>
      <el-alert v-if="error" :title="error" type="error" :closable="false" />
    </el-form>
    <template #footer
      ><el-button :disabled="saving" @click="open = false">取消</el-button
      ><el-button type="primary" :loading="saving" @click="save">保存客户</el-button></template
    >
  </el-dialog>
</template>
<style scoped>
.search-input {
  width: 240px;
}
.filter-select {
  width: 130px;
}
.owner-hint {
  color: #64748b;
  font-size: 12px;
  line-height: 1.6;
  margin: 0 0 18px;
}
</style>
