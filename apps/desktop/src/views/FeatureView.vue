<script setup lang="ts">
import { computed, defineAsyncComponent } from 'vue'
import { useRoute } from 'vue-router'
import { useSession } from '../stores/session'
const route = useRoute(),
  session = useSession()
const views: Record<string, unknown> = {
  dashboard: defineAsyncComponent(() => import('./DashboardView.vue')),
  users: defineAsyncComponent(() => import('./UsersView.vue')),
  roles: defineAsyncComponent(() => import('./RolesView.vue')),
  menus: defineAsyncComponent(() => import('./MenusView.vue')),
  permissions: defineAsyncComponent(() => import('./PermissionsView.vue')),
  dictionaries: defineAsyncComponent(() => import('./DictionariesView.vue')),
  parameters: defineAsyncComponent(() => import('./ParametersView.vue')),
  versions: defineAsyncComponent(() => import('./VersionsView.vue')),
  audit: defineAsyncComponent(() => import('./LogsView.vue')),
  settings: defineAsyncComponent(() => import('./SettingsView.vue')),
  files: defineAsyncComponent(() => import('./FilesView.vue')),
  exports: defineAsyncComponent(() => import('./DataToolsView.vue')),
  tokens: defineAsyncComponent(() => import('./ApiTokensView.vue')),
  apps: defineAsyncComponent(() => import('./ApplicationsView.vue')),
  codegen: defineAsyncComponent(() => import('./CodegenView.vue')),
}
const page = computed(() => views[session.findMenu(route.path)?.viewKey ?? ''])
</script>
<template>
  <component v-if="page" :is="page" :key="route.path" /><el-empty
    v-else
    description="当前页面尚未部署或无权访问"
  />
</template>
