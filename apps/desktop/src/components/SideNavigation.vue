<script setup lang="ts">
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ArrowDown, Grid, User, UserFilled, Menu, Lock } from '@element-plus/icons-vue'
import type { MenuRecord } from '../core/types'
defineOptions({ name: 'SideNavigation' })
const props = withDefaults(defineProps<{ nodes: MenuRecord[]; depth?: number }>(), { depth: 0 })
const route = useRoute(),
  router = useRouter(),
  expanded = ref(new Set<string>())
function toggle(id: string) {
  const value = new Set(expanded.value)
  value.has(id) ? value.delete(id) : value.add(id)
  expanded.value = value
}
const icons: Record<string, unknown> = {
  dashboard: Grid,
  users: User,
  roles: UserFilled,
  menus: Menu,
  permissions: Lock,
}
</script>
<template>
  <div v-for="node in nodes" :key="node.id" class="nav-node">
    <div
      class="nav-row"
      :class="{ active: route.path === node.path }"
      :style="{ paddingLeft: `${14 + Math.min(props.depth, 5) * 12}px` }"
    >
      <button :title="node.label" @click="node.viewKey ? router.push(node.path) : toggle(node.id)">
        <el-icon><component :is="icons[node.viewKey ?? ''] ?? Grid" /></el-icon
        ><span>{{ node.label }}</span></button
      ><button
        v-if="node.children?.length"
        class="nav-toggle"
        :class="{ expanded: expanded.has(node.id) }"
        @click="toggle(node.id)"
      >
        <el-icon><ArrowDown /></el-icon>
      </button>
    </div>
    <SideNavigation
      v-if="node.children?.length && expanded.has(node.id)"
      :nodes="node.children"
      :depth="props.depth + 1"
    />
  </div>
</template>
