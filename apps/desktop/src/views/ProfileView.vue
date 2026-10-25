<script setup lang="ts">
import { ref } from 'vue'
import { useSession } from '../stores/session'
import { errorText, scopeText } from '../ui'
import { ElMessage } from 'element-plus'
const session = useSession(),
  current = ref(''),
  password = ref(''),
  confirm = ref(''),
  error = ref(''),
  busy = ref(false)
async function save() {
  error.value = ''
  if (password.value !== confirm.value) {
    error.value = '两次新密码不一致'
    return
  }
  if (password.value.length < 12) {
    error.value = '新密码至少需要 12 个字符'
    return
  }
  busy.value = true
  try {
    await session.request('/auth/password', {
      method: 'POST',
      body: { currentPassword: current.value, newPassword: password.value },
    })
    await session.passwordChanged()
    ElMessage.success('密码已修改，请重新登录')
  } catch (failure) {
    error.value = errorText(failure)
  } finally {
    busy.value = false
  }
}
</script>
<template>
  <div class="page-heading">
    <div>
      <h1>个人中心</h1>
      <p>查看当前账号，维护你的登录凭证。</p>
    </div>
  </div>
  <div class="profile-grid">
    <section class="surface profile-summary">
      <div class="profile-avatar">{{ session.user?.displayName?.slice(0, 1) }}</div>
      <h2>{{ session.user?.displayName }}</h2>
      <p class="subtle">{{ session.user?.username }}</p>
      <div class="info-list">
        <span
          >数据范围<b>{{ scopeText(session.user?.dataScope) }}</b></span
        ><span
          >授权能力<b>{{
            session.user?.permissions.includes('*')
              ? '系统 Owner'
              : session.user?.permissions.length + ' 项'
          }}</b></span
        >
      </div>
    </section>
    <section class="surface">
      <h3>修改密码</h3>
      <p class="subtle">修改成功后，你的全部登录会话将退出。</p>
      <el-form label-position="top" class="password-form" @submit.prevent="save"
        ><el-form-item label="当前密码"
          ><el-input
            v-model="current"
            type="password"
            show-password
            autocomplete="off" /></el-form-item
        ><el-form-item label="新密码"
          ><el-input
            v-model="password"
            type="password"
            show-password
            placeholder="至少 12 个字符"
            autocomplete="off" /></el-form-item
        ><el-form-item label="确认新密码"
          ><el-input
            v-model="confirm"
            type="password"
            show-password
            autocomplete="off" /></el-form-item
        ><el-alert v-if="error" :title="error" type="error" :closable="false" /><el-button
          type="primary"
          native-type="submit"
          :loading="busy"
          >修改密码</el-button
        ></el-form
      >
    </section>
  </div>
</template>
