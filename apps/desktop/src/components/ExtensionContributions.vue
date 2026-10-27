<script setup lang="ts">
import { pluginToolName, type PackageInfo } from '../core/extensions'
const props = defineProps<{
  extensionId: string
  revisionId: string
  packageInfo: PackageInfo
  active: boolean
}>()
</script>
<template>
  <el-collapse v-if="packageInfo.kind === 'plugin'" class="extension-contributions">
    <el-collapse-item title="贡献能力" name="contributions">
      <p class="subtle">
        以下内容来自选定的固定修订；文件草稿保存为新修订后才会更新。安装和启用不会执行工具或启动程序。
      </p>
      <el-tag :type="active ? 'success' : 'info'">{{
        active ? '选定修订已启用' : '选定修订未启用'
      }}</el-tag>
      <p class="subtle">
        实际调用仍检查服务器当前状态、权限和范围。请在 MCP连接中管理连接与调用记录。
      </p>
      <h4>业务工具 · {{ packageInfo.tools.length }}</h4>
      <div v-for="tool in packageInfo.tools" :key="tool.name" class="contribution-entry">
        <strong>{{ tool.name }}</strong>
        <p>{{ tool.description }}</p>
        <small>业务操作 {{ tool.operation }}</small>
        <code>{{ pluginToolName(props.extensionId, props.revisionId, tool.name) }}</code>
        <details>
          <summary>输入格式与字段映射</summary>
          <pre>{{
            JSON.stringify({ inputSchema: tool.inputSchema, mappings: tool.mappings }, null, 2)
          }}</pre>
        </details>
      </div>
      <el-empty
        v-if="!packageInfo.tools.length"
        description="本修订没有业务工具绑定"
        :image-size="40"
      />
      <h4>MCP 程序来源 · {{ packageInfo.mcp.length }}</h4>
      <div v-for="source in packageInfo.mcp" :key="source.id" class="contribution-entry">
        <strong>{{ source.label }}</strong>
        <p>贡献 {{ source.id }} · 部署能力 {{ source.capabilityId }}</p>
        <small>批准工具：{{ source.tools.join(' · ') || '无' }}</small>
        <p class="subtle">
          选择此插件来源并保存连接后，仍需显式启动；启停版本或修订改变会使旧连接失效。
        </p>
      </div>
      <el-empty
        v-if="!packageInfo.mcp.length"
        description="本修订没有 MCP 程序来源"
        :image-size="40"
      />
    </el-collapse-item>
  </el-collapse>
</template>
<style scoped>
.extension-contributions {
  margin: 16px 0;
}
.contribution-entry {
  padding: 14px 0;
  border-bottom: 1px solid var(--el-border-color-lighter);
  overflow-wrap: anywhere;
}
.contribution-entry code {
  display: block;
  padding: 10px 0;
  font-size: 11px;
  overflow-wrap: anywhere;
}
.contribution-entry pre {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  max-height: 280px;
  overflow: auto;
  padding: 14px;
  background: var(--el-fill-color-light);
}
</style>
