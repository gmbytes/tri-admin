<script setup lang="ts">
import { useSession } from './stores/session'
import zhCn from 'element-plus/es/locale/lang/zh-cn'
const session = useSession()
</script>
<template>
  <el-config-provider :locale="zhCn"
    ><div v-if="session.initializing" class="boot-screen">
      <img src="/brand.svg" alt="Tri Admin" />
      <p>正在准备工作空间…</p>
    </div>
    <RouterView v-else
  /></el-config-provider>
</template>
