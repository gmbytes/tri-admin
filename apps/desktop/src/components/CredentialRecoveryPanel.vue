<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ElMessageBox, ElMessage } from 'element-plus'
import { errorText } from '../ui'
import type { CredentialOperation } from '../core/open-access'
const props = defineProps<{
    scope: string
    operations: CredentialOperation[]
    error: string
    recover: (row: CredentialOperation) => Promise<string>
    forget: (row: CredentialOperation) => void
  }>(),
  busy = ref(''),
  failure = ref('')
const labels = {
    'token.create': '签发 Token',
    'app.create': '创建应用',
    'app.rotate': '轮换应用密钥',
  },
  visible = computed(() => !!props.error || !!props.operations.length)
watch(
  () => props.scope,
  () => {
    busy.value = ''
    failure.value = ''
  },
  { flush: 'sync' },
)
async function recover(row: CredentialOperation) {
  const scope = props.scope
  busy.value = row.requestId
  failure.value = ''
  try {
    const message = await props.recover(row)
    if (scope === props.scope && message) ElMessage.success(message)
  } catch (error) {
    if (scope === props.scope) failure.value = errorText(error)
  } finally {
    if (scope === props.scope) busy.value = ''
  }
}
async function forget(row: CredentialOperation) {
  const scope = props.scope
  try {
    await ElMessageBox.confirm(
      '只移除本机标记，服务器中的凭据或操作不会撤销。确认前请核对请求标识。',
      '移除恢复标记',
      { confirmButtonText: '移除标记', cancelButtonText: '取消', type: 'warning' },
    )
    if (scope === props.scope) props.forget(row)
  } catch {}
}
</script>
<template>
  <section v-if="visible" class="surface recovery-panel">
    <el-alert v-if="error" :title="error" type="error" :closable="false" />
    <h3 v-if="operations.length">待确认的凭据操作</h3>
    <p v-if="operations.length" class="subtle">
      响应丢失时先查询，不能自动重新签发或轮换。恢复查询不会取回秘密。
    </p>
    <el-alert v-if="failure" :title="failure" type="error" :closable="false" />
    <div v-for="row in operations" :key="row.requestId" class="recovery-row">
      <div>
        <strong>{{ labels[row.kind] }}</strong
        ><code class="table-secondary">{{ row.requestId }}</code>
      </div>
      <el-button :loading="busy === row.requestId" @click="recover(row)">恢复查询</el-button
      ><el-button text @click="forget(row)">移除本机标记</el-button>
    </div>
  </section>
</template>
<style scoped>
.recovery-panel {
  padding: 20px;
  margin-bottom: 20px;
}
.recovery-row {
  display: flex;
  gap: 12px;
  align-items: center;
  padding: 12px 0;
  border-top: 1px solid var(--el-border-color-lighter);
}
.recovery-row > div {
  flex: 1;
  min-width: 0;
}
.recovery-row code {
  overflow-wrap: anywhere;
}
</style>
