<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Plus, Refresh, Search } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import type { RoleChoice, UserRecord } from '../core/types'
import { dateText, errorText, notifyError } from '../ui'
const session = useSession(),
  rows = ref<UserRecord[]>([]),
  roles = ref<RoleChoice[]>([]),
  rolesLoading = ref(false),
  loading = ref(false),
  saving = ref(false),
  open = ref(false),
  error = ref(''),
  editing = ref<UserRecord | null>(null)
const page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref('')
let sequence = 0
let roleSequence = 0
const form = reactive({
  username: '',
  displayName: '',
  password: '',
  roleIds: [] as string[],
  status: 'active',
})
const canCreate = computed(
  () =>
    session.can('users.write') && session.can('roles.assign') && session.user?.dataScope !== 'self',
)
const roleOptions = computed(() => {
  const choices = new Map<string, RoleChoice>()
  for (const item of editing.value?.roles ?? [])
    if (form.roleIds.includes(item.id)) choices.set(item.id, item)
  for (const item of roles.value) choices.set(item.id, item)
  return [...choices.values()]
})
async function load() {
  const id = ++sequence
  loading.value = true
  try {
    const response = await session.request<UserRecord[]>(
      `/users?page=${page.value}&pageSize=${pageSize.value}&search=${encodeURIComponent(search.value)}&sort=-createdAt`,
    )
    if (id === sequence) {
      rows.value = response.data
      total.value = response.meta?.total ?? 0
    }
  } catch (failure) {
    if (id === sequence) notifyError(failure)
  } finally {
    if (id === sequence) loading.value = false
  }
}
function queryUsers() {
  page.value = 1
  void load()
}
async function loadRoles(search = '') {
  const sequence = ++roleSequence
  if (session.can('roles.assign')) {
    rolesLoading.value = true
    try {
      const response = await session.request<RoleChoice[]>(
        `/roles/assignable?pageSize=100&sort=slug&search=${encodeURIComponent(search)}`,
      )
      if (sequence === roleSequence) roles.value = response.data
    } catch (failure) {
      if (sequence === roleSequence) notifyError(failure)
    } finally {
      if (sequence === roleSequence) rolesLoading.value = false
    }
  }
}
function edit(row: UserRecord | null) {
  editing.value = row
  Object.assign(form, {
    username: row?.username ?? '',
    displayName: row?.displayName ?? '',
    password: '',
    roleIds: [...(row?.roleIds ?? [])],
    status: row?.status ?? 'active',
  })
  error.value = ''
  open.value = true
  void loadRoles()
}
async function save() {
  error.value = ''
  if (!form.username || !form.displayName || !form.roleIds.length) {
    error.value = '请填写账号、显示名并选择角色'
    return
  }
  if (!editing.value && form.password.length < 12) {
    error.value = '初始密码至少需要 12 个字符'
    return
  }
  if (
    !editing.value &&
    session.user?.dataScope === 'roles' &&
    !form.roleIds.some((id) => session.user?.dataScopeRoleIds.includes(id))
  ) {
    error.value = '新用户至少需要属于一个可管理的角色'
    return
  }
  saving.value = true
  try {
    if (editing.value)
      await session.request(`/users/${editing.value.id}`, {
        method: 'PATCH',
        body: {
          version: editing.value.version,
          username: form.username,
          displayName: form.displayName,
          roleIds: form.roleIds,
          status: form.status,
        },
      })
    else await session.request('/users', { method: 'POST', body: { ...form } })
    const own = editing.value?.id === session.user?.id
    open.value = false
    form.password = ''
    ElMessage.success('用户已保存')
    if (own) await session.syncProfile()
    await load()
  } catch (failure) {
    error.value = errorText(failure)
  } finally {
    saving.value = false
  }
}
async function remove(row: UserRecord) {
  try {
    await ElMessageBox.confirm(`删除“${row.displayName}”后，该用户将无法继续登录。`, '删除用户', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    })
    await session.request(`/users/${row.id}?version=${row.version}`, { method: 'DELETE' })
    ElMessage.success('用户已删除')
    if (row.id === session.user?.id) await session.logout()
    else await load()
  } catch (failure) {
    if (failure !== 'cancel' && failure !== 'close') notifyError(failure)
  }
}
async function reset(row: UserRecord) {
  try {
    const value = await ElMessageBox.prompt(
      `为“${row.displayName}”设置新密码，现有会话将退出。`,
      '重置密码',
      {
        inputType: 'password',
        inputPlaceholder: '至少 12 个字符',
        inputValidator: (value) => (!!value && value.length >= 12) || '密码至少需要 12 个字符',
        confirmButtonText: '重置密码',
        cancelButtonText: '取消',
      },
    )
    await session.request(`/users/${row.id}/password`, {
      method: 'POST',
      body: { version: row.version, password: value.value },
    })
    ElMessage.success('密码已重置')
    if (row.id === session.user?.id) await session.passwordChanged()
    else await load()
  } catch (failure) {
    if (failure !== 'cancel' && failure !== 'close') notifyError(failure)
  }
}
onMounted(() => {
  void load()
  void loadRoles()
})
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">MEMBERS</div>
      <h1>用户管理</h1>
      <p>维护工作空间成员、角色和访问状态。</p>
    </div>
    <el-button v-if="canCreate" type="primary" :icon="Plus" @click="edit(null)">新增用户</el-button>
  </div>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <el-input
        v-model="search"
        placeholder="按账号前缀搜索"
        clearable
        :prefix-icon="Search"
        class="search-input"
        @keyup.enter="queryUsers"
        @clear="queryUsers"
      /><el-button @click="queryUsers">查询</el-button>
      <div class="toolbar-spacer" />
      <span class="subtle">共 {{ total }} 位用户</span
      ><el-button :icon="Refresh" text @click="load">刷新</el-button>
    </div>
    <el-table v-loading="loading" :data="rows" row-key="id"
      ><el-table-column label="用户" min-width="200"
        ><template #default="{ row }"
          ><div class="member-cell">
            <span class="table-avatar">{{ row.displayName.slice(0, 1) }}</span>
            <div>
              <strong>{{ row.displayName }}</strong
              ><small>{{ row.username }}</small>
            </div>
          </div></template
        ></el-table-column
      ><el-table-column label="角色" min-width="180"
        ><template #default="{ row }"
          ><el-tag v-for="role in row.roles" :key="role.id" type="info" class="role-tag">{{
            role.name
          }}</el-tag></template
        ></el-table-column
      ><el-table-column label="状态" width="110"
        ><template #default="{ row }"
          ><span class="status-label" :class="row.status"
            ><i />{{ row.status === 'active' ? '启用' : '停用' }}</span
          ></template
        ></el-table-column
      ><el-table-column label="创建时间" min-width="180"
        ><template #default="{ row }">{{ dateText(row.createdAt) }}</template></el-table-column
      ><el-table-column label="操作" width="230" fixed="right"
        ><template #default="{ row }"
          ><el-button
            v-if="session.can('users.write')"
            link
            type="primary"
            @click="edit(row as UserRecord)"
            >编辑</el-button
          ><el-button
            v-if="session.can('users.resetPassword')"
            link
            @click="reset(row as UserRecord)"
            >重置密码</el-button
          ><el-button
            v-if="session.can('users.write')"
            link
            type="danger"
            @click="remove(row as UserRecord)"
            >删除</el-button
          ></template
        ></el-table-column
      ></el-table
    >
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
    :title="editing ? '编辑用户' : '新增用户'"
    width="540px"
    :close-on-click-modal="false"
    ><el-form label-position="top" class="editor-form"
      ><div class="form-two">
        <el-form-item label="账号"
          ><el-input
            v-model="form.username"
            maxlength="64"
            placeholder="字母、数字、中文或 ._-" /></el-form-item
        ><el-form-item label="显示名"
          ><el-input v-model="form.displayName" maxlength="80"
        /></el-form-item>
      </div>
      <el-form-item v-if="!editing" label="初始密码"
        ><el-input
          v-model="form.password"
          type="password"
          show-password
          placeholder="至少 12 个字符"
          autocomplete="new-password" /></el-form-item
      ><el-form-item label="角色"
        ><el-select
          v-model="form.roleIds"
          multiple
          filterable
          remote
          :remote-method="loadRoles"
          :loading="rolesLoading"
          placeholder="搜索角色标识"
          :disabled="!session.can('roles.assign')"
          ><el-option
            v-for="role in roleOptions"
            :key="role.id"
            :label="role.name"
            :value="role.id" /></el-select></el-form-item
      ><el-form-item label="访问状态"
        ><el-radio-group v-model="form.status"
          ><el-radio-button value="active">启用</el-radio-button
          ><el-radio-button value="disabled">停用</el-radio-button></el-radio-group
        ></el-form-item
      ><el-alert v-if="error" :title="error" type="error" :closable="false" /></el-form
    ><template #footer
      ><el-button @click="open = false">取消</el-button
      ><el-button type="primary" :loading="saving" @click="save">保存用户</el-button></template
    ></el-dialog
  >
</template>
