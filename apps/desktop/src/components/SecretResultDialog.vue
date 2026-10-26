<script setup lang="ts">
import { computed } from 'vue'
import { ElMessage } from 'element-plus'
const props = defineProps<{
    modelValue: { kind: 'token' | 'application'; id: string; value: string } | null
  }>(),
  emit = defineEmits<{ (event: 'update:modelValue', value: null): void }>()
const open = computed({
  get: () => props.modelValue !== null,
  set: (value) => {
    if (!value) emit('update:modelValue', null)
  },
})
async function copy() {
  const value = props.modelValue?.value
  if (!value) return
  try {
    await navigator.clipboard.writeText(value)
    ElMessage.success('已复制')
  } catch {
    ElMessage.warning('复制不可用，请手动选择并保存')
  }
}
</script>
<template>
  <el-dialog
    v-model="open"
    title="一次性凭据"
    width="min(680px,94vw)"
    :close-on-click-modal="false"
    destroy-on-close
    ><el-alert
      title="仅本次展示。关闭后不能取回原秘密；请保存到你的凭据管理器。"
      type="warning"
      :closable="false"
    />
    <p class="subtle">
      {{ modelValue?.kind === 'token' ? 'Token' : '应用' }} ID：{{ modelValue?.id }}
    </p>
    <el-input
      :model-value="modelValue?.value ?? ''"
      type="textarea"
      :rows="3"
      readonly
      aria-label="一次性凭据"
    /><template #footer
      ><el-button @click="copy">复制凭据</el-button
      ><el-button type="primary" @click="open = false">我已保存，关闭</el-button></template
    ></el-dialog
  >
</template>
