<script setup lang="ts">
import { computed } from 'vue'
import type { Integration } from '../core/settings'
const props = defineProps<{ configuration: Integration }>()
const fields = computed(() => {
  const value = props.configuration
  switch (value.kind) {
    case 'mongodb':
      return {
        主机: value.hosts.map((host) => `${host.host}:${host.port}`).join('、'),
        数据库: value.database,
        认证数据库: value.authDatabase,
        副本集: value.replicaSet ?? '无',
        TLS: value.tls ? '启用' : '关闭',
        直连: value.directConnection ? '启用' : '关闭',
      }
    case 'localStorage':
      return { 受控子目录: value.subdirectory }
    case 's3':
      return {
        端点: value.endpoint,
        存储桶: value.bucket,
        区域: value.region,
        对象前缀: value.prefix || '无',
        路径样式: value.pathStyle ? '启用' : '关闭',
      }
    case 'smtp':
      return {
        主机: value.host,
        端口: value.port,
        安全方式: value.security,
        发件地址: value.fromAddress,
      }
    case 'http':
      return { 基础地址: value.baseUrl }
  }
})
</script>
<template>
  <el-descriptions :column="1" border
    ><el-descriptions-item v-for="(value, key) in fields" :key="key" :label="key">{{
      value
    }}</el-descriptions-item></el-descriptions
  >
</template>
