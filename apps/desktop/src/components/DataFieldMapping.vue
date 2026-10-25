<script setup lang="ts">
import { useId } from 'vue'
import type { DataResource } from '../core/data-tools'
const props = defineProps<{
  resource: DataResource
  headers: readonly string[]
  modelValue: Record<string, string>
  labels: Record<string, string>
  disabled?: boolean
}>()
const emit = defineEmits<{ 'update:modelValue': [Record<string, string>] }>(),
  instance = useId()
function update(field: string, value: unknown) {
  emit('update:modelValue', {
    ...props.modelValue,
    [field]: typeof value === 'string' ? value : '',
  })
}
function used(header: string, field: string) {
  return Object.entries(props.modelValue).some(
    ([other, value]) => other !== field && value === header,
  )
}
</script>
<template>
  <div class="data-field-mapping" role="group" aria-label="字段映射">
    <div v-for="field in resource.fields" :key="field" class="data-field-mapping-row">
      <label :for="`${instance}-${field}`"
        ><span>{{ labels[field] ?? field }}</span
        ><span v-if="resource.requiredFields.includes(field)" class="required-mark" title="必填">
          *</span
        ><span class="tiny subtle"> {{ field }}</span></label
      >
      <el-select
        :id="`${instance}-${field}`"
        :model-value="modelValue[field] ?? ''"
        :aria-label="`${labels[field] ?? field} 来源列`"
        :disabled="disabled"
        clearable
        placeholder="选择来源列；可选字段可留空"
        @update:model-value="(value) => update(field, value)"
        ><el-option
          v-for="header in headers"
          :key="header"
          :label="header"
          :value="header"
          :disabled="used(header, field)"
      /></el-select>
    </div>
  </div>
</template>
<style scoped>
.data-field-mapping {
  display: grid;
  gap: 12px;
  margin: 18px 0;
}
.data-field-mapping-row {
  display: grid;
  grid-template-columns: minmax(160px, 1fr) minmax(240px, 2fr);
  gap: 20px;
  align-items: center;
}
.data-field-mapping-row .el-select {
  width: 100%;
}
.required-mark {
  color: var(--el-color-danger);
}
@media (max-width: 800px) {
  .data-field-mapping-row {
    grid-template-columns: 1fr;
    gap: 8px;
  }
}
</style>
