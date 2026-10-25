<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Plus, Refresh } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import { dateText, errorText, notifyError } from '../ui'
import {
  integrationKinds,
  integrationLabel,
  scopeLabels,
  type ConnectionCheck,
  type IntegrationProfile,
} from '../core/settings'
import IntegrationEditor from './IntegrationEditor.vue'
import IntegrationSummary from './IntegrationSummary.vue'
const session = useSession(),
  rows = ref<IntegrationProfile[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  kind = ref(''),
  sort = ref('-createdAt')
const loading = ref(false),
  loadError = ref(''),
  editorOpen = ref(false),
  editing = ref<IntegrationProfile | null>(null),
  detail = ref<IntegrationProfile | null>(null),
  detailOpen = ref(false),
  pending = ref(new Set<string>())
const historyOpen = ref(false),
  historyProfile = ref<IntegrationProfile | null>(null),
  checks = ref<ConnectionCheck[]>([]),
  historyPage = ref(1),
  historyTotal = ref(0),
  historyLoading = ref(false),
  historyError = ref('')
let sequence = 0,
  historySequence = 0
async function load() {
  const request = ++sequence
  loading.value = true
  loadError.value = ''
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
      search: search.value,
      sort: sort.value,
    })
    if (kind.value) query.set('kind', kind.value)
    const response = await session.request<IntegrationProfile[]>(`/settings/profiles?${query}`)
    if (request !== sequence) return
    rows.value = response.data
    total.value = response.meta?.total ?? 0
  } catch (failure) {
    if (request === sequence) loadError.value = errorText(failure)
  } finally {
    if (request === sequence) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
async function edit(row: IntegrationProfile | null) {
  try {
    editing.value = row
      ? (await session.request<IntegrationProfile>(`/settings/profiles/${row.id}`)).data
      : null
    editorOpen.value = true
  } catch (failure) {
    notifyError(failure)
  }
}
async function inspect(row: IntegrationProfile) {
  try {
    detail.value = (await session.request<IntegrationProfile>(`/settings/profiles/${row.id}`)).data
    detailOpen.value = true
  } catch (failure) {
    notifyError(failure)
  }
}
async function saved() {
  ElMessage.success('集成配置已保存')
  await load()
}
async function remove(row: IntegrationProfile) {
  try {
    await ElMessageBox.confirm(
      `确认删除配置“${row.name}”？正在使用的默认配置需要先解除绑定。`,
      '删除集成配置',
      { type: 'warning', confirmButtonText: '删除配置', cancelButtonText: '取消' },
    )
    pending.value.add(row.id)
    await session.request(`/settings/profiles/${row.id}?version=${row.version}`, {
      method: 'DELETE',
    })
    if (rows.value.length === 1 && page.value > 1) page.value--
    await load()
    ElMessage.success('配置已删除')
  } catch (failure) {
    if (failure !== 'cancel' && failure !== 'close') notifyError(failure)
  } finally {
    pending.value.delete(row.id)
  }
}
async function rewrap(row: IntegrationProfile) {
  try {
    await ElMessageBox.confirm(`将“${row.name}”的凭据重新加密到服务器活动密钥？`, '轮换配置密钥', {
      confirmButtonText: '轮换密钥',
      cancelButtonText: '取消',
    })
    pending.value.add(row.id)
    await session.request(`/settings/profiles/${row.id}/rewrap`, {
      method: 'POST',
      body: { version: row.version },
    })
    await load()
    ElMessage.success('凭据密钥已轮换')
  } catch (failure) {
    if (failure !== 'cancel' && failure !== 'close') notifyError(failure)
  } finally {
    pending.value.delete(row.id)
  }
}
async function probe(row: IntegrationProfile) {
  pending.value.add(row.id)
  try {
    const result = (
      await session.request<ConnectionCheck>(`/settings/profiles/${row.id}/probe`, {
        method: 'POST',
        body: { version: row.version },
      })
    ).data
    await load()
    if (result.success) ElMessage.success(`${scopeLabels[result.scope] ?? result.scope} 检查通过`)
    else ElMessage.warning(`检查未通过：${result.errorCode ?? '未知状态'}`)
  } catch (failure) {
    notifyError(failure)
  } finally {
    pending.value.delete(row.id)
  }
}
async function loadHistory() {
  if (!historyProfile.value) return
  const request = ++historySequence
  historyLoading.value = true
  historyError.value = ''
  try {
    const response = await session.request<ConnectionCheck[]>(
      `/settings/profiles/${historyProfile.value.id}/checks?page=${historyPage.value}&pageSize=20&sort=-createdAt`,
    )
    if (request !== historySequence) return
    checks.value = response.data
    historyTotal.value = response.meta?.total ?? 0
  } catch (failure) {
    if (request === historySequence) historyError.value = errorText(failure)
  } finally {
    if (request === historySequence) historyLoading.value = false
  }
}
function history(row: IntegrationProfile) {
  historyProfile.value = row
  historyPage.value = 1
  checks.value = []
  historyOpen.value = true
  void loadHistory()
}
function checkLabel(row: IntegrationProfile) {
  return !row.lastCheck
    ? '尚未检查'
    : !row.lastCheck.isCurrent
      ? '历史检查'
      : row.lastCheck.success
        ? '检查通过'
        : '检查未通过'
}
onMounted(() => void load())
</script>
<template>
  <section class="surface table-surface">
    <div class="table-toolbar">
      <el-input
        v-model="search"
        class="search-input"
        clearable
        placeholder="按配置标识前缀搜索"
        @keyup.enter="query"
      /><el-button @click="query">查询</el-button>
      <el-select v-model="kind" class="sort-select" aria-label="配置类型筛选" @change="query"
        ><el-option label="全部类型" value="" /><el-option
          v-for="item in integrationKinds"
          :key="item.value"
          :label="item.label"
          :value="item.value"
      /></el-select>
      <el-select v-model="sort" class="sort-select" aria-label="配置排序" @change="query"
        ><el-option label="最近创建" value="-createdAt" /><el-option
          label="标识升序"
          value="code" /><el-option label="名称升序" value="name"
      /></el-select>
      <div class="toolbar-spacer" />
      <el-button text :icon="Refresh" @click="load">刷新</el-button
      ><el-button
        v-if="session.can('settings.write')"
        type="primary"
        :icon="Plus"
        @click="edit(null)"
        >新增配置</el-button
      >
    </div>
    <el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-table v-loading="loading" :data="rows" row-key="id">
      <el-table-column label="集成配置" min-width="200"
        ><template #default="{ row }"
          ><strong>{{ row.name }}</strong
          ><small class="table-secondary">{{ row.code }}</small></template
        ></el-table-column
      >
      <el-table-column label="类型" min-width="130"
        ><template #default="{ row }">{{
          integrationLabel(row.configuration.kind)
        }}</template></el-table-column
      >
      <el-table-column label="状态" width="85"
        ><template #default="{ row }"
          ><el-tag :type="row.enabled ? 'success' : 'info'">{{
            row.enabled ? '启用' : '停用'
          }}</el-tag></template
        ></el-table-column
      >
      <el-table-column label="凭据" min-width="140"
        ><template #default="{ row }"
          >{{ row.credentials.configured ? '已配置' : '未配置'
          }}<small v-if="row.credentials.configured" class="table-secondary">{{
            row.credentials.keyAvailable ? `密钥 ${row.credentials.keyId}` : '解密密钥不可用'
          }}</small></template
        ></el-table-column
      >
      <el-table-column label="最近检查" min-width="175"
        ><template #default="{ row }"
          ><el-tag
            :type="
              row.lastCheck?.isCurrent ? (row.lastCheck.success ? 'success' : 'danger') : 'info'
            "
            >{{ checkLabel(row as IntegrationProfile) }}</el-tag
          ><small v-if="row.lastCheck" class="table-secondary"
            >{{ scopeLabels[row.lastCheck.scope] ?? row.lastCheck.scope }} ·
            {{ dateText(row.lastCheck.completedAt) }}</small
          ></template
        ></el-table-column
      >
      <el-table-column label="操作" :width="session.can('settings.write') ? 325 : 150" fixed="right"
        ><template #default="{ row }">
          <el-button link type="primary" @click="inspect(row as IntegrationProfile)">详情</el-button
          ><el-button link @click="history(row as IntegrationProfile)">检查历史</el-button>
          <template v-if="session.can('settings.write')"
            ><el-button
              link
              type="primary"
              :disabled="pending.has(row.id)"
              @click="edit(row as IntegrationProfile)"
              >编辑</el-button
            ><el-button
              link
              type="primary"
              :disabled="pending.has(row.id) || !row.enabled"
              @click="probe(row as IntegrationProfile)"
              >检查连接</el-button
            ><el-button
              v-if="row.credentials.configured"
              link
              :disabled="pending.has(row.id) || !row.credentials.keyAvailable"
              @click="rewrap(row as IntegrationProfile)"
              >轮换密钥</el-button
            ><el-button
              link
              type="danger"
              :disabled="pending.has(row.id)"
              @click="remove(row as IntegrationProfile)"
              >删除</el-button
            ></template
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
  <p class="muted tiny">
    检查仅验证所列范围：MongoDB ping、S3 存储桶访问、SMTP 会话、HTTP HEAD
    或本地临时文件读写。配置变更或服务重启后需要重新检查。
  </p>
  <IntegrationEditor v-if="editorOpen" v-model="editorOpen" :profile="editing" @saved="saved" />
  <el-dialog v-model="detailOpen" title="集成配置详情" width="650px"
    ><template v-if="detail"
      ><h3>{{ detail.name }} · {{ detail.code }}</h3>
      <p>{{ detail.description || '暂无说明' }}</p>
      <IntegrationSummary :configuration="detail.configuration" />
      <p class="muted tiny">
        配置版本 {{ detail.version }} · 凭据{{
          detail.credentials.configured ? '已配置' : '未配置'
        }}
        · 更新于 {{ dateText(detail.updatedAt) }}
      </p></template
    ></el-dialog
  >
  <el-dialog v-model="historyOpen" title="连接检查历史" width="900px" @closed="historySequence++">
    <p v-if="historyProfile">{{ historyProfile.name }} · {{ historyProfile.code }}</p>
    <el-alert v-if="historyError" :title="historyError" type="error" :closable="false" />
    <el-table v-loading="historyLoading" :data="checks" row-key="id">
      <el-table-column type="expand"
        ><template #default="{ row }"
          ><el-descriptions :column="1"
            ><el-descriptions-item label="操作者 ID">{{ row.actorId }}</el-descriptions-item
            ><el-descriptions-item label="关联号">{{ row.requestId ?? '无' }}</el-descriptions-item
            ><el-descriptions-item label="完成时间">{{
              dateText(row.completedAt)
            }}</el-descriptions-item></el-descriptions
          ></template
        ></el-table-column
      >
      <el-table-column label="时间" min-width="170"
        ><template #default="{ row }">{{ dateText(row.startedAt) }}</template></el-table-column
      >
      <el-table-column label="检查范围" min-width="150"
        ><template #default="{ row }">{{
          scopeLabels[row.scope] ?? row.scope
        }}</template></el-table-column
      >
      <el-table-column label="结果" min-width="140"
        ><template #default="{ row }"
          ><el-tag :type="row.success ? 'success' : 'danger'">{{
            row.success ? '通过' : '未通过'
          }}</el-tag
          ><small class="table-secondary"
            >{{ row.isCurrent ? '当前配置' : '历史配置或会话' }} · v{{ row.profileVersion }}</small
          ></template
        ></el-table-column
      >
      <el-table-column label="状态摘要" min-width="185"
        ><template #default="{ row }"
          >{{ row.errorCode ?? '成功'
          }}<small v-if="row.upstreamStatus" class="table-secondary"
            >响应状态 {{ row.upstreamStatus }}</small
          ></template
        ></el-table-column
      >
      <el-table-column label="耗时" width="90"
        ><template #default="{ row }">{{ row.durationMs }} ms</template></el-table-column
      >
    </el-table>
    <div class="pagination">
      <el-pagination
        v-model:current-page="historyPage"
        :page-size="20"
        layout="total,prev,pager,next"
        :total="historyTotal"
        @change="loadHistory"
      />
    </div>
    <template #footer
      ><el-button @click="loadHistory">刷新历史</el-button
      ><el-button @click="historyOpen = false">关闭历史</el-button></template
    >
  </el-dialog>
</template>
