<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { Search, Refresh } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import type { Permission } from '../core/types'
import { notifyError } from '../ui'
const session = useSession(),
  rows = ref<Permission[]>([]),
  loading = ref(false),
  search = ref('')
const filtered = computed(() =>
  rows.value.filter((item) =>
    `${item.code} ${item.label} ${item.group}`.toLowerCase().includes(search.value.toLowerCase()),
  ),
)
async function load() {
  loading.value = true
  try {
    rows.value = (await session.request<Permission[]>('/permissions')).data
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
      <p>查看可授权的操作能力，通过角色统一分配。</p>
    </div>
    <el-button :icon="Refresh" @click="load">刷新</el-button>
  </div>
  <section class="surface table-surface">
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
  </section>
</template>
