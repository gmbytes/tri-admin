<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { useSession } from '../stores/session'
import { dateText, errorText } from '../ui'
import { servicePayload, type ServiceSettings } from '../core/settings'
import IntegrationBindingSelect from './IntegrationBindingSelect.vue'
const session = useSession(),
  original = ref<ServiceSettings | null>(null),
  draft = ref<ServiceSettings | null>(null)
const loading = ref(false),
  saving = ref(false),
  error = ref(''),
  useBootLimit = ref(true)
let sequence = 0
async function load() {
  const request = ++sequence
  loading.value = true
  error.value = ''
  try {
    const result = (await session.request<ServiceSettings>('/settings/service')).data
    if (request !== sequence) return
    original.value = result
    draft.value = { ...result }
    useBootLimit.value = result.loginLimitPerIpOverride === null
  } catch (failure) {
    if (request === sequence) error.value = errorText(failure)
  } finally {
    if (request === sequence) loading.value = false
  }
}
function changeLimitMode(value: boolean | string | number) {
  if (draft.value)
    draft.value.loginLimitPerIpOverride = value
      ? null
      : (original.value?.effectiveLoginLimitPerIp ?? 100)
}
async function save() {
  if (!draft.value || !original.value) return
  error.value = ''
  saving.value = true
  try {
    const body = servicePayload(draft.value, original.value)
    const result = (
      await session.request<ServiceSettings>('/settings/service', { method: 'PATCH', body })
    ).data
    original.value = result
    draft.value = { ...result }
    await session.loadApplication()
    ElMessage.success('服务设置已保存')
  } catch (failure) {
    error.value = errorText(failure)
  } finally {
    saving.value = false
  }
}
onMounted(() => void load())
</script>
<template>
  <section class="surface settings-panel" v-loading="loading">
    <div class="section-heading">
      <h3>服务设置</h3>
      <el-button :disabled="saving" text @click="load">重新读取</el-button>
    </div>
    <el-alert v-if="error" :title="error" type="error" :closable="false" />
    <el-form
      v-if="draft && original"
      label-position="top"
      class="settings-form"
      @submit.prevent="save"
    >
      <el-form-item label="应用名称"
        ><el-input
          v-model="draft.displayName"
          :disabled="!session.can('settings.write') || saving"
          maxlength="80"
      /></el-form-item>
      <el-form-item label="登录频率策略"
        ><el-switch
          v-model="useBootLimit"
          active-text="使用启动默认值"
          :disabled="!session.can('settings.write') || saving"
          @change="changeLimitMode"
      /></el-form-item>
      <el-form-item v-if="!useBootLimit" label="每 IP 每分钟登录上限"
        ><el-input-number
          v-model="draft.loginLimitPerIpOverride"
          :min="1"
          :max="10000"
          :disabled="!session.can('settings.write') || saving"
      /></el-form-item>
      <p class="muted tiny">
        当前生效上限：{{ original.effectiveLoginLimitPerIp }} 次 / IP /
        分钟。保存后对下一次登录生效。
      </p>
      <el-form-item label="默认存储"
        ><IntegrationBindingSelect
          :key="`storage-${original.version}`"
          v-model="draft.storageProfileId"
          :kinds="['localStorage', 's3']"
          :disabled="!session.can('settings.write') || saving"
      /></el-form-item>
      <el-form-item label="默认邮件配置"
        ><IntegrationBindingSelect
          :key="`mail-${original.version}`"
          v-model="draft.mailProfileId"
          :kinds="['smtp']"
          :disabled="!session.can('settings.write') || saving"
      /></el-form-item>
      <p class="muted tiny">
        只能绑定已启用且类型正确的配置。版本 {{ original.version }} · 更新于
        {{ dateText(original.updatedAt) }}
      </p>
      <el-button v-if="session.can('settings.write')" type="primary" :loading="saving" @click="save"
        >保存服务设置</el-button
      >
    </el-form>
  </section>
</template>
