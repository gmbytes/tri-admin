<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { useCredentialPage } from '../composables/useCredentialPage'
import {
  openScopes,
  parseApplication,
  parseIssue,
  parsePage,
  parseRotation,
  policyFromForm,
  type Application,
  type OpenScope,
  type CredentialOperation,
} from '../core/open-access'
import { errorText } from '../ui'
import AccessPolicyFields from '../components/AccessPolicyFields.vue'
import SecretResultDialog from '../components/SecretResultDialog.vue'
import CredentialRecoveryPanel from '../components/CredentialRecoveryPanel.vue'
import ApplicationUsersPanel from '../components/ApplicationUsersPanel.vue'
import ApplicationCallsView from './ApplicationCallsView.vue'
const gate = useCredentialPage(),
  session = gate.session
const rows = ref<Application[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  loading = ref(false),
  error = ref(''),
  open = ref(false),
  saving = ref(false),
  formError = ref(''),
  editing = ref<Application | null>(null),
  userApp = ref<Application | null>(null),
  callsApp = ref<Application | null>(null)
const form = reactive({
  name: '',
  code: '',
  enabled: true,
  scopes: [] as OpenScope[],
  ips: '',
  minute: 600,
  daily: 100000,
})
const scopeLabels: Record<OpenScope, string> = {
  'open.profile.read': '读取本人资料',
  'open.profile.write': '修改本人资料',
  'open.users.read': '读取应用用户',
  'open.users.write': '管理应用用户',
}
const grant = (scope: OpenScope) =>
  session.can(scope.endsWith('.write') ? 'applicationUsers.write' : 'applicationUsers.read')
const canCreate = computed(() => session.can('applications.write') && openScopes.some(grant))
const pending = computed(() => gate.pending.value.filter((row) => row.kind.startsWith('app.')))
const blocked = computed(
  () => saving.value || !!gate.recoveryError.value || pending.value.length > 0,
)
const canPolicy = computed(() => !editing.value || editing.value.allowedScopes.every(grant))
let sequence = 0
gate.onReset(() => {
  sequence++
  rows.value = []
  total.value = 0
  open.value = false
  editing.value = null
  userApp.value = null
  callsApp.value = null
  error.value = ''
  formError.value = ''
  saving.value = false
  loading.value = false
  ElMessageBox.close()
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
    const result = parsePage(await gate.request(`/applications?${query}`), parseApplication)
    if (!gate.current(expected) || request !== sequence) return
    rows.value = result.data
    total.value = result.meta.total
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
function edit(row: Application | null) {
  editing.value = row
  formError.value = ''
  Object.assign(
    form,
    row
      ? {
          name: row.name,
          code: row.code,
          enabled: row.enabled,
          scopes: [...row.allowedScopes],
          ips: row.allowedIps.join('\n'),
          minute: row.minuteLimit,
          daily: row.dailyQuota,
        }
      : {
          name: '',
          code: '',
          enabled: true,
          scopes: openScopes.filter(grant).slice(0, 1),
          ips: '',
          minute: 600,
          daily: 100000,
        },
  )
  open.value = true
}
async function reloadEdit() {
  if (!editing.value) return
  const expected = gate.ticket()
  try {
    const row = parseApplication((await gate.request(`/applications/${editing.value.id}`)).data)
    if (gate.current(expected)) edit(row)
  } catch (failure) {
    if (gate.current(expected)) formError.value = errorText(failure)
  }
}
async function save() {
  if (!form.name.trim() || !form.scopes.length) {
    formError.value = '请填写应用名称并选择 Scope'
    return
  }
  const expected = gate.ticket()
  let operation: string | null = null
  saving.value = true
  formError.value = ''
  try {
    const policy = policyFromForm(form.ips, form.minute, form.daily),
      old = editing.value
    if (old) {
      const body: Record<string, unknown> = { version: old.version }
      if (form.name.trim() !== old.name) body.name = form.name.trim()
      if (form.enabled !== old.enabled) body.enabled = form.enabled
      if ([...form.scopes].sort().join() !== [...old.allowedScopes].sort().join())
        body.allowedScopes = [...form.scopes]
      if (policy.allowedIps.join() !== old.allowedIps.join()) body.allowedIps = policy.allowedIps
      if (policy.minuteLimit !== old.minuteLimit) body.minuteLimit = policy.minuteLimit
      if (policy.dailyQuota !== old.dailyQuota) body.dailyQuota = policy.dailyQuota
      if (Object.keys(body).length === 1) {
        open.value = false
        return
      }
      parseApplication(
        (await gate.request(`/applications/${old.id}`, { method: 'PATCH', body })).data,
      )
    } else {
      const marker = gate.begin('app.create')
      operation = marker.requestId
      const result = parseIssue(
        (
          await gate.request('/applications', {
            method: 'POST',
            body: {
              requestId: marker.requestId,
              name: form.name.trim(),
              code: form.code.trim(),
              enabled: form.enabled,
              allowedScopes: [...form.scopes],
              ...policy,
            },
          })
        ).data,
        'application',
      )
      if (result.entity.requestId !== marker.requestId) throw new Error('应用结果与请求不匹配')
      if (!gate.current(expected)) return
      gate.confirmed(marker.requestId, expected)
      if (result.secret)
        gate.secret.value = { kind: 'application', id: result.entity.id, value: result.secret }
      else ElMessage.warning('应用创建已确认，原秘密不能再次展示')
    }
    if (gate.current(expected)) {
      open.value = false
      await load()
    }
  } catch (failure) {
    if (gate.current(expected)) {
      if (operation) gate.rejected(operation, expected, failure)
      formError.value = errorText(failure)
      gate.readPending()
    }
  } finally {
    if (gate.current(expected)) saving.value = false
  }
}
async function rotate(row: Application) {
  const expected = gate.ticket()
  let operation: string | null = null
  try {
    await ElMessageBox.confirm(
      `轮换“${row.name}”将立即使旧应用服务密钥和访问会话失效。应用用户会话不受影响。`,
      '轮换应用密钥',
      { confirmButtonText: '轮换', cancelButtonText: '取消', type: 'warning' },
    )
    if (!gate.current(expected) || blocked.value) return
    saving.value = true
    const marker = gate.begin('app.rotate', row.id, row.version)
    operation = marker.requestId
    const result = parseRotation(
      (
        await gate.request(`/applications/${row.id}/secret`, {
          method: 'POST',
          body: { requestId: marker.requestId, version: row.version },
        })
      ).data,
    )
    if (
      result.entity.id !== row.id ||
      result.receipt.requestId !== marker.requestId ||
      result.receipt.version !== row.version + 1
    )
      throw new Error('轮换结果与请求不匹配')
    if (!gate.current(expected)) return
    gate.confirmed(marker.requestId, expected)
    if (result.secret) gate.secret.value = { kind: 'application', id: row.id, value: result.secret }
    else ElMessage.warning('轮换已确认，原秘密不能再次展示')
    await load()
  } catch (failure) {
    if (gate.current(expected) && failure !== 'cancel' && failure !== 'close') {
      if (operation) gate.rejected(operation, expected, failure)
      error.value = errorText(failure)
      gate.readPending()
    }
  } finally {
    if (gate.current(expected)) saving.value = false
  }
}
async function remove(row: Application) {
  const expected = gate.ticket()
  try {
    await ElMessageBox.confirm(
      `删除“${row.name}”会撤销该应用全部会话，应用标识不能再使用。`,
      '删除开放应用',
      { confirmButtonText: '删除', cancelButtonText: '取消', type: 'warning' },
    )
    if (!gate.current(expected)) return
    await gate.request(`/applications/${row.id}?version=${row.version}`, { method: 'DELETE' })
    if (gate.current(expected)) {
      ElMessage.success('应用已删除')
      await load()
    }
  } catch (failure) {
    if (gate.current(expected) && failure !== 'cancel' && failure !== 'close')
      error.value = errorText(failure)
  }
}
async function recover(row: CredentialOperation) {
  const message = await gate.recover(row)
  await load()
  return message
}
onMounted(() => void load())
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">OPEN ACCESS</div>
      <h1>开放应用</h1>
      <p>管理独立应用身份、应用用户和调用策略。</p>
    </div>
    <el-button v-if="canCreate" type="primary" :disabled="blocked" @click="edit(null)"
      >创建应用</el-button
    >
  </div>
  <CredentialRecoveryPanel
    :scope="gate.identity.value"
    :operations="pending"
    :error="gate.recoveryError.value"
    :recover="recover"
    :forget="(row) => gate.forget(row, gate.ticket())"
  />
  <section class="surface table-surface">
    <div class="table-toolbar">
      <el-input
        v-model="search"
        placeholder="按应用标识前缀搜索"
        clearable
        class="search-input"
        @keyup.enter="query"
      /><el-button @click="query">查询</el-button>
      <div class="toolbar-spacer" />
      <el-button text @click="load">刷新应用</el-button>
    </div>
    <el-alert v-if="error" :title="error" type="error" :closable="false" />
    <el-table :data="rows" v-loading="loading" row-key="id" class="applications-table"
      ><el-table-column label="应用" min-width="220"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><code class="table-secondary">{{ row.code }}</code
          ><small class="table-secondary">{{ row.id }}</small></template
        ></el-table-column
      ><el-table-column label="Scope" min-width="220"
        ><template #default="{ row }"
          ><span class="tiny">{{ row.allowedScopes.join(' · ') }}</span></template
        ></el-table-column
      ><el-table-column label="状态" width="90"
        ><template #default="{ row }"
          ><el-tag :type="row.enabled ? 'success' : 'info'">{{
            row.enabled ? '已启用' : '已停用'
          }}</el-tag></template
        ></el-table-column
      ><el-table-column label="额度 / 版本" min-width="170"
        ><template #default="{ row }"
          >{{ row.minuteLimit }} / 分钟 · {{ row.dailyQuota }} / 日<small class="table-secondary"
            >配置 {{ row.version }} · 凭据 {{ row.credentialVersion }}</small
          ><small class="table-secondary">{{
            row.allowedIps.join(', ') || '不限制来源'
          }}</small></template
        ></el-table-column
      ><el-table-column label="操作" min-width="260" fixed="right"
        ><template #default="{ row }"
          ><el-button
            v-if="session.can('applicationUsers.read')"
            link
            @click="userApp = row as Application"
            >用户</el-button
          ><el-button
            v-if="session.can('applicationLogs.read')"
            link
            @click="callsApp = row as Application"
            >调用</el-button
          ><template v-if="session.can('applications.write')"
            ><el-button link @click="edit(row as Application)">编辑</el-button
            ><el-button
              v-if="(row as Application).allowedScopes.every(grant)"
              link
              :disabled="blocked"
              @click="rotate(row as Application)"
              >轮换密钥</el-button
            ><el-button link type="danger" @click="remove(row as Application)"
              >删除</el-button
            ></template
          ></template
        ></el-table-column
      ></el-table
    >
    <div class="table-footer">
      <span class="subtle">共 {{ total }} 个应用</span
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
  </section>
  <el-dialog
    v-model="open"
    :title="editing ? '编辑应用' : '创建应用'"
    width="min(700px,94vw)"
    :close-on-click-modal="false"
    :show-close="!saving"
    :close-on-press-escape="!saving"
    destroy-on-close
    ><el-form label-position="top" :disabled="saving" @submit.prevent="save"
      ><el-alert v-if="formError" :title="formError" type="error" :closable="false" />
      <p v-if="editing" class="subtle">
        按配置版本 {{ editing.version }} 保存。发生冲突时，重新读取后再修改。
      </p>
      <el-form-item label="应用名称"
        ><el-input v-model="form.name" aria-label="应用名称" maxlength="80" /></el-form-item
      ><el-form-item label="应用标识"
        ><el-input v-model="form.code" aria-label="应用标识" :disabled="!!editing" maxlength="64" />
        <p class="tiny subtle">
          3–64 位小写字母、数字及 ._-，以字母开头；创建后不能修改或复用。
        </p></el-form-item
      ><el-form-item label="应用状态"
        ><el-switch
          v-model="form.enabled"
          active-text="启用"
          :disabled="!canPolicy && !form.enabled" /></el-form-item
      ><el-form-item label="开放 Scope"
        ><el-checkbox-group v-model="form.scopes"
          ><el-checkbox
            v-for="scope in openScopes"
            :key="scope"
            :value="scope"
            :disabled="!grant(scope) || !canPolicy"
            >{{ scopeLabels[scope] }}</el-checkbox
          ></el-checkbox-group
        ></el-form-item
      >
      <fieldset :disabled="!canPolicy" class="access-policy">
        <AccessPolicyFields
          v-model:ips="form.ips"
          v-model:minute-limit="form.minute"
          v-model:daily-quota="form.daily"
        />
      </fieldset>
      <p v-if="!canPolicy" class="tiny subtle">
        当前权限只允许修改名称或停用；Scope、来源、额度和重新启用需要对应应用用户权限。
      </p></el-form
    ><template #footer
      ><el-button v-if="editing" :disabled="saving" @click="reloadEdit">重新读取</el-button
      ><el-button :disabled="saving" @click="open = false">取消</el-button
      ><el-button
        type="primary"
        :loading="saving"
        :disabled="editing ? saving : blocked"
        @click="save"
        >保存应用</el-button
      ></template
    ></el-dialog
  >
  <el-dialog
    :model-value="!!userApp"
    title="应用用户"
    width="min(1080px,96vw)"
    destroy-on-close
    @update:model-value="
      (value) => {
        if (!value) userApp = null
      }
    "
    ><ApplicationUsersPanel v-if="userApp" :key="userApp.id" :application="userApp"
  /></el-dialog>
  <el-dialog
    :model-value="!!callsApp"
    title="应用调用记录"
    width="min(1280px,96vw)"
    destroy-on-close
    @update:model-value="
      (value) => {
        if (!value) callsApp = null
      }
    "
    ><ApplicationCallsView v-if="callsApp" :key="callsApp.id" :application="callsApp"
  /></el-dialog>
  <SecretResultDialog v-model="gate.secret.value" />
</template>
<style scoped>
.access-policy {
  border: 0;
  padding: 0;
  margin: 0;
  min-width: 0;
}
</style>
