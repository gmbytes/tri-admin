<script setup lang="ts">
import type { DataFilter, FilterOp, QueryField, QuerySchema } from '../core/export-templates'
const props = defineProps<{
    modelValue: DataFilter[]
    schema: QuerySchema | null
    disabled?: boolean
  }>(),
  emit = defineEmits<{ 'update:modelValue': [DataFilter[]] }>()
const labels: Record<FilterOp, string> = {
  eq: '等于',
  prefix: '开头为',
  gt: '大于',
  gte: '大于等于',
  lt: '小于',
  lte: '小于等于',
  in: '属于列表',
}
function field(value: DataFilter) {
  return props.schema?.fields.find((field) => field.field === value.field)
}
function initial(field: QueryField | undefined): unknown {
  return field?.kind === 'boolean'
    ? true
    : field?.kind === 'integer'
      ? 0
      : field?.kind === 'choice'
        ? (field.choices[0] ?? '')
        : field?.kind === 'timestamp'
          ? new Date().toISOString()
          : ''
}
function update(index: number, value: Partial<DataFilter>) {
  emit(
    'update:modelValue',
    props.modelValue.map((row, i) => (i === index ? { ...row, ...value } : row)),
  )
}
function changeField(index: number, key: string) {
  const current = props.schema?.fields.find((field) => field.field === key)
  update(index, { field: key, op: current?.operators[0] ?? 'eq', value: initial(current) })
}
function changeOp(index: number, op: FilterOp) {
  update(index, { op, value: op === 'in' ? [] : initial(field(props.modelValue[index])) })
}
function lines(index: number, value: string) {
  const current = field(props.modelValue[index])
  update(index, {
    value: value
      .split('\n')
      .map((v) => v.trim())
      .filter(Boolean)
      .map((v) => (current?.kind === 'integer' ? Number(v) : v)),
  })
}
function date(index: number, value: unknown) {
  update(index, { value: value instanceof Date ? value.toISOString() : '' })
}
function add() {
  const current = props.schema?.fields[0]
  if (current)
    emit('update:modelValue', [
      ...props.modelValue,
      { field: current.field, op: current.operators[0], value: initial(current) },
    ])
}
</script>
<template>
  <div class="template-filters">
    <div v-for="(row, index) in modelValue" :key="index" class="template-filter-row">
      <el-select
        :model-value="row.field"
        :aria-label="`筛选字段 ${index + 1}`"
        :disabled="disabled || !schema"
        @update:model-value="(value) => changeField(index, String(value))"
        ><el-option
          v-for="item in schema?.fields ?? []"
          :key="item.field"
          :label="item.label"
          :value="item.field"
      /></el-select>
      <el-select
        :model-value="row.op"
        :aria-label="`筛选操作 ${index + 1}`"
        :disabled="disabled || !schema"
        @update:model-value="(value) => changeOp(index, value as FilterOp)"
        ><el-option
          v-for="operation in field(row)?.operators ?? []"
          :key="operation"
          :label="labels[operation]"
          :value="operation"
      /></el-select>
      <el-select
        v-if="field(row)?.kind === 'choice'"
        :model-value="row.value as string | string[]"
        :multiple="row.op === 'in'"
        :aria-label="`筛选值 ${index + 1}`"
        :disabled="disabled"
        @update:model-value="(value) => update(index, { value })"
        ><el-option
          v-for="value in field(row)?.choices ?? []"
          :key="value"
          :label="value"
          :value="value"
      /></el-select>
      <el-input
        v-else-if="row.op === 'in'"
        :model-value="Array.isArray(row.value) ? row.value.join('\n') : ''"
        type="textarea"
        :rows="2"
        :aria-label="`筛选值 ${index + 1}`"
        placeholder="每行一个值，最多32项"
        :disabled="disabled"
        @update:model-value="(value) => lines(index, value)"
      />
      <el-select
        v-else-if="field(row)?.kind === 'boolean'"
        :model-value="row.value as boolean"
        :aria-label="`筛选值 ${index + 1}`"
        :disabled="disabled"
        @update:model-value="(value) => update(index, { value })"
        ><el-option label="是" :value="true" /><el-option label="否" :value="false"
      /></el-select>
      <el-input-number
        v-else-if="field(row)?.kind === 'integer'"
        :model-value="typeof row.value === 'number' ? row.value : undefined"
        :aria-label="`筛选值 ${index + 1}`"
        :precision="0"
        :step="1"
        :disabled="disabled"
        @update:model-value="(value) => update(index, { value: value ?? '' })"
      />
      <el-date-picker
        v-else-if="field(row)?.kind === 'timestamp'"
        :model-value="typeof row.value === 'string' && row.value ? new Date(row.value) : null"
        type="datetime"
        :aria-label="`筛选值 ${index + 1}`"
        :disabled="disabled"
        @update:model-value="(value: unknown) => date(index, value)"
      />
      <el-input
        v-else
        :model-value="typeof row.value === 'string' ? row.value : ''"
        :aria-label="`筛选值 ${index + 1}`"
        :disabled="disabled"
        maxlength="255"
        @update:model-value="(value) => update(index, { value })"
      />
      <el-button
        text
        type="danger"
        :disabled="disabled"
        :aria-label="`移除筛选 ${index + 1}`"
        @click="
          emit(
            'update:modelValue',
            modelValue.filter((_, i) => i !== index),
          )
        "
        >移除</el-button
      >
    </div>
    <el-button :disabled="disabled || !schema || modelValue.length >= 8" @click="add"
      >添加筛选</el-button
    >
    <p class="tiny subtle">所有条件同时满足；账号数据范围会由服务器继续限制。</p>
  </div>
</template>
<style scoped>
.template-filter-row {
  display: grid;
  grid-template-columns: 1fr 1fr 1.6fr 70px;
  gap: 10px;
  margin-bottom: 10px;
  align-items: start;
}
.template-filter-row .el-select,
.template-filter-row .el-input-number,
.template-filter-row .el-date-editor {
  width: 100%;
}
@media (max-width: 850px) {
  .template-filter-row {
    grid-template-columns: 1fr 1fr;
  }
  .template-filter-row > :nth-child(3) {
    grid-column: 1;
  }
}
</style>
