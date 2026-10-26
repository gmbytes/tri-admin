<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { useCredentialPage } from '../composables/useCredentialPage'
import {
  parseApplicationUser,
  parsePage,
  type Application,
  type ApplicationUser,
  type OpenScope,
} from '../core/open-access'
import { errorText } from '../ui'
const props = defineProps<{ application: Application }>(),
  gate = useCredentialPage(),
  session = gate.session
const rows = ref<ApplicationUser[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  loading = ref(false),
  saving = ref(false),
  error = ref(''),
  formError = ref(''),
  open = ref(false),
  editing = ref<ApplicationUser | null>(null),
  mode = ref<'create' | 'edit' | 'password'>('create'),
  uncertain = ref(false)
const form = reactive({
  username: '',
  displayName: '',
  password: '',
  scopes: [] as OpenScope[],
  status: 'active' as 'active' | 'disabled',
})
const canWrite = computed(
  () =>
    session.can('applicationUsers.write') &&
    session.can('applicationUsers.read') &&
    session.can('applications.read'),
)
const available = computed(() =>
  props.application.allowedScopes.filter((scope) => scope.startsWith('open.profile.')),
)
const labels = {
  'open.profile.read': '读取本人资料',
  'open.profile.write': '修改本人资料',
} as Record<string, string>
const title = computed(() =>
  mode.value === 'password' ? '重置应用用户密码' : editing.value ? '编辑应用用户' : '新增应用用户',
)
let sequence = 0
gate.onReset(() => {
  sequence++
  rows.value = []
  open.value = false
  error.value = ''
  formError.value = ''
  form.password = ''
  loading.value = false
  saving.value = false
  ElMessageBox.close()
})
onBeforeUnmount(() => {
  form.password = ''
})
async function load() {
  const expected = gate.ticket(),
    request = ++sequence
  loading.value = true
  error.value = ''
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
      search: search.value,
    })
    const result = parsePage(
      await gate.request(`/applications/${props.application.id}/users?${query}`),
      parseApplicationUser,
    )
    if (!gate.current(expected) || request !== sequence) return
    if (result.data.some((row) => row.applicationId !== props.application.id))
      throw new Error('用户所属应用不匹配')
    rows.value = result.data
    total.value = result.meta.total
    uncertain.value = false
  } catch (failure) {
    if (gate.current(expected) && request === sequence) error.value = errorText(failure)
  } finally {
    if (gate.current(expected) && request === sequence) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
function edit(row: ApplicationUser | null, password = false) {
  editing.value = row
  mode.value = password ? 'password' : row ? 'edit' : 'create'
  formError.value = ''
  uncertain.value = false
  Object.assign(
    form,
    row
      ? {
          username: row.username,
          displayName: row.displayName,
          password: '',
          scopes: [...row.scopes],
          status: row.status,
        }
      : {
          username: '',
          displayName: '',
          password: '',
          scopes: available.value.slice(0, 1),
          status: 'active',
        },
  )
  open.value = true
}
async function save() {
  const expected = gate.ticket()
  saving.value = true
  formError.value = ''
  let sent = false
  try {
    const old = editing.value,
      base = `/applications/${props.application.id}/users`
    if (mode.value === 'password') {
      if (!old) throw new Error('用户无效')
      if ([...form.password].length < 12) throw new Error('密码至少需要 12 个字符')
      sent = true
      await gate.request(`${base}/${old.id}/password`, {
        method: 'POST',
        body: { version: old.version, password: form.password },
      })
    } else {
      if (!form.username.trim() || !form.displayName.trim() || !form.scopes.length)
        throw new Error('请填写账号、显示名和资料 Scope')
      const body: Record<string, unknown> = old
        ? { version: old.version }
        : {
            username: form.username.trim(),
            displayName: form.displayName.trim(),
            password: form.password,
            scopes: [...form.scopes],
            status: form.status,
          }
      if (old) {
        if (form.username.trim() !== old.username) body.username = form.username.trim()
        if (form.displayName.trim() !== old.displayName) body.displayName = form.displayName.trim()
        if ([...form.scopes].sort().join() !== [...old.scopes].sort().join())
          body.scopes = [...form.scopes]
        if (form.status !== old.status) body.status = form.status
        if (Object.keys(body).length === 1) {
          open.value = false
          return
        }
      } else if ([...form.password].length < 12) throw new Error('密码至少需要 12 个字符')
      sent = true
      const result = parseApplicationUser(
        (
          await gate.request(old ? `${base}/${old.id}` : base, {
            method: old ? 'PATCH' : 'POST',
            body,
          })
        ).data,
      )
      if (result.applicationId !== props.application.id) throw new Error('用户所属应用不匹配')
    }
    if (gate.current(expected)) {
      open.value = false
      ElMessage.success(
        mode.value === 'password' ? '密码已重置，旧用户会话已撤销' : '应用用户已保存',
      )
      await load()
    }
  } catch (failure) {
    if (gate.current(expected)) {
      formError.value = errorText(failure)
      if (sent) {
        uncertain.value = true
        formError.value += '；请关闭表单并刷新用户，核对账号与版本后再操作。不会自动重复提交。'
      }
    }
  } finally {
    form.password = ''
    if (gate.current(expected)) saving.value = false
  }
}
async function remove(row: ApplicationUser) {
  const expected = gate.ticket()
  try {
    await ElMessageBox.confirm(`删除“${row.username}”会撤销其全部会话。`, '删除应用用户', {
      confirmButtonText: '删除',
      cancelButtonText: '取消',
      type: 'warning',
    })
    if (!gate.current(expected)) return
    await gate.request(
      `/applications/${props.application.id}/users/${row.id}?version=${row.version}`,
      { method: 'DELETE' },
    )
    if (gate.current(expected)) await load()
  } catch (failure) {
    if (gate.current(expected) && failure !== 'cancel' && failure !== 'close')
      error.value = errorText(failure)
  }
}
onMounted(() => void load())
</script>
<template>
  <p>
    <strong>{{ application.name }}</strong> · <code>{{ application.id }}</code>
  </p>
  <div class="table-toolbar">
    <el-input
      v-model="search"
      placeholder="按用户账号前缀搜索"
      clearable
      class="search-input"
      @keyup.enter="query"
    /><el-button @click="query">查询用户</el-button>
    <div class="toolbar-spacer" />
    <el-button @click="load">刷新用户</el-button
    ><el-button v-if="canWrite" type="primary" :disabled="!available.length" @click="edit(null)"
      >新增应用用户</el-button
    >
  </div>
  <el-alert v-if="error" :title="error" type="error" :closable="false" /><el-table
    :data="rows"
    v-loading="loading"
    row-key="id"
    ><el-table-column label="用户" min-width="220"
      ><template #default="{ row }"
        ><strong>{{ row.displayName }}</strong
        ><small class="table-secondary">{{ row.username }}</small
        ><small class="table-secondary">{{ row.id }}</small></template
      ></el-table-column
    ><el-table-column label="Scope" min-width="200"
      ><template #default="{ row }">{{ row.scopes.join(' · ') }}</template></el-table-column
    ><el-table-column label="状态 / 版本" width="140"
      ><template #default="{ row }"
        ><el-tag :type="row.status === 'active' ? 'success' : 'info'">{{
          row.status === 'active' ? '有效' : '已禁用'
        }}</el-tag
        ><small class="table-secondary">版本 {{ row.version }}</small></template
      ></el-table-column
    ><el-table-column v-if="canWrite" label="操作" width="210"
      ><template #default="{ row }"
        ><el-button link @click="edit(row as ApplicationUser)">编辑用户</el-button
        ><el-button link @click="edit(row as ApplicationUser, true)">重置密码</el-button
        ><el-button link type="danger" @click="remove(row as ApplicationUser)"
          >删除用户</el-button
        ></template
      ></el-table-column
    ></el-table
  >
  <div class="table-footer">
    <span class="subtle">共 {{ total }} 个应用用户</span
    ><el-pagination
      v-model:current-page="page"
      v-model:page-size="pageSize"
      :total="total"
      :page-sizes="[20, 50, 100]"
      layout="sizes,prev,pager,next"
      @current-change="load"
      @size-change="query"
    />
  </div>
  <el-dialog
    v-model="open"
    :title="title"
    width="min(600px,94vw)"
    append-to-body
    :close-on-click-modal="false"
    :show-close="!saving"
    :close-on-press-escape="!saving"
    destroy-on-close
    @closed="form.password = ''"
    ><el-form label-position="top" :disabled="saving" @submit.prevent="save"
      ><el-alert v-if="formError" :title="formError" type="error" :closable="false" />
      <p v-if="editing" class="subtle">按用户版本 {{ editing.version }} 保存；账号属于当前应用。</p>
      <template v-if="mode !== 'password'"
        ><el-form-item label="用户账号"
          ><el-input v-model="form.username" aria-label="用户账号" maxlength="64" /></el-form-item
        ><el-form-item label="用户显示名"
          ><el-input
            v-model="form.displayName"
            aria-label="用户显示名"
            maxlength="80" /></el-form-item
        ><el-form-item label="资料 Scope"
          ><el-checkbox-group v-model="form.scopes"
            ><el-checkbox v-for="scope in available" :key="scope" :value="scope">{{
              labels[scope]
            }}</el-checkbox></el-checkbox-group
          ></el-form-item
        ><el-form-item label="用户状态"
          ><el-select v-model="form.status" aria-label="用户状态"
            ><el-option label="有效" value="active" /><el-option
              label="已禁用"
              value="disabled" /></el-select></el-form-item></template
      ><el-form-item
        v-if="mode === 'create' || mode === 'password'"
        :label="mode === 'password' ? '新密码' : '初始密码'"
        ><el-input
          v-model="form.password"
          :aria-label="mode === 'password' ? '新密码' : '初始密码'"
          type="password"
          show-password
          autocomplete="new-password"
        />
        <p class="tiny subtle">
          至少 12 个字符；不在本机保存。提交后清空，响应丢失时先核对用户状态。
        </p></el-form-item
      ></el-form
    ><template #footer
      ><el-button :disabled="saving" @click="open = false">取消</el-button
      ><el-button type="primary" :loading="saving" :disabled="saving || uncertain" @click="save">{{
        mode === 'password' ? '重置密码' : '保存应用用户'
      }}</el-button></template
    ></el-dialog
  >
</template>
