<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { useSession } from '../stores/session'
import { errorText } from '../ui'
import {
  integrationKinds,
  profilePayload,
  type Integration,
  type IntegrationKind,
  type IntegrationProfile,
  type IntegrationSecret,
} from '../core/settings'

const props = defineProps<{ profile: IntegrationProfile | null }>()
const open = defineModel<boolean>({ required: true })
const emit = defineEmits<{ saved: [] }>()
const session = useSession(),
  saving = ref(false),
  error = ref('')
const form = reactive({
  name: '',
  code: '',
  description: '',
  enabled: false,
  kind: 'http' as IntegrationKind,
  hosts: [{ host: '127.0.0.1', port: 27017 }],
  database: '',
  authDatabase: 'admin',
  replicaSet: '',
  tls: true,
  directConnection: false,
  subdirectory: '',
  endpoint: '',
  bucket: '',
  region: 'us-east-1',
  prefix: '',
  pathStyle: false,
  host: '',
  port: 465,
  security: 'tls' as 'tls' | 'startTls' | 'plain',
  fromAddress: '',
  baseUrl: '',
  secretMode: 'keep' as 'keep' | 'clear' | 'replace',
  username: '',
  password: '',
  accessKeyId: '',
  secretAccessKey: '',
  sessionToken: '',
  headerName: '',
  headerValue: '',
})
function clearSecrets() {
  Object.assign(form, {
    username: '',
    password: '',
    accessKeyId: '',
    secretAccessKey: '',
    sessionToken: '',
    headerName: '',
    headerValue: '',
  })
}
watch(
  () => form.kind,
  () => {
    clearSecrets()
    form.secretMode = 'keep'
  },
)
watch(() => form.secretMode, clearSecrets)
watch(open, (value) => {
  if (!value) clearSecrets()
})
onBeforeUnmount(clearSecrets)
const immutableStorage = computed(
  () => !!props.profile && ['localStorage', 's3'].includes(form.kind),
)
if (props.profile) {
  const row = props.profile,
    config = row.configuration
  Object.assign(form, {
    name: row.name,
    code: row.code,
    description: row.description,
    enabled: row.enabled,
    kind: config.kind,
  })
  if (config.kind === 'mongodb')
    Object.assign(form, {
      ...config,
      hosts: config.hosts.map((host) => ({ ...host })),
      replicaSet: config.replicaSet ?? '',
    })
  else Object.assign(form, config)
}
function configuration(): Integration {
  switch (form.kind) {
    case 'mongodb':
      return {
        kind: form.kind,
        hosts: form.hosts.map((host) => ({ host: host.host, port: host.port })),
        database: form.database,
        authDatabase: form.authDatabase,
        replicaSet: form.replicaSet || null,
        tls: form.tls,
        directConnection: form.directConnection,
      }
    case 'localStorage':
      return { kind: form.kind, subdirectory: form.subdirectory }
    case 's3':
      return {
        kind: form.kind,
        endpoint: form.endpoint,
        bucket: form.bucket,
        region: form.region,
        prefix: form.prefix,
        pathStyle: form.pathStyle,
      }
    case 'smtp':
      return {
        kind: form.kind,
        host: form.host,
        port: form.port,
        security: form.security,
        fromAddress: form.fromAddress,
      }
    case 'http':
      return { kind: form.kind, baseUrl: form.baseUrl }
  }
}
function secret(): IntegrationSecret | null {
  if (form.secretMode !== 'replace') return null
  switch (form.kind) {
    case 'mongodb':
    case 'smtp':
      return { kind: form.kind, username: form.username, password: form.password }
    case 's3':
      return {
        kind: form.kind,
        accessKeyId: form.accessKeyId,
        secretAccessKey: form.secretAccessKey,
        sessionToken: form.sessionToken || null,
      }
    case 'http':
      return { kind: form.kind, headerName: form.headerName, value: form.headerValue }
    case 'localStorage':
      return null
  }
}
async function save() {
  error.value = ''
  try {
    const body = profilePayload(
      {
        name: form.name,
        code: form.code,
        description: form.description,
        enabled: form.enabled,
        configuration: configuration(),
        secretMode: form.secretMode,
        secret: secret(),
      },
      props.profile,
    )
    saving.value = true
    await session.request(
      props.profile ? `/settings/profiles/${props.profile.id}` : '/settings/profiles',
      { method: props.profile ? 'PATCH' : 'POST', body },
    )
    clearSecrets()
    open.value = false
    emit('saved')
  } catch (failure) {
    error.value = errorText(failure)
  } finally {
    saving.value = false
  }
}
</script>
<template>
  <el-dialog
    v-model="open"
    :title="profile ? '编辑集成配置' : '新增集成配置'"
    width="680px"
    :close-on-click-modal="false"
    :close-on-press-escape="!saving"
    :show-close="!saving"
  >
    <el-form label-position="top" :disabled="saving" @submit.prevent="save">
      <el-alert v-if="error" :title="error" type="error" :closable="false" />
      <div class="form-grid">
        <el-form-item label="配置名称"
          ><el-input v-model="form.name" maxlength="80"
        /></el-form-item>
        <el-form-item label="配置标识"
          ><el-input
            v-model="form.code"
            :disabled="!!profile"
            maxlength="64"
            placeholder="小写字母、数字与连字符"
        /></el-form-item>
      </div>
      <el-form-item label="配置类型"
        ><el-select v-model="form.kind" :disabled="!!profile"
          ><el-option
            v-for="kind in integrationKinds"
            :key="kind.value"
            :value="kind.value"
            :label="kind.label" /></el-select
      ></el-form-item>
      <el-form-item label="说明"
        ><el-input v-model="form.description" type="textarea" :rows="2" maxlength="1000"
      /></el-form-item>
      <el-form-item label="启用配置"><el-switch v-model="form.enabled" /></el-form-item>
      <el-alert
        v-if="immutableStorage"
        title="存储位置创建后固定。需要更换时，请新建配置并切换默认存储。"
        type="info"
        :closable="false"
      />
      <template v-if="form.kind === 'mongodb'">
        <div v-for="(host, index) in form.hosts" :key="index" class="form-grid mongo-host-row">
          <el-form-item :label="`主机 ${index + 1}`"
            ><el-input v-model="host.host" placeholder="主机名或 IP，不带协议"
          /></el-form-item>
          <el-form-item :label="`端口 ${index + 1}`"
            ><el-input-number v-model="host.port" :min="1" :max="65535" /><el-button
              v-if="form.hosts.length > 1"
              text
              type="danger"
              @click="form.hosts.splice(index, 1)"
              >移除</el-button
            ></el-form-item
          >
        </div>
        <el-button
          :disabled="form.hosts.length >= 16"
          @click="form.hosts.push({ host: '', port: 27017 })"
          >添加主机</el-button
        >
        <div class="form-grid">
          <el-form-item label="数据库"><el-input v-model="form.database" /></el-form-item>
          <el-form-item label="认证数据库"><el-input v-model="form.authDatabase" /></el-form-item>
        </div>
        <el-form-item label="副本集"
          ><el-input v-model="form.replicaSet" placeholder="可留空"
        /></el-form-item>
        <div class="form-grid">
          <el-form-item label="TLS"><el-switch v-model="form.tls" /></el-form-item
          ><el-form-item label="直连模式"
            ><el-switch v-model="form.directConnection"
          /></el-form-item>
        </div>
      </template>
      <template v-else-if="form.kind === 'localStorage'">
        <el-form-item label="受控子目录"
          ><el-input
            v-model="form.subdirectory"
            :disabled="immutableStorage"
            placeholder="例如 managed，不填写绝对路径"
        /></el-form-item>
        <p class="muted tiny">目录位于服务器启动时设置的存储根下。</p>
      </template>
      <template v-else-if="form.kind === 's3'">
        <el-form-item label="S3 端点"
          ><el-input
            v-model="form.endpoint"
            :disabled="immutableStorage"
            placeholder="https://s3.example.com"
        /></el-form-item>
        <div class="form-grid">
          <el-form-item label="存储桶"
            ><el-input v-model="form.bucket" :disabled="immutableStorage" /></el-form-item
          ><el-form-item label="区域"
            ><el-input v-model="form.region" :disabled="immutableStorage"
          /></el-form-item>
        </div>
        <el-form-item label="对象前缀"
          ><el-input v-model="form.prefix" :disabled="immutableStorage"
        /></el-form-item>
        <el-form-item label="路径样式"
          ><el-switch v-model="form.pathStyle" :disabled="immutableStorage"
        /></el-form-item>
      </template>
      <template v-else-if="form.kind === 'smtp'">
        <div class="form-grid">
          <el-form-item label="SMTP 主机"><el-input v-model="form.host" /></el-form-item
          ><el-form-item label="SMTP 端口"
            ><el-input-number v-model="form.port" :min="1" :max="65535"
          /></el-form-item>
        </div>
        <el-form-item label="传输安全"
          ><el-select v-model="form.security"
            ><el-option value="tls" label="TLS" /><el-option
              value="startTls"
              label="STARTTLS" /><el-option
              value="plain"
              label="明文（仅环回开发认证）" /></el-select
        ></el-form-item>
        <el-form-item label="发件地址"
          ><el-input v-model="form.fromAddress" placeholder="admin@example.com"
        /></el-form-item>
      </template>
      <el-form-item v-else label="基础地址"
        ><el-input v-model="form.baseUrl" placeholder="https://api.example.com"
      /></el-form-item>
      <template v-if="form.kind !== 'localStorage'">
        <el-divider content-position="left">凭据</el-divider>
        <p class="muted tiny">
          {{
            profile?.credentials.configured
              ? `已保存凭据 · 密钥 ${profile.credentials.keyId ?? '未知'}`
              : '当前未配置凭据'
          }}。凭据只写入，页面不读取原值。
        </p>
        <el-form-item label="凭据处理"
          ><el-radio-group v-model="form.secretMode"
            ><el-radio value="keep">{{ profile ? '保留凭据' : '不设置凭据' }}</el-radio
            ><el-radio value="replace">替换凭据</el-radio
            ><el-radio v-if="profile" value="clear">清除凭据</el-radio></el-radio-group
          ></el-form-item
        >
        <template v-if="form.secretMode === 'replace'">
          <template v-if="form.kind === 'mongodb' || form.kind === 'smtp'">
            <el-form-item label="认证用户名"
              ><el-input v-model="form.username" autocomplete="off"
            /></el-form-item>
            <el-form-item label="认证密码"
              ><el-input v-model="form.password" type="password" autocomplete="new-password"
            /></el-form-item>
          </template>
          <template v-else-if="form.kind === 's3'">
            <el-form-item label="Access Key ID"
              ><el-input v-model="form.accessKeyId" autocomplete="off"
            /></el-form-item>
            <el-form-item label="Secret Access Key"
              ><el-input v-model="form.secretAccessKey" type="password" autocomplete="new-password"
            /></el-form-item>
            <el-form-item label="Session Token"
              ><el-input
                v-model="form.sessionToken"
                type="password"
                autocomplete="new-password"
                placeholder="可留空"
            /></el-form-item>
          </template>
          <template v-else>
            <el-form-item label="授权头名称"
              ><el-input v-model="form.headerName" placeholder="例如 X-API-Key" autocomplete="off"
            /></el-form-item>
            <el-form-item label="授权头值"
              ><el-input v-model="form.headerValue" type="password" autocomplete="new-password"
            /></el-form-item>
          </template>
        </template>
        <el-alert
          v-if="form.secretMode === 'clear'"
          title="保存后将移除该配置的凭据。"
          type="warning"
          :closable="false"
        />
      </template>
    </el-form>
    <template #footer
      ><el-button :disabled="saving" @click="open = false">取消</el-button
      ><el-button type="primary" :loading="saving" @click="save">保存配置</el-button></template
    >
  </el-dialog>
</template>
<style scoped>
.form-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: 16px;
}
</style>
