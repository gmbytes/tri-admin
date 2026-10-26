<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { useCredentialPage } from '../composables/useCredentialPage'
import {
  parseToken,
  parsePage,
  parseIssue,
  policyFromForm,
  type ApiToken,
} from '../core/open-access'
import type { Permission } from '../core/types'
import { dateText, errorText } from '../ui'
import AccessPolicyFields from '../components/AccessPolicyFields.vue'
import SecretResultDialog from '../components/SecretResultDialog.vue'
import CredentialRecoveryPanel from '../components/CredentialRecoveryPanel.vue'
import AccessCallHistory from '../components/AccessCallHistory.vue'
const gate = useCredentialPage(),
  session = gate.session,
  rows = ref<ApiToken[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  loading = ref(false),
  error = ref(''),
  open = ref(false),
  saving = ref(false),
  formError = ref(''),
  choices = ref<Permission[]>([]),
  calls = ref(''),
  showCalls = ref(false)
const form = reactive({
  name: '',
  scopes: [] as string[],
  days: 30,
  ips: '',
  minute: 600,
  daily: 100000,
})
const pending = computed(() => gate.pending.value.filter((row) => row.kind === 'token.create')),
  blocked = computed(() => saving.value || !!gate.recoveryError.value || pending.value.length > 0)
let sequence = 0
gate.onReset(() => {
  sequence++
  rows.value = []
  total.value = 0
  open.value = false
  showCalls.value = false
  loading.value = false
  saving.value = false
  formError.value = ''
  error.value = ''
  form.scopes = []
  choices.value = []
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
    const value = parsePage(await gate.request(`/api-tokens?${query}`), parseToken)
    if (!gate.current(expected) || request !== sequence) return
    rows.value = value.data
    total.value = value.meta.total
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
async function edit() {
  const expected = gate.ticket()
  formError.value = ''
  try {
    const permissions = await gate.request<Permission[]>('/api-tokens/scopes')
    if (!gate.current(expected)) return
    choices.value = permissions.data
    Object.assign(form, { name: '', scopes: [], days: 30, ips: '', minute: 600, daily: 100000 })
    open.value = true
  } catch (failure) {
    if (gate.current(expected)) error.value = errorText(failure)
  }
}
async function save() {
  formError.value = ''
  if (
    !form.name.trim() ||
    !form.scopes.length ||
    !Number.isInteger(form.days) ||
    form.days < 1 ||
    form.days > 365
  ) {
    formError.value = '请填写名称、选择 Scope，并设置 1–365 天有效期'
    return
  }
  const expected = gate.ticket()
  let operation: string | null = null
  saving.value = true
  try {
    const policy = policyFromForm(form.ips, form.minute, form.daily),
      marker = gate.begin('token.create')
    operation = marker.requestId
    const value = parseIssue(
      (
        await gate.request('/api-tokens', {
          method: 'POST',
          body: {
            requestId: marker.requestId,
            name: form.name.trim(),
            scopes: [...form.scopes],
            expiresInDays: form.days,
            ...policy,
          },
        })
      ).data,
      'token',
    )
    if (value.entity.requestId !== marker.requestId) throw new Error('签发结果与请求不匹配')
    if (!gate.current(expected)) return
    gate.confirmed(marker.requestId, expected)
    open.value = false
    if (value.secret)
      gate.secret.value = { kind: 'token', id: value.entity.id, value: value.secret }
    else ElMessage.warning('签发已确认，原秘密不能再次展示')
    await load()
  } catch (failure) {
    if (!gate.current(expected)) return
    if (operation) gate.rejected(operation, expected, failure)
    formError.value = errorText(failure)
    gate.readPending()
  } finally {
    if (gate.current(expected)) saving.value = false
  }
}
async function revoke(row: ApiToken) {
  const expected = gate.ticket()
  try {
    await ElMessageBox.confirm(`撤销“${row.name}”后，新调用会立即被拒绝。`, '撤销 Token', {
      confirmButtonText: '撤销',
      cancelButtonText: '取消',
      type: 'warning',
    })
    if (!gate.current(expected)) return
    parseToken(
      (await gate.request(`/api-tokens/${row.id}/revoke`, { method: 'POST', body: {} })).data,
    )
    if (gate.current(expected)) {
      ElMessage.success('Token 已撤销')
      await load()
    }
  } catch (failure) {
    if (gate.current(expected) && failure !== 'cancel' && failure !== 'close')
      error.value = errorText(failure)
  }
}
async function recover(row: Parameters<typeof gate.recover>[0]) {
  const message = await gate.recover(row)
  await load()
  return message
}
function forget(row: Parameters<typeof gate.forget>[0]) {
  gate.forget(row, gate.ticket())
}
const labels = { active: '有效', expired: '已过期', revoked: '已撤销' }
function showHistory(id = '') {
  calls.value = id
  showCalls.value = true
}
onMounted(() => void load())
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">CONTROLLED AUTOMATION</div>
      <h1>API Token</h1>
      <p>为自动化设置明确权限、来源和额度，秘密仅在签发后展示一次。</p>
    </div>
    <el-button
      v-if="session.can('apiTokens.write')"
      type="primary"
      :disabled="blocked"
      @click="edit"
      >签发 Token</el-button
    >
  </div>
  <CredentialRecoveryPanel
    :scope="gate.identity.value"
    :operations="pending"
    :error="gate.recoveryError.value"
    :recover="recover"
    :forget="forget"
  />
  <section class="surface table-surface">
    <div class="table-toolbar">
      <el-input
        v-model="search"
        placeholder="按 Token 名称前缀搜索"
        clearable
        class="search-input"
        @keyup.enter="query"
      /><el-button @click="query">查询</el-button>
      <div class="toolbar-spacer" />
      <el-button text @click="showHistory()">全部调用记录</el-button
      ><el-button text @click="load">刷新</el-button>
    </div>
    <el-alert v-if="error" :title="error" type="error" :closable="false" /><el-table
      :data="rows"
      v-loading="loading"
      row-key="id"
      ><el-table-column label="Token" min-width="220"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><small class="table-secondary">{{ row.id }}</small></template
        ></el-table-column
      ><el-table-column label="Scope" min-width="230"
        ><template #default="{ row }"
          ><span class="tiny">{{ row.scopes.join(' · ') }}</span></template
        ></el-table-column
      ><el-table-column label="状态" width="105"
        ><template #default="{ row }"
          ><el-tag :type="row.status === 'active' ? 'success' : 'info'">{{
            labels[row.status as keyof typeof labels]
          }}</el-tag></template
        ></el-table-column
      ><el-table-column label="额度 / 来源" min-width="170"
        ><template #default="{ row }"
          >{{ row.minuteLimit }} / 分钟<small class="table-secondary"
            >{{ row.dailyQuota }} / UTC 日</small
          ><small class="table-secondary">{{
            row.allowedIps.join(', ') || '不限制来源'
          }}</small></template
        ></el-table-column
      ><el-table-column label="过期时间" min-width="175"
        ><template #default="{ row }">{{ dateText(row.expiresAt) }}</template></el-table-column
      ><el-table-column label="操作" width="140" fixed="right"
        ><template #default="{ row }"
          ><el-button link @click="showHistory(row.id)">调用</el-button
          ><el-button
            v-if="session.can('apiTokens.write') && row.status === 'active'"
            link
            type="danger"
            @click="revoke(row as ApiToken)"
            >撤销</el-button
          ></template
        ></el-table-column
      ></el-table
    >
    <div class="table-footer">
      <span class="subtle">共 {{ total }} 个 · 最多 32 个活动 Token</span
      ><el-pagination
        v-model:current-page="page"
        v-model:page-size="pageSize"
        :page-sizes="[20, 50, 100]"
        :total="total"
        layout="sizes,prev,pager,next"
        @current-change="load"
        @size-change="query"
      />
    </div>
  </section>
  <AccessCallHistory v-if="showCalls" kind="token" :id="calls" /><el-dialog
    v-model="open"
    title="签发 Token"
    width="min(680px,94vw)"
    :close-on-click-modal="false"
    :show-close="!saving"
    :close-on-press-escape="!saving"
    destroy-on-close
    ><el-form label-position="top" :disabled="saving" @submit.prevent="save"
      ><el-alert v-if="formError" :title="formError" type="error" :closable="false" /><el-form-item
        label="Token 名称"
        ><el-input v-model="form.name" aria-label="Token 名称" maxlength="64" /></el-form-item
      ><el-form-item label="Token Scope"
        ><el-select
          v-model="form.scopes"
          multiple
          filterable
          aria-label="Token Scope"
          style="width: 100%"
          ><el-option
            v-for="row in choices"
            :key="row.code"
            :value="row.code"
            :label="`${row.label} (${row.code})`"
        /></el-select>
        <p class="tiny subtle">
          仅列出当前可授予且支持自动化的操作；账号与凭据管理仍需交互会话。
        </p></el-form-item
      ><el-form-item label="有效天数"
        ><el-input-number
          v-model="form.days"
          :min="1"
          :max="365"
          aria-label="有效天数" /></el-form-item
      ><AccessPolicyFields
        v-model:ips="form.ips"
        v-model:minute-limit="form.minute"
        v-model:daily-quota="form.daily" /></el-form
    ><template #footer
      ><el-button :disabled="saving" @click="open = false">取消</el-button
      ><el-button type="primary" :loading="saving" :disabled="blocked" @click="save"
        >签发</el-button
      ></template
    ></el-dialog
  ><SecretResultDialog v-model="gate.secret.value" />
</template>
