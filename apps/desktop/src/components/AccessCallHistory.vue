<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'
import { useCredentialPage } from '../composables/useCredentialPage'
import { objectId, parseCall, parsePage, type CallRecord } from '../core/open-access'
import { dateText, errorText } from '../ui'
const props = defineProps<{ kind: 'token' | 'application'; id?: string }>(),
  gate = useCredentialPage(),
  rows = ref<CallRecord[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  loading = ref(false),
  error = ref(''),
  status = ref<number | undefined>(),
  principal = ref(''),
  state = ref(''),
  userId = ref(''),
  range = ref<[Date, Date] | null>(null)
let sequence = 0
async function load() {
  const expected = gate.ticket(),
    request = ++sequence
  loading.value = true
  error.value = ''
  try {
    if (props.kind === 'application' && !props.id) {
      rows.value = []
      total.value = 0
      return
    }
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: String(pageSize.value),
    })
    if (props.id && props.kind === 'token') query.set('tokenId', props.id)
    if (status.value) query.set('status', String(status.value))
    if (props.kind === 'application' && principal.value) query.set('principalKind', principal.value)
    if (props.kind === 'application' && state.value) query.set('state', state.value)
    if (props.kind === 'application' && userId.value.trim()) {
      if (!objectId(userId.value.trim())) throw new Error('用户 ID 需要 24 位小写十六进制标识')
      query.set('userId', userId.value.trim())
    }
    if (range.value) {
      query.set('from', range.value[0].toISOString())
      query.set('to', range.value[1].toISOString())
    }
    const path = props.kind === 'token' ? '/api-tokens/calls' : `/applications/${props.id}/calls`
    const result = parsePage(await gate.request(`${path}?${query}`), (value) =>
      parseCall(value, props.kind),
    )
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
watch(() => [props.kind, props.id], query)
gate.onReset(() => {
  sequence++
  rows.value = []
  total.value = 0
  error.value = ''
  loading.value = false
})
onMounted(() => void load())
</script>
<template>
  <section class="surface table-surface access-call-history">
    <div class="table-toolbar">
      <h3>调用记录</h3>
      <el-input-number
        v-model="status"
        :min="100"
        :max="599"
        placeholder="HTTP 状态"
        aria-label="HTTP 状态筛选"
      /><el-select
        v-if="kind === 'application'"
        v-model="principal"
        aria-label="调用身份"
        style="width: 150px"
        ><el-option label="全部身份" value="" /><el-option
          label="应用服务"
          value="application" /><el-option label="应用用户" value="user" /></el-select
      ><el-select
        v-if="kind === 'application'"
        v-model="state"
        aria-label="记录状态"
        style="width: 150px"
        ><el-option label="全部结果" value="" /><el-option
          label="待确认"
          value="started" /><el-option label="已确认" value="finished" /></el-select
      ><el-input
        v-if="kind === 'application'"
        v-model="userId"
        aria-label="用户 ID 筛选"
        placeholder="用户 ID（可选）"
        clearable
        style="max-width: 240px"
      /><el-date-picker
        v-model="range"
        type="datetimerange"
        start-placeholder="开始时间"
        end-placeholder="结束时间"
        style="max-width: 350px"
      /><el-button @click="query">查询记录</el-button>
      <div class="toolbar-spacer" />
      <el-button text @click="load">刷新记录</el-button>
    </div>
    <p class="subtle access-note">
      保留 30 天；“待确认”不代表失败或成功。HTTP 状态表示响应处理结果，文件保存仍需完成内容校验。
    </p>
    <el-alert v-if="error" :title="error" type="error" :closable="false" /><el-table
      :data="rows"
      v-loading="loading"
      row-key="id"
      ><el-table-column label="操作" min-width="220"
        ><template #default="{ row }"
          ><strong>{{ row.operationId ?? row.route }}</strong
          ><small class="table-secondary">{{ row.method }} {{ row.route }}</small></template
        ></el-table-column
      ><el-table-column label="身份 / 来源" min-width="180"
        ><template #default="{ row }"
          ><span>{{
            row.principalKind === 'user'
              ? '应用用户'
              : row.principalKind === 'application'
                ? '应用服务'
                : 'API Token'
          }}</span
          ><small class="table-secondary">{{ row.ip ?? '来源未知' }}</small
          ><small class="table-secondary" v-if="row.userId">{{ row.userId }}</small></template
        ></el-table-column
      ><el-table-column label="结果" min-width="150"
        ><template #default="{ row }"
          ><el-tag
            :type="row.state === 'started' ? 'warning' : row.status >= 400 ? 'danger' : 'success'"
            >{{ row.state === 'started' ? '待确认' : `HTTP ${row.status}` }}</el-tag
          ><small v-if="row.errorCode" class="table-secondary">{{ row.errorCode }}</small></template
        ></el-table-column
      ><el-table-column label="时间 / 耗时" min-width="190"
        ><template #default="{ row }"
          >{{ dateText(row.createdAt)
          }}<small class="table-secondary">{{
            row.durationMs === null ? '—' : `${row.durationMs} ms`
          }}</small></template
        ></el-table-column
      ><el-table-column label="请求 ID" min-width="260"
        ><template #default="{ row }"
          ><code>{{ row.requestId }}</code></template
        ></el-table-column
      ></el-table
    >
    <div class="table-footer">
      <span class="subtle">共 {{ total }} 条</span
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
</template>
<style scoped>
.table-toolbar {
  flex-wrap: wrap;
}
.access-note {
  padding: 0 20px;
}
</style>
