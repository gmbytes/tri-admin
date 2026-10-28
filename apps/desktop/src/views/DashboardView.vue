<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { User, UserFilled, Menu, Lock, ArrowRight } from '@element-plus/icons-vue'
import { useRouter } from 'vue-router'
import { useSession } from '../stores/session'
import type { MenuRecord, Permission } from '../core/types'
import { notifyError, scopeText } from '../ui'
const session = useSession(),
  router = useRouter(),
  loading = ref(true),
  counts = ref<Record<string, number>>({})
const cards = [
  {
    key: 'users',
    label: '用户',
    description: '工作空间成员',
    icon: User,
    permission: 'users.read',
    path: '/users',
  },
  {
    key: 'roles',
    label: '角色',
    description: '授权与数据范围',
    icon: UserFilled,
    permission: 'roles.read',
    path: '/roles',
  },
  {
    key: 'menus',
    label: '菜单',
    description: '导航与页面入口',
    icon: Menu,
    permission: 'menus.read',
    path: '/menus',
  },
  {
    key: 'permissions',
    label: '操作权限',
    description: '清晰控制每项能力',
    icon: Lock,
    permission: 'permissions.read',
    path: '/permissions',
  },
]
async function load() {
  loading.value = true
  try {
    await Promise.all(
      cards
        .filter((card) => session.can(card.permission))
        .map(async (card) => {
          if (card.key === 'menus')
            counts.value.menus = (await session.request<MenuRecord[]>('/menus')).data.length
          else if (card.key === 'permissions')
            counts.value.permissions = (
              await session.request<Permission[]>('/permissions')
            ).data.length
          else
            counts.value[card.key] =
              (await session.request<unknown[]>(`/${card.key}?pageSize=1`)).meta?.total ?? 0
        }),
    )
  } catch (error) {
    notifyError(error)
  } finally {
    loading.value = false
  }
}
onMounted(load)
</script>
<template>
  <div class="page-heading">
    <div>
      <h1>工作台</h1>
      <p>欢迎回来，{{ session.user?.displayName }}。管理服务已连接。</p>
    </div>
    <el-button @click="load" :loading="loading">刷新概览</el-button>
  </div>
  <div class="stat-grid">
    <section
      v-for="card in cards.filter((card) => session.can(card.permission))"
      :key="card.key"
      class="stat-card"
    >
      <div class="stat-top">
        <span>{{ card.label }}</span
        ><el-icon><component :is="card.icon" /></el-icon>
      </div>
      <div class="stat-value">{{ loading ? '—' : (counts[card.key] ?? '—') }}</div>
      <small>{{ card.description }}</small>
    </section>
  </div>
  <div class="dashboard-bottom">
    <section class="surface">
      <h3>快捷入口</h3>
      <div class="quick-links">
        <button
          v-for="node in session.navigation.filter((node) => node.viewKey)"
          :key="node.id"
          @click="router.push(node.path)"
        >
          <span>{{ node.label }}</span
          ><el-icon><ArrowRight /></el-icon>
        </button>
      </div>
    </section>
    <section class="surface">
      <h3>当前会话</h3>
      <div class="info-list">
        <span
          >账号<b>{{ session.user?.username }}</b></span
        ><span
          >数据范围<b>{{ scopeText(session.user?.dataScope) }}</b></span
        ><span
          >服务状态<b class="positive">{{
            session.connection === 'ready' ? '已连接' : '待检查'
          }}</b></span
        >
      </div>
    </section>
  </div>
</template>
