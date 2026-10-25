<script setup lang="ts">
import { ref } from 'vue'
import ServiceSettingsPanel from '../components/ServiceSettingsPanel.vue'
import IntegrationProfilesPanel from '../components/IntegrationProfilesPanel.vue'
import RuntimeSettingsPanel from '../components/RuntimeSettingsPanel.vue'
const tab = ref('service')
</script>
<template>
  <div class="page-heading">
    <div>
      <div class="eyebrow muted">SERVICE CONFIGURATION</div>
      <h1>配置中心</h1>
      <p>维护服务显示、默认集成和连接配置，查看服务器启动状态。</p>
    </div>
  </div>
  <el-tabs v-model="tab"
    ><el-tab-pane label="服务设置" name="service" /><el-tab-pane
      label="集成配置"
      name="profiles" /><el-tab-pane label="启动状态" name="runtime"
  /></el-tabs>
  <ServiceSettingsPanel v-if="tab === 'service'" /><IntegrationProfilesPanel
    v-else-if="tab === 'profiles'"
  /><RuntimeSettingsPanel v-else />
</template>
<style>
.settings-panel {
  padding: 24px;
}
.settings-form {
  max-width: 650px;
}
.settings-panel .section-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 18px;
}
.settings-panel .section-heading h3 {
  margin: 0;
}
.settings-panel .el-alert {
  margin-bottom: 18px;
}
.settings-panel .el-select {
  width: 100%;
}
.runtime-settings .el-descriptions__content {
  overflow-wrap: anywhere;
}
.field-error {
  color: var(--el-color-danger);
}
.mongo-host-row {
  margin-top: 12px;
}
</style>
