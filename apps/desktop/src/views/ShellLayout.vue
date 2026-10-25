<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ArrowDown, Connection, Refresh, Setting } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import { notifyError } from '../ui'
import SideNavigation from '../components/SideNavigation.vue'
import ServerDialog from '../components/ServerDialog.vue'
const session = useSession(),
  route = useRoute(),
  router = useRouter(),
  serverOpen = ref(false)
const title = computed(() =>
  route.path === '/profile' ? '个人中心' : (session.findMenu(route.path)?.label ?? '工作空间'),
)
const host = computed(() => {
  try {
    return new URL(session.settings.apiBaseUrl).host
  } catch {
    return '未连接'
  }
})
watch(
  () => session.user,
  (value) => {
    if (!value) void router.replace('/login')
  },
)
async function command(value: string) {
  if (value === 'profile') await router.push('/profile')
  if (value === 'server') serverOpen.value = true
  if (value === 'logout')
    try {
      await session.logout()
      await router.replace('/login')
    } catch (error) {
      notifyError(error)
    }
}
</script>
<template>
  <div class="app-shell">
    <aside class="sidebar">
      <div class="brand">
        <img src="/brand.svg" alt="" /><span>TRI<span class="brand-light">ADMIN</span></span>
      </div>
      <div class="side-caption">工作空间</div>
      <nav><SideNavigation :nodes="session.navigation" /></nav>
      <div class="sidebar-bottom">
        <button @click="serverOpen = true">
          <el-icon><Connection /></el-icon><span>{{ host }}</span
          ><i :class="session.connection" /></button
        ><small>TRI ADMIN · 0.1.0</small>
      </div>
    </aside>
    <div class="workspace">
      <header class="topbar">
        <div class="breadcrumb">
          <span>工作空间</span><b>/</b><strong>{{ title }}</strong>
        </div>
        <div class="top-actions">
          <span class="connection-text"
            ><i :class="session.connection" />{{
              session.connection === 'ready'
                ? '服务已连接'
                : session.connection === 'checking'
                  ? '正在连接'
                  : '连接待检查'
            }}</span
          ><el-button
            text
            circle
            :icon="Refresh"
            title="检查连接"
            @click="session.probe"
          /><el-button
            text
            circle
            :icon="Setting"
            title="服务器设置"
            @click="serverOpen = true"
          /><span class="top-divider" /><el-dropdown trigger="click" @command="command"
            ><button class="user-button">
              <span class="avatar">{{ session.user?.displayName?.slice(0, 1) ?? 'U' }}</span
              ><span>{{ session.user?.displayName }}</span
              ><el-icon><ArrowDown /></el-icon></button
            ><template #dropdown
              ><el-dropdown-menu
                ><el-dropdown-item command="profile">个人中心</el-dropdown-item
                ><el-dropdown-item command="server">服务器设置</el-dropdown-item
                ><el-dropdown-item command="logout" divided
                  >退出登录</el-dropdown-item
                ></el-dropdown-menu
              ></template
            ></el-dropdown
          >
        </div>
      </header>
      <main class="page-content"><RouterView /></main>
      <footer class="workspace-footer">
        <span>Tri Admin Workspace</span><span>让每一次操作都有迹可循</span>
      </footer>
    </div>
    <ServerDialog v-model="serverOpen" />
  </div>
</template>
