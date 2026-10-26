<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { Search, Refresh } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import type { Permission, ApiCatalog } from '../core/types'
import { notifyError } from '../ui'
const session = useSession(),
  rows = ref<Permission[]>([]),
  loading = ref(false),
  search = ref('')
const active = ref('permissions'),
  catalog = ref<ApiCatalog | null>(null),
  apiSearch = ref(''),
  apiMethod = ref(''),
  apiPage = ref(1),
  apiPageSize = ref(20)
const apiFiltered = computed(() =>
  (catalog.value?.endpoints ?? []).filter(
    (item) =>
      (!apiMethod.value || item.method === apiMethod.value) &&
      `${item.operationId} ${item.path} ${item.label} ${item.requiredPermissions.join(' ')} ${item.requiredScopes.join(' ')} ${item.group} ${item.notes}`
        .toLowerCase()
        .includes(apiSearch.value.toLowerCase()),
  ),
)
const apiRows = computed(() =>
  apiFiltered.value.slice(
    (apiPage.value - 1) * apiPageSize.value,
    apiPage.value * apiPageSize.value,
  ),
)
const methods = computed(() =>
  [...new Set(catalog.value?.endpoints.map((item) => item.method) ?? [])].sort(),
)
watch([apiSearch, apiMethod], () => {
  apiPage.value = 1
})
function accessLabel(access: string) {
  return (
    (
      {
        public: '公开',
        authenticated: '登录',
        refreshCredential: '刷新凭证',
        permission: '操作权限',
        openApplication: '应用服务身份',
        openUser: '应用用户身份',
        openRefreshCredential: '开放刷新凭证',
      } as Record<string, string>
    )[access] ?? access
  )
}
function methodType(method: string) {
  return method === 'GET'
    ? 'success'
    : method === 'DELETE'
      ? 'danger'
      : method === 'PATCH'
        ? 'warning'
        : 'info'
}
const filtered = computed(() =>
  rows.value.filter((item) =>
    `${item.code} ${item.label} ${item.group}`.toLowerCase().includes(search.value.toLowerCase()),
  ),
)
async function load() {
  loading.value = true
  try {
    const [permissions, apis] = await Promise.all([
      session.request<Permission[]>('/permissions'),
      session.request<ApiCatalog>('/api-catalog'),
    ])
    rows.value = permissions.data
    catalog.value = apis.data
  } catch (failure) {
    notifyError(failure)
  } finally {
    loading.value = false
  }
}
onMounted(load)
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">PERMISSION CATALOG</div>
      <h1>权限目录</h1>
      <p>查看可授权的操作能力和当前服务的接口，通过角色统一分配。</p>
    </div>
    <el-button :icon="Refresh" @click="load">刷新</el-button>
  </div>
  <section class="surface table-surface">
    <el-tabs v-model="active" class="catalog-tabs"
      ><el-tab-pane label="权限操作" name="permissions" /><el-tab-pane label="API 接口" name="apis"
    /></el-tabs>
    <template v-if="active === 'permissions'">
      <div class="table-toolbar">
        <el-input
          v-model="search"
          :prefix-icon="Search"
          placeholder="搜索权限名或代码"
          clearable
          class="search-input"
        />
        <div class="toolbar-spacer" />
        <span class="subtle">{{ filtered.length }} 项能力</span>
      </div>
      <el-table v-loading="loading" :data="filtered"
        ><el-table-column prop="label" label="权限名" min-width="220" /><el-table-column
          label="操作码"
          min-width="260"
          ><template #default="{ row }"
            ><code class="permission-code">{{ row.code }}</code></template
          ></el-table-column
        ><el-table-column prop="group" label="分组" min-width="160"
      /></el-table>
    </template>
    <template v-else>
      <div class="table-toolbar">
        <el-input
          v-model="apiSearch"
          :prefix-icon="Search"
          placeholder="搜索操作标识、路径或权限"
          clearable
          class="search-input"
        />
        <el-select v-model="apiMethod" aria-label="HTTP 方法" class="method-select"
          ><el-option label="全部方法" value="" /><el-option
            v-for="method in methods"
            :key="method"
            :label="method"
            :value="method"
        /></el-select>
        <div class="toolbar-spacer" />
        <span class="subtle">{{ apiFiltered.length }} 个接口</span>
      </div>
      <el-table v-loading="loading" :data="apiRows" row-key="operationId" class="api-catalog-table">
        <el-table-column label="接口" min-width="210"
          ><template #default="{ row }"
            ><strong>{{ row.label }}</strong
            ><small class="table-secondary">{{ row.operationId }}</small></template
          ></el-table-column
        >
        <el-table-column label="方法" width="100"
          ><template #default="{ row }"
            ><el-tag :type="methodType(row.method)">{{ row.method }}</el-tag></template
          ></el-table-column
        >
        <el-table-column label="路径" min-width="330"
          ><template #default="{ row }"
            ><code class="permission-code">{{ row.path }}</code></template
          ></el-table-column
        >
        <el-table-column label="访问要求" min-width="200"
          ><template #default="{ row }"
            ><span>{{ accessLabel(row.access) }}</span
            ><small v-for="code in row.requiredPermissions" :key="code" class="table-secondary">{{
              code
            }}</small
            ><small v-for="scope in row.requiredScopes" :key="scope" class="table-secondary">{{
              scope
            }}</small
            ><small v-if="row.notes" class="table-secondary">{{ row.notes }}</small></template
          ></el-table-column
        >
        <el-table-column prop="group" label="分组" min-width="120" />
      </el-table>
      <div class="catalog-footer">
        <span v-if="catalog" class="subtle" :title="catalog.checksum"
          >目录版本 {{ catalog.schemaVersion }} · 指纹 {{ catalog.checksum.slice(0, 12) }}</span
        >
        <div class="toolbar-spacer" />
        <el-pagination
          v-model:current-page="apiPage"
          v-model:page-size="apiPageSize"
          :page-sizes="[20, 50, 100]"
          layout="total,sizes,prev,pager,next"
          :total="apiFiltered.length"
        />
      </div>
    </template>
  </section>
</template>
<style scoped>
.catalog-tabs {
  padding: 12px 24px 0;
}
.method-select {
  width: 140px;
}
.catalog-footer {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 20px 24px;
}
.catalog-footer .subtle {
  font-size: 12px;
}
</style>
