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

随后向 `/connections/{id}/actions` 提交 requestId、当前 version 和 kind（start/stop/restart）。首次返回 202 操作记录，重放同一操作者/UUID/输入返回原记录。轮询 `/api/v1/mcp/actions/{actionId}`，或按 `/actions/by-request/{requestId}` 恢复；不要因响应丢失换 UUID 重做。重启先等待原实例退出。停止可以打断启动，未确认退出的实例不允许再次启动。

`GET /connections/{id}/tools` 返回实际发现清单与部署白名单的交集。调用 `/connections/{id}/calls` 提供 requestId、version、name 和 arguments 对象，调用需要 mcp.read + mcp.invoke；生命周期管理需要 mcp.read + mcp.write。所有连接及历史查询应用当前 all/self/roles 范围，事务中再次核对 JWT/PAT、角色和 Scope。

调用结果加密保存，原调用者在当前范围内使用 `/actions/{id}/result` 读取 content、size、sha256 和 kind。content 是原 JSON 的 UTF-8 文本，以保留数值及字节摘要；其他读取者只能看到安全元数据。最大结果为 512 KiB，结果载荷保留 30 天，原操作身份不会因清理或同名新连接而改绑。

`POST /actions/{id}/cancel` 提供新的 requestId 记录取消意图。取消、超时、断线和进程停止可能已经产生远端效果，原调用会保留 unknown；不会自动重发。发送前取消或并发忙碌可明确标注未发送。中间输入请求和远端 task 只保存响应，不自动驱动后续轮次。

服务租约失效后标为 orphaned，其他服务实例不会凭旧 PID 终止或再启动程序。运维人员先在宿主确认遗留程序已经清理，再通过交互会话向 `/connections/{id}/confirm-orphan` 提交 requestId、version、confirmed=true 和 reason；要求全部数据范围与 mcp.write，PAT 不能执行。该操作记录人工确认，不伪装成实际观察到的退出证明。

`admin-api maintain-mcp` 执行一次有界租约维护；serve 会周期维护并在关闭时停止自身持有的程序。STDIO 管理接口已接入；HTTP、实际后台业务 MCP 工具、插件能力激活及管理界面仍按 Task 9 B 后续完成。
