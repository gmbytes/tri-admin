<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Plus, Refresh } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import type { DataScope, Permission, RoleRecord } from '../core/types'
import { errorText, notifyError, scopeText } from '../ui'
const session = useSession(),
  rows = ref<RoleRecord[]>([]),
  permissions = ref<Permission[]>([]),
  loading = ref(false),
  saving = ref(false),
  open = ref(false),
  error = ref(''),
  editing = ref<RoleRecord | null>(null),
  page = ref(1),
  total = ref(0)
const form = reactive({
  name: '',
  slug: '',
  permissions: [] as string[],
  dataScope: 'self' as DataScope,
  dataScopeRoleIds: [] as string[],
})
const scopeChoices = ref<Array<{ id: string; name: string; slug?: string }>>([]),
  scopeLoading = ref(false)
let scopeQuery = 0
const scopeOptions = computed(() => {
  const options = new Map<string, { id: string; name: string; slug?: string }>()
  for (const item of [...(editing.value?.dataScopeRoles ?? []), ...scopeChoices.value])
    options.set(item.id, item)
  return [...options.values()].filter(
    (item) => session.user?.dataScope === 'all' || session.user?.dataScopeRoleIds.includes(item.id),
  )
})
async function loadScopeChoices(search = '') {
  const query = ++scopeQuery
  scopeLoading.value = true
  try {
    const response = await session.request<RoleRecord[]>(
      `/roles?pageSize=100&sort=slug&search=${encodeURIComponent(search)}`,
    )
    if (query === scopeQuery) scopeChoices.value = response.data
  } catch (failure) {
    if (query === scopeQuery) notifyError(failure)
  } finally {
    if (query === scopeQuery) scopeLoading.value = false
  }
}
const groups = computed(() => {
  const result = new Map<string, Permission[]>()
  for (const item of permissions.value.filter((item) => session.can(item.code))) {
    const group = result.get(item.group) ?? []
    group.push(item)
    result.set(item.group, group)
  }
  return [...result.entries()]
})
async function load() {
  loading.value = true
  try {
    const response = await session.request<RoleRecord[]>(`/roles?page=${page.value}&pageSize=20`)
    rows.value = response.data
    total.value = response.meta?.total ?? 0
  } catch (failure) {
    notifyError(failure)
  } finally {
    loading.value = false
  }
}
async function loadPermissions() {
  if (session.can('permissions.read'))
    permissions.value = (await session.request<Permission[]>('/permissions')).data
  else
    permissions.value = (session.user?.permissions ?? [])
      .filter((code) => code !== '*')
      .map((code) => ({ code, label: code, group: code.split('.')[0] ?? 'other' }))
}
function edit(row: RoleRecord | null) {
  editing.value = row
  Object.assign(form, {
    name: row?.name ?? '',
    slug: row?.slug ?? '',
    permissions: [...(row?.permissions ?? [])],
    dataScope: row?.dataScope ?? 'self',
    dataScopeRoleIds: [...(row?.dataScopeRoleIds ?? [])],
  })
  error.value = ''
  open.value = true
  void loadScopeChoices()
}
async function save() {
  error.value = ''
  if (!form.name || !form.slug) {
    error.value = '请填写角色名和标识'
    return
  }
  if (form.dataScope === 'roles' && form.dataScopeRoleIds.length === 0) {
    error.value = '请选择范围角色'
    return
  }
  saving.value = true
  try {
    await session.request(editing.value ? `/roles/${editing.value.id}` : '/roles', {
      method: editing.value ? 'PATCH' : 'POST',
      body: {
        ...form,
        dataScopeRoleIds: form.dataScope === 'roles' ? form.dataScopeRoleIds : [],
        ...(editing.value ? { version: editing.value.version } : {}),
      },
    })
    open.value = false
    ElMessage.success('角色已保存')
    await session.syncProfile()
    await load()
  } catch (failure) {
    error.value = errorText(failure)
  } finally {
    saving.value = false
  }
}
async function remove(row: RoleRecord) {
  try {
    await ElMessageBox.confirm(
      `确认删除角色“${row.name}”？仍被用户使用的角色不能删除。`,
      '删除角色',
      { type: 'warning', confirmButtonText: '删除', cancelButtonText: '取消' },
    )
    await session.request(`/roles/${row.id}?version=${row.version}`, { method: 'DELETE' })
    ElMessage.success('角色已删除')
    await load()
  } catch (failure) {
    if (failure !== 'cancel' && failure !== 'close') notifyError(failure)
  }
}
onMounted(() => {
  void load()
  void loadPermissions().catch(notifyError)
})
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">ACCESS CONTROL</div>
      <h1>角色管理</h1>
      <p>按职责组织操作权限与数据范围。</p>
    </div>
    <el-button v-if="session.can('roles.write')" type="primary" :icon="Plus" @click="edit(null)"
      >新增角色</el-button
    >
  </div>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <h3>角色列表</h3>
      <div class="toolbar-spacer" />
      <span class="subtle">共 {{ total }} 个角色</span
      ><el-button text :icon="Refresh" @click="load">刷新</el-button>
    </div>
    <el-table v-loading="loading" :data="rows" row-key="id"
      ><el-table-column label="角色" min-width="220"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><el-tag v-if="row.protected" size="small" type="warning" class="role-tag"
            >系统角色</el-tag
          ><small class="table-secondary">{{ row.slug }}</small></template
        ></el-table-column
      ><el-table-column label="操作权限" min-width="180"
        ><template #default="{ row }"
          ><el-tag type="info">{{
            row.permissions.includes('*') ? '全部操作' : row.permissions.length + ' 项权限'
          }}</el-tag></template
        ></el-table-column
      ><el-table-column label="数据范围" min-width="160"
        ><template #default="{ row }"
          >{{ scopeText(row.dataScope)
          }}<small v-if="row.dataScope === 'roles'" class="table-secondary">{{
            row.dataScopeRoles.map((role: { name: string }) => role.name).join('、')
          }}</small></template
        ></el-table-column
      ><el-table-column label="操作" width="170" fixed="right"
        ><template #default="{ row }"
          ><template v-if="session.can('roles.write') && !row.protected"
            ><el-button link type="primary" @click="edit(row as RoleRecord)">编辑</el-button
            ><el-button link type="danger" @click="remove(row as RoleRecord)"
              >删除</el-button
            ></template
          ><span v-else class="subtle">{{ row.protected ? '受保护' : '只读' }}</span></template
        ></el-table-column
      ></el-table
    >
    <div class="pagination">
      <el-pagination
        v-model:current-page="page"
        layout="total,prev,pager,next"
        :page-size="20"
        :total="total"
        @change="load"
      />
    </div>
  </section>
  <el-dialog
    v-model="open"
    :title="editing ? '编辑角色' : '新增角色'"
    width="720px"
    :close-on-click-modal="false"
    ><el-form label-position="top"
      ><div class="form-two">
        <el-form-item label="角色名"><el-input v-model="form.name" maxlength="80" /></el-form-item
        ><el-form-item label="角色标识"
          ><el-input v-model="form.slug" placeholder="例如 operator" maxlength="64"
        /></el-form-item>
      </div>
      <el-form-item label="数据范围"
        ><el-radio-group v-model="form.dataScope"
          ><el-radio-button value="self">自身数据</el-radio-button
          ><el-radio-button value="all" :disabled="session.user?.dataScope !== 'all'"
            >全部数据</el-radio-button
          ><el-radio-button value="roles" :disabled="session.user?.dataScope === 'self'"
            >指定角色</el-radio-button
          ></el-radio-group
        ></el-form-item
      >
      <el-form-item v-if="form.dataScope === 'roles'" label="范围角色">
        <el-select
          v-model="form.dataScopeRoleIds"
          multiple
          filterable
          remote
          :remote-method="loadScopeChoices"
          :loading="scopeLoading"
          :multiple-limit="32"
          placeholder="搜索角色标识，最多选择 32 个"
        >
          <el-option
            v-for="item in scopeOptions"
            :key="item.id"
            :value="item.id"
            :label="item.slug ? `${item.name} · ${item.slug}` : item.name"
          />
        </el-select>
      </el-form-item>
      <div class="permission-heading">
        <strong>操作权限</strong><span class="subtle">已选 {{ form.permissions.length }} 项</span>
      </div>
      <div class="permission-editor">
        <section v-for="[name, items] in groups" :key="name">
          <h4>{{ name }}</h4>
          <el-checkbox-group v-model="form.permissions"
            ><el-checkbox v-for="item in items" :key="item.code" :value="item.code">{{
              item.label
            }}</el-checkbox></el-checkbox-group
          >
        </section>
      </div>
      <el-alert v-if="error" :title="error" type="error" :closable="false" /></el-form
    ><template #footer
      ><el-button @click="open = false">取消</el-button
      ><el-button type="primary" :loading="saving" @click="save">保存角色</el-button></template
    ></el-dialog
  >
</template>
