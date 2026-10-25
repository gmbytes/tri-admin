<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue'
import { useSession } from '../stores/session'
import { parseHistoryPage, type DataResource, type ImportSummary } from '../core/data-tools'
import { dateText, errorText } from '../ui'
const props = defineProps<{ resources: DataResource[]; revision: number; disabled: boolean }>(),
  emit = defineEmits<{ open: [string] }>(),
  session = useSession()
const rows = ref<ImportSummary[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  resource = ref(''),
  status = ref(''),
  loading = ref(false),
  error = ref('')
const statusLabels: Record<string, string> = {
  pending: '待处理',
  completed: '已完成',
  partial: '部分完成',
  failed: '有错误',
  expired: '已过期',
}
const payloadLabels: Record<string, string> = {
  full: '内容可用',
  expired: '等待清理',
  redacted: '内容已清理',
}
const choices = computed(() => props.resources.filter((value) => value.importable)),
  query = computed(() => [page.value, pageSize.value, resource.value, status.value, props.revision])
let alive = true,
  sequence = 0,
  current: AbortController | null = null
async function load() {
  const request = ++sequence
  current?.abort()
  const control = new AbortController()
  current = control
  loading.value = true
  error.value = ''
  const expected = { page: page.value, pageSize: pageSize.value },
    parameters = new URLSearchParams({
      page: String(expected.page),
      pageSize: String(expected.pageSize),
    })
  if (resource.value) parameters.set('resource', resource.value)
  if (status.value) parameters.set('status', status.value)
  try {
    const response = await session.request<unknown[]>('/data-tools/imports?' + parameters, {
      signal: control.signal,
    })
    if (!alive || control.signal.aborted || request !== sequence) return
    const result = parseHistoryPage(response, expected)
    rows.value = result.rows
    total.value = result.total
  } catch (failure) {
    if (alive && !control.signal.aborted && request === sequence) error.value = errorText(failure)
  } finally {
    if (alive && request === sequence) loading.value = false
  }
}
watch([resource, status, pageSize], () => (page.value = 1))
watch(
  () => props.revision,
  () => (page.value = 1),
)
watch(query, () => void load(), { flush: 'post' })
onMounted(() => void load())
onBeforeUnmount(() => {
  alive = false
  sequence++
  current?.abort()
})
</script>
<template>
  <section class="surface data-import-history">
    <div class="history-heading">
      <div>
        <h2>导入历史</h2>
        <p class="tiny subtle">原始内容保留 24 小时；到期后回执仍可查询，旧请求不会重复创建。</p>
      </div>
      <el-button @click="load">刷新历史</el-button>
    </div>
    <div class="history-filters">
      <el-select v-model="resource" aria-label="历史资源" placeholder="全部资源"
        ><el-option label="全部资源" value="" /><el-option
          v-for="item in choices"
          :key="item.key"
          :label="item.label"
          :value="item.key" /></el-select
      ><el-select v-model="status" aria-label="历史状态" placeholder="全部状态"
        ><el-option label="全部状态" value="" /><el-option
          v-for="(label, value) in statusLabels"
          :key="value"
          :label="label"
          :value="value"
      /></el-select>
      <div class="history-spacer" />
      <el-tag type="info">{{ total }} 条记录</el-tag>
    </div>
    <el-alert v-if="error" :title="error" type="error" :closable="false" />
    <el-table v-loading="loading" :data="rows" row-key="id"
      ><el-table-column label="资源 / 导入标识" min-width="205"
        ><template #default="{ row }"
          ><div>{{ resources.find((r) => r.key === row.resource)?.label ?? row.resource }}</div>
          <div class="tiny subtle">{{ row.id }}</div></template
        ></el-table-column
      ><el-table-column label="创建时间" min-width="170"
        ><template #default="{ row }">{{ dateText(row.createdAt) }}</template></el-table-column
      ><el-table-column label="状态" width="105"
        ><template #default="{ row }"
          ><el-tag
            :type="
              row.status === 'completed' ? 'success' : row.status === 'pending' ? 'info' : 'warning'
            "
            >{{ statusLabels[row.status] }}</el-tag
          ></template
        ></el-table-column
      ><el-table-column label="处理情况" min-width="155"
        ><template #default="{ row }"
          ><div>已创建 {{ row.appliedCount }} / {{ row.rowCount }}</div>
          <div class="tiny subtle">
            待处理 {{ row.readyCount }} · 问题 {{ row.errorCount }}
          </div></template
        ></el-table-column
      ><el-table-column label="原始内容" width="125"
        ><template #default="{ row }">{{
          payloadLabels[row.payloadState]
        }}</template></el-table-column
      ><el-table-column label="操作" width="110"
        ><template #default="{ row }"
          ><el-button text :disabled="disabled || loading" @click="emit('open', row.id)"
            >查看结果</el-button
          ></template
        ></el-table-column
      ></el-table
    >
    <el-pagination
      v-model:current-page="page"
      v-model:page-size="pageSize"
      :page-sizes="[20, 50, 100]"
      :total="total"
      layout="total, sizes, prev, pager, next"
    />
  </section>
</template>
<style scoped>
.data-import-history {
  padding: 24px;
  margin-bottom: 20px;
}
.history-heading {
  display: flex;
  justify-content: space-between;
  gap: 20px;
  align-items: start;
}
.history-heading h2 {
  margin: 0 0 12px;
  font-size: 18px;
}
.history-filters {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin: 16px 0;
}
.history-filters .el-select {
  width: 170px;
}
.history-spacer {
  flex: 1;
}
.el-pagination {
  margin-top: 18px;
  justify-content: flex-end;
}
</style>
