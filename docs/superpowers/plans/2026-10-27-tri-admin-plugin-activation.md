# 插件贡献能力实施细化

> **For agentic workers:** Use superpowers:executing-plans to implement this authorized Task 9 B continuation inline. Whole-branch fresh review remains at the full-goal completion gate.

**Goal:** 启用经过验证的插件贡献，通过现有 MCP 服务实际调用后台业务工具，并显式建立引用批准 STDIO 能力的连接。

**Architecture:** 工具绑定发布到已有 `/api/v1/mcp/server`，名称固定为 `plugin.<extensionId>.<revisionId>.<toolName>`。调用使用原 AuthContext 和真实业务处理器；现有 MCP 连接/调用记录承担 UUID、取消、未知效果和加密结果，避免另建调用账本。插件 MCP 贡献使用新的 plugin 连接来源，固定扩展/修订/贡献 ID，底层仍是部署批准的 STDIO 程序。

**Tech Stack:** Rust 1.94、MongoDB、rmcp 3.5.1、Vue 3；JSON Schema 固定 jsonschema 0.56.0，关闭默认引用解析并显式 offline、使用线性正则引擎。

**Spec:** `docs/design-extensions-mcp.md`, `docs/design-mcp-management.md`, `docs/design-mcp-server.md`。

## 约束与工程裁定

- 不增加旧项目兼容层，不重新引入客户/客户端管理。安装、修订和启用不执行代码、启动进程或访问上游。
- 工具 operation 只接受现有 25 项后台处理器。mappings 为目标字段到声明的来源属性；不求值表达式、不使用任意 API 路径。create/update 的平面目标字段由明确适配器包装为原处理器的 input；明细列表的 query 同样由适配器构造。
- 编译时检查目标字段、必填映射和 Schema；调用时同时验证来源 Schema 与处理器原生 Schema。原 JSON 数值不经过浮点重写。外部引用不可解析，错误不回显 Schema/参数正文。
- 当前扩展权限和 all/self/roles 范围、启用状态、固定修订、MCP invoke 与实际业务权限均在受保护的接受点核对。停用/更新后的新请求不能执行旧贡献；已接受的业务调用保留原身份及未知效果语义，不自动重做。
- 全部启用的插件工具与原生工具共同遵守当前 MCP 128 项、256 KiB、深度 32、节点 4096 的发现预算；启用时明确拒绝超额，不静默截断。共用 extensions 不变量处理并发启用。
- 插件 STDIO 来源仅可选批准的 capabilityId 和贡献工具子集。创建、启动、发现发布和调用再次核对当前绑定，停用/换修订关闭自身持有的旧实例，不根据陌生 PID 终止程序。

## Review Focus

1. 同名更新后旧修订工具不能执行新内容；覆盖工具名称、连接绑定和旧 UUID 回执。
2. 包声明权限或注解不能授予业务权限；覆盖只读、实时撤权、PAT Scope 和不同所有者范围。
3. 非法映射、外部 Schema 引用、超大目录不能导致网络/文件读取或静默截断；覆盖实际拒绝和并发启用。
4. 安装/启用没有执行效果；只有显式调用或启动产生实际记录/子进程。
5. 停用/更新与运行中的调用/进程竞争时，旧结果不能覆盖新实例；保留未知效果，不自动重发。

## Task 1：绑定编译与真实工具目录合同

**Files:** `services/admin-api/src/modules/extensions/tool_binding.rs`, `services/admin-api/src/modules/mcp/business.rs`, `services/admin-api/tests/plugin_bindings.rs`, Cargo manifests/lock。

- [x] 先写并运行别名映射、create/update 包装、必填/未知目标、Schema/外部引用及 u64 测试，观察缺失模块失败。
- [x] `compile(binding: &ToolBinding) -> Result<CompiledTool, AppError>` 仅从实际业务合同取得权限、注解、原生 Schema；`CompiledTool::arguments(&Value) -> Result<Value, AppError>` 保留数值并验证两端格式。
- [x] 固定依赖，离线且不启用 HTTP/file resolver，限制正则和结构预算；9 项目标测试和严格检查通过。重复映射放大已先复现，再补上输出参数预算。

## Task 2：启用验证和实际 MCP 插件工具

**Files:** `services/admin-api/src/modules/extensions/{contributions,service}.rs`, `services/admin-api/src/modules/mcp/server.rs`, `services/admin-api/tests/plugin_tools.rs`, `services/admin-api/src/storage/extensions_schema.rs`。

- [x] 先测试安装默认停用、合法启用与真实参数创建/查询、当前权限/Scope、更新/停用拒绝旧工具、审计失败和并发目录预算，观察原启用冲突失败。
- [x] 启用时解析核对原 ZIP、编译业务工具，在同一扩展不变量下验证全局发现预算；新增修订仍默认停用。MCP 贡献按 Task 3 校验批准引用。
- [x] 目录按当前授权过滤；调用精确解析原扩展/修订，提交接受审计后由现有业务处理器执行。处理器不替换原 credential；代码生成继续使用已有 UUID 合同。
- [x] 6 项真实协议/数据库用例、9 项绑定合同及原有扩展/MCP 服务 14 项通过，严格 workspace/all-targets 检查通过。完整插件验收见 Task 3/4。

## Task 3：插件 MCP 来源与运行约束

**Files:** `services/admin-api/src/modules/mcp/{dto,binding,runtime,service}.rs`, `services/admin-api/src/storage/mcp_schema.rs`, `services/admin-api/tests/plugin_mcp.rs`。

- [x] 先测试新来源创建、显式启动/发现/调用、工具子集、版本与范围、停用/更新后的释放和旧 UUID 查询，观察缺失来源失败。
- [x] 新 transport plugin 固定 extensionId/revisionId/contributionId，解析为批准 STDIO 能力；指纹包含固定来源、启停记录版本和部署配置。启用不自动启动，空子集不扩大工具权限。
- [x] 当前授权在绑定/使用时检查，生命周期保存现有实体和审计；后台只停止自身旧资源，实际子进程验证退出。
- [x] 显式自身 mcp v4 升级拒绝旧二进制就绪，保留当前 STDIO/HTTP/历史/密文，9 项新用例及相关 37 项回归通过。

## Task 4：管理界面与完整验收

**Files:** `apps/desktop/src/views/ExtensionsView.vue`, `apps/desktop/src/core/extensions.ts`, `apps/desktop/src/views/McpView.vue`, `apps/desktop/src/core/mcp.ts`, `services/admin-api/src/modules/mcp/router.rs` 及对应 API 目录、docs 与 `tests/e2e` 用例。

- [x] 提供当前贡献预览/稳定工具名称、MCP 插件来源选择和明确的批准工具/修订显示；不让说明文本成为执行指令。
- [x] 真实页面验证启用、21 条来源搜索/跨页、显式连接启动与插件工具调用、权限限制、冲突、响应丢失单次效果、停用和重新绑定；补充重启更换实例与 PID 的页面验证通过。
- [x] 完整 Rust 453 项常规、工具 6 项/客户端 122 项、完整页面 76 项通过，12 项专项按原配置保留；严格检查、构建和桌面/窄屏核对通过。实际库升级 mcp v4，维护 0/0/0/0、只读核验注销、API/前端/模块 200。原生 GUI 暂停约束继续保留。
- [x] 更新 Task 9 B 状态并保存提交；继续 9 C/D、tok 与后续阶段，不在此关闭整个目标。
