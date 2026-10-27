<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useSession } from '../stores/session'
import { parsePage } from '../core/open-access'
import { parsePluginCapability, type McpPluginCapability } from '../core/mcp'
import { errorText } from '../ui'
const open = defineModel<boolean>({ default: false }),
  emit = defineEmits<{ select: [value: McpPluginCapability] }>(),
  session = useSession(),
  identity = computed(() => `${session.settings.apiBaseUrl}|${session.user?.id ?? ''}`)
const rows = ref<McpPluginCapability[]>([]),
  page = ref(1),
  total = ref(0),
  search = ref(''),
  loading = ref(false),
  error = ref('')
let sequence = 0,
  control: AbortController | null = null
function stop() {
  sequence++
  control?.abort()
  control = null
  loading.value = false
}
async function load() {
  stop()
  if (!open.value) return
  const request = ++sequence,
    actor = identity.value,
    abort = new AbortController()
  control = abort
  loading.value = true
  error.value = ''
  try {
    const query = new URLSearchParams({
      page: String(page.value),
      pageSize: '20',
      search: search.value,
      sort: 'name',
    })
    const result = parsePage(
      await session.request(`/mcp/plugin-capabilities?${query}`, { signal: abort.signal }),
      parsePluginCapability,
    )
    if (open.value && sequence === request && identity.value === actor) {
      rows.value = result.data
      total.value = result.meta.total
    }
  } catch (value) {
    if (open.value && sequence === request && identity.value === actor)
      error.value = errorText(value)
  } finally {
    if (sequence === request) {
      loading.value = false
      control = null
    }
  }
}
function searchRows() {
  page.value = 1
  void load()
}
function choose(value: McpPluginCapability) {
  if (!value.current || loading.value) return
  emit('select', value)
  open.value = false
}
const key = (value: McpPluginCapability) =>
  `${value.extensionId}:${value.revisionId}:${value.contributionId}`
watch(open, (value) => {
  stop()
  rows.value = []
  total.value = 0
  error.value = ''
  if (value) {
    page.value = 1
    search.value = ''
    void load()
  }
})
watch(identity, () => {
  open.value = false
  stop()
  rows.value = []
})
onBeforeUnmount(stop)
</script>
<template>
  <el-dialog
    v-model="open"
    class="plugin-source-dialog"
    title="选择插件来源"
    top="5vh"
    width="min(960px, 96vw)"
    :close-on-click-modal="false"
  >
    <p class="subtle">只列出当前范围内已启用插件的来源。选择并保存连接后，仍需显式启动。</p>
    <el-alert v-if="error" :title="error" type="error" :closable="false" />
    <div class="table-toolbar">
      <el-input
        v-model="search"
        placeholder="按插件名称前缀搜索"
        clearable
        @keyup.enter="searchRows"
      />
      <el-button :loading="loading" @click="searchRows">搜索来源</el-button>
    </div>
    <el-table
      :data="rows"
      :row-key="key"
      class="mcp-source-options"
      v-loading="loading"
      max-height="46vh"
    >
      <el-table-column label="插件" min-width="190"
        ><template #default="{ row }"
          ><strong>{{ row.extensionName }}</strong
          ><small class="source-secondary"
            >修订 {{ row.revision }} · 版本 {{ row.extensionVersion }}</small
          ><code class="source-secondary">{{ row.revisionId }}</code></template
        ></el-table-column
      >
      <el-table-column label="批准来源" min-width="190"
        ><template #default="{ row }"
          >{{ row.label
          }}<small class="source-secondary">{{ row.capabilityId }} · {{ row.contributionId }}</small
          ><el-tag v-if="!row.current" type="warning">部署能力不可用</el-tag></template
        ></el-table-column
      >
      <el-table-column label="批准工具" min-width="190"
        ><template #default="{ row }"
          ><span>{{ row.allowedTools.join(' · ') || '未批准工具' }}</span></template
        ></el-table-column
      >
      <el-table-column label="操作" width="110"
        ><template #default="{ row }"
          ><el-button
            text
            :disabled="loading || !row.current"
            @click="choose(row as McpPluginCapability)"
            >使用来源</el-button
          ></template
        ></el-table-column
      >
    </el-table>
    <el-empty v-if="!loading && !rows.length" description="暂无匹配的启用来源" />
    <el-pagination
      v-model:current-page="page"
      :page-size="20"
      :total="total"
      layout="total, prev, pager, next"
      @current-change="load"
    />
    <template #footer><el-button @click="open = false">关闭选择</el-button></template>
  </el-dialog>
</template>
<style scoped>
:global(.plugin-source-dialog) {
  max-width: calc(100vw - 32px);
  max-height: calc(100vh - 40px);
  display: flex;
  flex-direction: column;
}
:global(.plugin-source-dialog .el-dialog__body) {
  min-height: 0;
  overflow: auto;
}
.source-secondary {
  display: block;
  margin-top: 6px;
  font-size: 11px;
  overflow-wrap: anywhere;
}
.el-pagination {
  margin-top: 18px;
}
</style>
