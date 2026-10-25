<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useSession } from '../stores/session'
import { errorText } from '../ui'
import type { RuntimeSettings } from '../core/settings'
const session = useSession(),
  runtime = ref<RuntimeSettings | null>(null),
  loading = ref(false),
  error = ref('')
async function load() {
  loading.value = true
  error.value = ''
  try {
    runtime.value = (await session.request<RuntimeSettings>('/settings/runtime')).data
  } catch (failure) {
    error.value = errorText(failure)
  } finally {
    loading.value = false
  }
}
onMounted(() => void load())
</script>
<template>
  <section class="surface settings-panel runtime-settings" v-loading="loading">
    <div class="section-heading">
      <h3>启动状态</h3>
      <el-button text @click="load">刷新状态</el-button>
    </div>
    <p class="muted">以下设置由服务器部署环境提供，修改后需要重启服务。</p>
    <el-alert v-if="error" :title="error" type="error" :closable="false" />
    <el-descriptions v-if="runtime" :column="1" border>
      <el-descriptions-item label="监听地址">{{ runtime.configuredBind }}</el-descriptions-item>
      <el-descriptions-item label="主数据库"
        >{{ runtime.databaseName ?? '未配置' }} ·
        {{ runtime.databaseConfigured ? '已配置' : '未配置' }}</el-descriptions-item
      >
      <el-descriptions-item label="允许的来源"
        ><div v-for="origin in runtime.corsOrigins" :key="origin">
          {{ origin }}
        </div></el-descriptions-item
      >
      <el-descriptions-item label="存储根">{{ runtime.storageRoot }}</el-descriptions-item>
      <el-descriptions-item label="访问令牌有效期"
        >{{ runtime.accessTokenSeconds }} 秒</el-descriptions-item
      >
      <el-descriptions-item label="会话有效期">{{ runtime.sessionDays }} 天</el-descriptions-item>
      <el-descriptions-item label="启动登录上限"
        >{{ runtime.bootLoginLimitPerIp }} 次 / IP / 分钟</el-descriptions-item
      >
      <el-descriptions-item label="凭据加密">{{
        runtime.settingsKeysConfigured ? '已配置' : '未配置'
      }}</el-descriptions-item>
      <el-descriptions-item label="活动加密密钥">{{
        runtime.settingsActiveKeyId ?? '未配置'
      }}</el-descriptions-item>
      <el-descriptions-item label="可用解密密钥标识">{{
        runtime.settingsDecryptionKeyIds.join('、') || '无'
      }}</el-descriptions-item>
    </el-descriptions>
  </section>
</template>
