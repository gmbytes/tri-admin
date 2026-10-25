<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { Connection, Lock, User, ArrowRight } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import { errorText } from '../ui'
import ServerDialog from '../components/ServerDialog.vue'
const session = useSession(),
  router = useRouter(),
  serverOpen = ref(false)
const username = ref(''),
  password = ref(''),
  remember = ref(session.settings.rememberSession),
  error = ref('')
const serverLabel = computed(() => {
  try {
    return new URL(session.settings.apiBaseUrl).host
  } catch {
    return '尚未设置服务器'
  }
})
async function submit() {
  error.value = ''
  if (!username.value || !password.value) {
    error.value = '请输入账号和密码'
    return
  }
  try {
    await session.login(username.value, password.value, remember.value)
    password.value = ''
    await router.replace(session.firstPath)
  } catch (failure) {
    error.value = errorText(failure)
  }
}
</script>
<template>
  <main class="login-page">
    <section class="login-story">
      <div class="brand">
        <img src="/brand.svg" alt="Tri Admin" /><span
          >TRI<span class="brand-light">ADMIN</span></span
        >
      </div>
      <div class="story-copy">
        <div class="eyebrow">WORKSPACE, CONNECTED.</div>
        <h1>让管理，<br />回归清晰。</h1>
        <p>连接服务、协作管理、掌握运行状态。<br />在一个专注的工作空间里，处理每一天的工作。</p>
        <div class="story-line"><span></span> 一个入口 · 有序协作</div>
      </div>
      <div class="story-foot">Tri Admin <span>DESKTOP WORKSPACE</span></div>
    </section>
    <section class="login-panel">
      <div class="login-card">
        <div class="eyebrow muted">欢迎回来</div>
        <h2>登录工作空间</h2>
        <p class="subtle">使用你的管理账号继续</p>
        <el-form label-position="top" @submit.prevent="submit"
          ><el-form-item label="账号"
            ><el-input
              v-model="username"
              size="large"
              :prefix-icon="User"
              placeholder="请输入账号"
              autocomplete="username" /></el-form-item
          ><el-form-item label="密码"
            ><el-input
              v-model="password"
              size="large"
              :prefix-icon="Lock"
              type="password"
              show-password
              placeholder="请输入密码"
              autocomplete="off"
          /></el-form-item>
          <div class="login-options">
            <el-checkbox v-model="remember" :disabled="!session.native">保持登录</el-checkbox
            ><span v-if="!session.native" class="tiny subtle">桌面客户端可保存会话</span>
          </div>
          <el-alert
            v-if="error || session.notice"
            :title="error || session.notice"
            type="error"
            :closable="false"
            show-icon
            class="login-error" /><el-button
            class="login-submit"
            size="large"
            type="primary"
            native-type="submit"
            :loading="session.busy"
            :disabled="!session.settings.apiBaseUrl"
            >登录<el-icon><ArrowRight /></el-icon></el-button></el-form
        ><button class="server-link" @click="serverOpen = true">
          <el-icon><Connection /></el-icon><span>{{ serverLabel }}</span
          ><i :class="session.connection" />
        </button>
      </div>
      <div class="login-copyright">Tri Admin · 安全、有序的管理工作空间</div>
    </section>
    <ServerDialog v-model="serverOpen" />
  </main>
</template>
