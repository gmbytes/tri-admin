# MCP 部署程序与后台接口

后台 STDIO 连接引用部署批准的能力 ID。先在受保护的配置目录准备 JSON，再设置 `TRI_MCP_PROGRAMS_FILE` 并重启服务。程序必须已经安装，路径和工作目录必须存在且为绝对路径。不要把带凭据的本地配置提交到仓库。

```json
{
  "formatVersion": 1,
  "programs": [
    {
      "id": "records",
      "label": "记录服务",
      "program": "C:\\Program Files\\nodejs\\node.exe",
      "arguments": ["E:\\mcp-services\\records\\server.mjs"],
      "workingDirectory": "E:\\mcp-services\\records",
      "environment": {"SystemRoot": "C:\\Windows"},
      "allowedTools": ["records.list", "records.update"],
      "startupTimeoutMs": 10000,
      "requestTimeoutMs": 30000,
      "stopTimeoutMs": 5000
    }
  ]
}
```

示例路径需替换为实际批准的程序；该程序必须支持 2026-07-28 的 server/discover 生命周期。配置最多 64 KiB、16 项能力，未知字段或无效限制会阻止服务启动。环境清空后仅传显式映射，Windows 网络程序通常需要 SystemRoot。后台公开目录仅返回 ID、标签和工具白名单，不能通过 HTTP 添加命令、路径、参数或环境。目录配置不是操作系统沙箱，程序本身由部署者信任。

创建连接使用 `POST /api/v1/mcp/connections`：

```json
{
  "requestId": "00000000-0000-4000-8000-000000000001",
  "name": "记录连接",
  "transport": {"kind": "stdio", "capabilityId": "records"},
  "enabled": true
}
```

随后向 `/connections/{id}/actions` 提交 requestId、当前 version 和 kind（start/stop/restart/refresh）。首次返回 202 操作记录，重放同一操作者/UUID/输入返回原记录。轮询 `/api/v1/mcp/actions/{actionId}`，或按 `/actions/by-request/{requestId}` 恢复；不要因响应丢失换 UUID 重做。重启先等待原实例退出。停止可以打断启动，未确认退出的实例不允许再次启动。

`GET /connections/{id}/tools` 返回最后一次实际发现清单与部署白名单的交集，不自动访问上游。显式 refresh 在记录意图和审计后重新发现，保留配置版本和运行实例；同一 UUID 重放不会再次发现。调用 `/connections/{id}/calls` 提供 requestId、version、name 和 arguments 对象，调用需要 mcp.read + mcp.invoke；生命周期管理需要 mcp.read + mcp.write。所有连接及历史查询应用当前 all/self/roles 范围，事务中再次核对 JWT/PAT、角色和 Scope。

调用结果加密保存，原调用者在当前范围内使用 `/actions/{id}/result` 读取 content、size、sha256 和 kind。content 是原 JSON 的 UTF-8 文本，以保留数值及字节摘要；其他读取者只能看到安全元数据。最大结果为 512 KiB，结果载荷保留 30 天，原操作身份不会因清理或同名新连接而改绑。

`POST /actions/{id}/cancel` 提供新的 requestId 记录取消意图。取消、超时、断线和进程停止可能已经产生远端效果，原调用会保留 unknown；不会自动重发。发送前取消或并发忙碌可明确标注未发送。中间输入请求和远端 task 只保存响应，不自动驱动后续轮次。

服务租约失效后标为 orphaned，其他服务实例不会凭旧 PID 终止或再启动程序。运维人员先在宿主确认遗留程序已经清理，再通过交互会话向 `/connections/{id}/confirm-orphan` 提交 requestId、version、confirmed=true 和 reason；要求全部数据范围与 mcp.write，PAT 不能执行。该操作记录人工确认，不伪装成实际观察到的退出证明。

`admin-api maintain-mcp` 执行一次有界租约维护；serve 会周期维护并在关闭时停止自身持有的程序。当前共享 mcp v4 的运行资料包含 transportKind 和 releaseConfirmed，STDIO 仍需真实进程退出确认；v4 接入插件来源并保留已有 STDIO/HTTP 绑定、历史和定制菜单，旧二进制不能按旧模型继续就绪。HTTP 持久引用与业务服务见 [HTTP MCP 设计](design-mcp-http.md) 和 [业务服务设计](design-mcp-server.md)。

## 插件来源

在「Skills 与插件」安装并检查插件，启用时会验证 MCP 贡献是否只引用已部署的 capabilityId 和批准工具子集，不会启动程序。管理文件中的「贡献能力」展示选定固定修订的业务工具调用名称、Schema/映射和 MCP 来源，历史名称不会转向同名新包或新修订。

创建 MCP 连接时选择「插件 MCP」和「选择插件来源」。来源目录支持插件名称前缀搜索及分页，展示内容修订、记录版本、部署能力 ID 和批准工具；没有当前读取权限的来源不会进入计数或页面。底层依然是 STDIO，运行后显示真实 PID；没有 caller 提交的程序、参数、环境或工具扩展字段。

```json
{
  "requestId": "00000000-0000-4000-8000-000000000001",
  "name": "插件记录连接",
  "enabled": true,
  "transport": {
    "kind": "plugin",
    "extensionId": "000000000000000000000001",
    "revisionId": "000000000000000000000002",
    "contributionId": "records"
  }
}
```

示例 ID 需替换为当前已启用插件的实际值。`GET /api/v1/mcp/plugin-capabilities` 使用 page/pageSize/search/sort（name 或 createdAt）分页，要求当前 mcp.read + skills.read；创建/启动/调用同时按 MCP 权限及插件所有者范围复核。工具只取贡献声明与部署批准清单的子集，空声明表示没有工具。

来源指纹包含固定修订、启停记录版本和部署配置。停用、卸载或换修订会关闭本服务自身持有的旧进程；快速重新启用也不会继续沿用旧绑定。先等待实际退出，再编辑连接重新选择当前来源并保存，然后显式启动。旧 UUID 和历史结果继续指向原操作，不会重新执行。失去插件读取权限后仍可按当前 MCP 写权限停止已有实例。

## 管理页面

进入「MCP连接」，创建连接时选择部署批准的 STDIO 能力、插件固定来源，或已保存的 HTTP 配置及其精确版本。HTTP 绑定需要配置管理权限；密钥和真实程序路径不出现在连接表单。允许连接后仍需在「管理」中点击「启动」，保存本身不执行工具或建立连接。

管理抽屉提供停止、重启、显式刷新、工具清单和操作历史。调用前可查看工具输入格式，填写 JSON 对象；数值原文按原始 JSON 提交。结果只在当前授权的原调用者窗口中读取并核验 UTF-8 大小和 SHA-256，不写入浏览器存储。工具报错、待输入、远端任务和效果未确认分别显示；不会把收到响应当作业务成功，也不会自动驱动远端任务。

响应丢失时，「待确认 MCP 操作」只保存服务器、账号隔离的 UUID 和操作元数据，使用「恢复查询」读取原回执。不要新建调用重做。取消已发送请求不能证明远端效果撤销，需核对上游状态。记录版本冲突保留本机草稿，使用「重新载入记录」查看当前版本后再决定修改。

「已删除」分页仍可查看当前范围内的连接元数据和操作历史，不提供启动和工具调用。列表接受 status=active/deleted；读取已删除连接不会恢复连接或改变原操作身份。
