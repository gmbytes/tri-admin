# HTTP MCP 客户端与配置引用

这是已批准 Task 9 B 的 HTTP 接入细化。公共 RPC 会话位于 `tri_mcp_runtime::session`，包含有界发现/调用、并发槽位、取消清理及结果类型；STDIO 进程所有权继续位于 `process`，HTTP 不创建或伪造进程。

`http::connect` 使用固定 rmcp 3.5.1 和显式 Discover 2026-07-28。宿主提供已经完成目标策略检查的 reqwest Client；主后台接入时必须使用现有 settings 网络工厂，保留 DNS 解析固定、HTTPS/本机 HTTP、明确 CA、无代理、无重定向和禁止重试。此受信任宿主接口不接受直接来自管理 HTTP 请求的任意 Client、URL 或明文凭据。

传输使用自有 StreamableHttpClient 适配器，JSON 和 SSE 整个响应均受 max_frame_bytes 预算约束，包括未提供 Content-Length 的响应。SSE 原始字节在解析前受限，进度与注释也计入预算。队列、并发、启动/请求/停止时间均有限额；响应缓存、自动重建会话和 SSE 重连关闭，不发送旧 GET/DELETE 会话请求。凭据头禁止覆盖协议、路由及代理头，值标记为敏感。

每次工具调用只发送一次。超时、取消、连接丢失及无法证明未执行的远端 RPC 错误保持未知效果；只保留安全代码。已知请求格式/方法/参数拒绝单独分类；其他 RPC 错误使用 RemoteUnknown。complete、input_required 和 task 保留原结果数据，调用者仍须区分工具返回的 isError/effect，不把收到完整响应当作业务成功。

Windows/Linux 核心测试分别覆盖实际 JSON、真实分片 SSE、调用元数据、取消关闭响应流、丢失/404 回包后一次效果、无 Content-Length 超限和原进程生命周期。真实后台桥接测试验证当前管理身份、工具过滤、禁止越权及角色撤销后立即生效。这些结果不计作加密配置绑定、持久 HTTP 生命周期或外部上游的完整交付。

## 持久接入约束

接下来 HTTP 连接使用 profileId、精确 profileVersion 和 allowedTools 引用已保存、启用的 HTTP 集成配置。创建或更换这项绑定要求当前 settings.write 和 MCP 写权限；使用已批准连接仍按当前 mcp.invoke、连接范围和实际工具清单控制。公开连接不返回目标凭据或私有指纹，安装包内容不能绕过绑定授权。

HTTP 配置引用必须和连接写入、UUID 回执、版本及审计同事务；配置禁用/删除受活动引用保护。配置改变后旧绑定失效，不把旧记录静默指向新目标，也不重新执行旧调用。实际 I/O 在事务外，启动发布及调用前重新检查当前身份、绑定和实例栅栏。

HTTP 生命周期记录网络会话所有权，PID 始终为空。持久层将以明确 transportKind 和统一 releaseConfirmed 区分资源释放：STDIO 需要真实进程退出确认，HTTP 需要本地会话关闭或失效所有权的栅栏处理；均不把未知远端效果改成成功。此模型变化通过本项目自身 mcp schema v1→v2 显式升级完成，保留原 STDIO、UUID、操作和加密结果，不加入旧工程兼容分支。该升级与持久引用目前仍待实现，主服务仍运行 mcp v1。

服务重启后，遗留 HTTP 调用保留 unknown，显式新连接不能重放旧调用；STDIO 继续保留遗留进程的交互确认规则。后续还需 HTTP 持久管理/重启/撤权/密钥与审计故障、插件激活及真实管理界面验收。
