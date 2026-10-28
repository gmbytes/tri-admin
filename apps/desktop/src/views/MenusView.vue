<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Plus, Refresh } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import type { MenuRecord, Permission } from '../core/types'
import { filterPermissions } from '../core/navigation'
import { errorText, notifyError } from '../ui'
const session = useSession(),
  rows = ref<MenuRecord[]>([]),
  permissions = ref<Permission[]>([]),
  loading = ref(false),
  saving = ref(false),
  open = ref(false),
  error = ref(''),
  editing = ref<MenuRecord | null>(null)
const form = reactive({
  label: '',
  path: '',
  viewKey: null as string | null,
  icon: null as string | null,
  permission: null as string | null,
  parentId: null as string | null,
  sort: 0,
  enabled: true,
})
const views = [
  { value: 'dashboard', label: '工作台' },
  { value: 'files', label: '文件管理' },
  { value: 'users', label: '用户管理' },
  { value: 'roles', label: '角色管理' },
  { value: 'menus', label: '菜单管理' },
  { value: 'permissions', label: '权限目录' },
  { value: 'audit', label: '日志中心' },
]
function tree(items: MenuRecord[], parent: string | null = null): MenuRecord[] {
  return items
    .filter((item) => item.parentId === parent)
    .sort((a, b) => a.sort - b.sort || a.id.localeCompare(b.id))
    .map((item) => ({ ...item, children: tree(items, item.id) }))
}
const menuTree = computed(() => tree(rows.value))
const parentOptions = computed(() => {
  if (!editing.value) return menuTree.value
  const excluded = new Set([editing.value.id])
  let changed = true
  while (changed) {
    changed = false
    for (const row of rows.value)
      if (row.parentId && excluded.has(row.parentId) && !excluded.has(row.id)) {
        excluded.add(row.id)
        changed = true
      }
  }
  return tree(rows.value.filter((row) => !excluded.has(row.id)))
})
async function load() {
  loading.value = true
  try {
    rows.value = (await session.request<MenuRecord[]>('/menus')).data
  } catch (failure) {
    notifyError(failure)
  } finally {
    loading.value = false
  }
}
function edit(row: MenuRecord | null) {
  editing.value = row
  Object.assign(form, {
    label: row?.label ?? '',
    path: row?.path ?? '',
    viewKey: row?.viewKey ?? null,
    icon: row?.icon ?? null,
    permission: row?.permission ?? null,
    parentId: row?.parentId ?? null,
    sort: row?.sort ?? 0,
    enabled: row?.enabled ?? true,
  })
  error.value = ''
  open.value = true
}
async function save() {
  error.value = ''
  if (!form.label || !form.path) {
    error.value = '请填写菜单名和路径'
    return
  }
  saving.value = true
  try {
    await session.request(editing.value ? `/menus/${editing.value.id}` : '/menus', {
      method: editing.value ? 'PATCH' : 'POST',
      body: {
        ...form,
        viewKey: form.viewKey || null,
        icon: form.icon || null,
        permission: form.permission || null,
        parentId: form.parentId || null,
        sort: form.sort ?? 0,
        ...(editing.value ? { version: editing.value.version } : {}),
      },
    })
    open.value = false
    ElMessage.success('菜单已保存')
    await load()
    await session.loadNavigation()
  } catch (failure) {
    error.value = errorText(failure)
  } finally {
    saving.value = false
  }
}
async function remove(row: MenuRecord) {
  try {
    await ElMessageBox.confirm(`确认删除菜单“${row.label}”？请先移动或删除其子菜单。`, '删除菜单', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    })
    await session.request(`/menus/${row.id}?version=${row.version}`, { method: 'DELETE' })
    ElMessage.success('菜单已删除')
    await load()
    await session.loadNavigation()
  } catch (failure) {
    if (failure !== 'cancel' && failure !== 'close') notifyError(failure)
  }
}
onMounted(() => {
  void load()
  if (session.can('permissions.read'))
    void session
      .request<Permission[]>('/permissions')
      .then((value) => {
        permissions.value = filterPermissions(value.data)
      })
      .catch(notifyError)
})
</script>
<template>
  <div class="page-heading">
    <div>
      <h1>菜单管理</h1>
      <p>组织页面入口，按权限呈现清晰导航。</p>
    </div>
    <el-button v-if="session.can('menus.write')" type="primary" :icon="Plus" @click="edit(null)"
      >新增菜单</el-button
    >
  </div>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <h3>导航结构</h3>
      <div class="toolbar-spacer" />
      <span class="subtle">{{ rows.length }} 个菜单</span
      ><el-button text :icon="Refresh" @click="load">刷新</el-button>
    </div>
    <el-table v-loading="loading" :data="menuTree" row-key="id" default-expand-all
      ><el-table-column prop="label" label="菜单名" min-width="220" /><el-table-column
        prop="path"
        label="路径"
        min-width="180"
      /><el-table-column label="页面" min-width="130"
        ><template #default="{ row }">{{
          views.find((view) => view.value === row.viewKey)?.label ?? '目录'
        }}</template></el-table-column
      ><el-table-column prop="sort" label="排序" width="80" /><el-table-column
        label="状态"
        width="100"
        ><template #default="{ row }"
          ><el-tag :type="row.enabled ? 'success' : 'info'">{{
            row.enabled ? '启用' : '停用'
          }}</el-tag></template
        ></el-table-column
      ><el-table-column label="操作" width="150" fixed="right"
        ><template #default="{ row }"
          ><template v-if="session.can('menus.write')"
            ><el-button link type="primary" @click="edit(row as MenuRecord)">编辑</el-button
            ><el-button link type="danger" @click="remove(row as MenuRecord)"
              >删除</el-button
            ></template
          ></template
        ></el-table-column
      ></el-table
    >
  </section>
  <el-dialog
    v-model="open"
    :title="editing ? '编辑菜单' : '新增菜单'"
    width="620px"
    :close-on-click-modal="false"
    ><el-form label-position="top"
      ><div class="form-two">
        <el-form-item label="菜单名"><el-input v-model="form.label" maxlength="80" /></el-form-item
        ><el-form-item label="路径"
          ><el-input v-model="form.path" placeholder="例如 /members"
        /></el-form-item>
      </div>
      <el-form-item label="父级菜单"
        ><el-tree-select
          v-model="form.parentId"
          :data="parentOptions"
          node-key="id"
          :props="{ label: 'label', children: 'children' }"
          check-strictly
          clearable
          placeholder="留空为根菜单"
      /></el-form-item>
      <div class="form-two">
        <el-form-item label="页面"
          ><el-select v-model="form.viewKey" clearable placeholder="留空作为目录"
            ><el-option
              v-for="view in views"
              :key="view.value"
              :label="view.label"
              :value="view.value" /></el-select></el-form-item
        ><el-form-item label="所需权限"
          ><el-select
            v-model="form.permission"
            clearable
            filterable
            allow-create
            placeholder="无额外权限"
            ><el-option
              v-for="permission in permissions"
              :key="permission.code"
              :label="permission.label"
              :value="permission.code" /></el-select
        ></el-form-item>
      </div>
      <div class="form-two">
        <el-form-item label="排序"
          ><el-input-number v-model="form.sort" :min="-10000" :max="10000" /></el-form-item
        ><el-form-item label="启用"><el-switch v-model="form.enabled" /></el-form-item>
      </div>
      <el-alert v-if="error" :title="error" type="error" :closable="false" /></el-form
    ><template #footer
      ><el-button @click="open = false">取消</el-button
      ><el-button type="primary" :loading="saving" @click="save">保存菜单</el-button></template
    ></el-dialog
  >
</template>
