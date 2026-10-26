<script setup lang="ts">
defineProps<{ ips: string; minuteLimit: number; dailyQuota: number }>()
const emit = defineEmits<{
  (event: 'update:ips', value: string): void
  (event: 'update:minuteLimit', value: number): void
  (event: 'update:dailyQuota', value: number): void
}>()
</script>
<template>
  <el-form-item label="来源 CIDR"
    ><el-input
      :model-value="ips"
      type="textarea"
      :rows="2"
      placeholder="每行一条，例如 192.0.2.0/24；留空不限制来源"
      @update:model-value="emit('update:ips', $event)"
  /></el-form-item>
  <div class="form-grid">
    <el-form-item label="每分钟限额"
      ><el-input-number
        :model-value="minuteLimit"
        :min="1"
        :max="10000"
        @update:model-value="emit('update:minuteLimit', $event ?? 0)" /></el-form-item
    ><el-form-item label="每日配额"
      ><el-input-number
        :model-value="dailyQuota"
        :min="1"
        :max="10000000"
        @update:model-value="emit('update:dailyQuota', $event ?? 0)"
    /></el-form-item>
  </div>
</template>
