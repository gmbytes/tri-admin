# 后台业务 MCP 服务

这是已批准 Task 9 B 的业务服务接入，使用固定 rmcp 3.5.1 和 MCP 2026-07-28。`POST /api/v1/mcp/server` 提供实际 Streamable HTTP JSON-RPC 请求处理；只支持现代无会话协议，拒绝 initialize、旧版本和批量消息，不做握手回退。传输规则依据 [MCP 官方 Streamable HTTP 规范](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)。

每个 HTTP 请求先走已有 AuthContext 提取器，使用当前管理 JWT 或管理 API Token、账号、角色及 Scope。工具参数无法指定操作者，开放应用/用户凭据不进入管理身份。SDK 每请求实例捕获这次原始 AuthContext，既不制造管理员身份，也不再次认证/预占 Token 配额。业务写入继续使用原领域服务和事务审计，代码生成仍在事务中重查身份、权限及 all/self/roles 范围。

发现与列表要求 mcp.read；工具列表按实际业务权限过滤。每次调用另外要求 mcp.invoke 和该工具对应的参数/字典/代码生成权限；代码生成写入继续要求 codegen.read + codegen.write。工具注解仅是说明，不授予权限。共享的字典/参数保持已有全局资源语义，代码生成定义/历史保持创建者的数据范围，范围先于计数和分页。

明确登记 25 项实现：参数 5 项 CRUD/分页；字典及明细 9 项；代码生成定义与生成结果 11 项，包括实际生成、原 UUID 回执和文件预览。没有把任意 API 路径、脚本或包内容变成可执行工具。输入使用现有 DTO 和领域校验，修改/删除带版本。代码生成创建/生成继续按操作者/UUID 去重，未知响应先查原回执；参数和字典创建没有增加隐式重试，响应不明确时应先查询已有资料。

Origin 必须匹配明确的 TRI_CORS_ORIGINS，允许没有 Origin 的认证服务请求。Host 默认只允许 localhost、127.0.0.1 和 ::1；远程部署通过 TRI_MCP_ALLOWED_HOSTS 配置 1–32 个明确主机或 host:port，不接受通配符、凭据、URL、路径或重复项。反向代理须保留符合白名单的 Host。请求最多 128 KiB，工具参数最多 64 KiB，工具结果序列化后最多 512 KiB；过大分页返回安全错误，客户端须减小分页。

浏览器跨域预检明确允许 MCP-Protocol-Version、Mcp-Method 和 Mcp-Name，不放开任意请求头或来源。未配置的来源不能通过浏览器预检读取响应，实际协议请求也会检查 Origin。

业务失败返回结构化工具错误，不回显参数或数据库/SDK 原文。数据库或服务在修改后发生不明确失败时保留 effect=unknown，不声称没有写入，也不自动重发。已提交但结果超限会明确 effect=committed。协议错误保留 JSON-RPC 格式及安全代码，避免被通用 REST 错误归一化改写。所有响应禁止缓存。

SDK 工作任务显式继承安全请求追踪上下文，使领域审计保留原 actor、requestId 和协议路由；不复制请求正文或 Authorization。主程序已有 rmcp/process_wrap 日志过滤继续生效。

已启用插件的业务别名按固定 `plugin.<extensionId>.<revisionId>.<toolName>` 加入目录；实际处理器仍只有上述明确登记的业务操作。目录按当前扩展范围和业务权限过滤；调用重新核对原修订并保存接受审计，不把包内说明、Schema 或参数当作代码。全局启用预算避免目录超过现有 MCP 客户端的 128 项/256 KiB/结构上限，不静默截断。

受控 HTTP 客户端、加密配置引用和 MCP 管理界面已有独立验收。插件来源使用独立分页接口，协议业务别名仍由本服务按当前权限执行；受管 HTTP 连接已有实际插件调用、丢回包后按原 UUID 查询且不重复业务效果的页面验证。真实本机 HTTP/协议和隔离 MongoDB 测试只证明对应边界，不计作第三方外部上游或 Linux 主 API 发布验收。
