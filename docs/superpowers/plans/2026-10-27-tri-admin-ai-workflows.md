# AI 上游与工作流实施计划

> Use superpowers:executing-plans inline. Preserve the full V3 goal; one fresh whole-branch review remains at final completion.

**Goal:** 新 Rust AI 上游、流式响应、需求分析/Prompt 工作流、持久任务/取消/历史与授权导出。
**Architecture:** 协议核心独立 crate，管理服务解析加密绑定和当前权限，MongoDB 意图/租约/事件驱动持久任务，Vue/Tauri 复用会话和完整性下载。
**Tech Stack:** Rust1.94/Axum/Tokio/MongoDB/Vue3，固定 Responses HTTP 合同；不新增旧 Go/SQL/API 兼容层。
**Spec:** `docs/design-ai-workflows.md`。用户暂无服务并授权选型，首个上游选择 OpenAI Responses API；实际凭据验收单列。

## 全局约束

- 原身份、当前权限/Scope 和批准上游/模型；不伪造 JWT/管理员/开放用户转换。
- 写入意图和审计先于外部效果；未知响应不自动重发，恢复只读已知响应。
- 沿用加密 HTTP 引用、受控网络和密钥环。提示/结果不入日志或浏览器恢复记录。
- 普通事件/提示 64KiB，完整终态帧/总响应 4MiB；4096 事件、32 输出部分、1MiB 文本、120s 总期限/30s 空闲、2 执行槽。
- 原生 GUI 暂停持续；仅 headless/CLI/MockRuntime，外部真实厂商验收单独报告。

## Review Focus

1. 未收到终态、拒绝或不完整响应不得显示成功；协议状态与业务分析结果分别判断。
2. 发送后丢响应不重复付费生成；响应 ID/事件游标保存与租约代际不可分离。
3. 账号/服务器/权限/上游版本变化后旧内容不能进入新窗口或新目标。
4. 提示、API Key、上游错误正文和模型工具请求不能泄漏或提升权限。
5. 取消/重启/导出/历史相互竞争时不复活任务，不接受任意主机落盘路径。

## Task 1：协议状态与有界流核心

**Files:** `crates/ai-runtime/{Cargo.toml,src/lib.rs,src/responses.rs,src/sse.rs}`, `crates/ai-runtime/tests/{responses,sse}.rs`, workspace Cargo。
**Interfaces:** `ResponseState::apply_json(&mut self, payload: &[u8]) -> Result<Update, Error>`；`finish(self) -> Result<Reply, Error>`，状态可序列化供加密检查点，类型不输出原正文到 Debug/Error。

- [x] 写文本/拒绝/多部分、乱序/重复/ID变化、keepalive不改游标、截断和预算失败测试，观察缺失 crate/模块失败。
- [x] 实现当前语义事件、原字节/数值保留、完整终态校验与不自动执行工具的状态机。
- [x] 使用现有 SSE 库和有界输入处理实现碎片 UTF-8/CRLF/多行/截断/超限测试；严格检查并保存核心。

## Task 2：实际 HTTP Responses 适配器

**Files:** `crates/ai-runtime/src/{http,request}.rs`, `crates/ai-runtime/tests/http.rs`。
**Interfaces:** 宿主传入受控 reqwest Client、固定端点及敏感头；创建只 POST 一次，已知响应 GET/背景取消独立方法；以有界流返回经过校验的更新和恢复检查点。

- [x] 用真实受控 HTTP/SSE 测试请求、分片、JSON与流、取消、超时、302不带凭据跳转、拒绝/未知与一次生成，先观察失败。
- [x] 实现固定端点/安全响应 ID、无重试、总/空闲期限、请求/事件/响应预算和安全错误；没有当前配置时不发真实厂商请求。
- [x] 验证背景恢复只 GET、keepalive不污染游标、丢回包/终态不一致保留未知；运行适当核心回归和严格检查。

Task 1/2 验证：30 项协议/SSE/真实本机 HTTP 测试通过，完整 workspace 483 项常规测试通过、12 项外部专项按原配置保留；严格 all-targets Clippy、格式检查通过。已修正失败快照保留部分输出、当前收紧限额、响应头空闲期限和缓存事件取消/总期限四项实际失败。尚无管理服务配置/持久运行/页面或真实厂商调用，继续 Task 3–5。

## Task 3：批准上游配置与持久 AI 运行

**Files:** `services/admin-api/src/modules/ai/{dto,models,binding,service,router,worker,journal}.rs`, `storage/ai_schema.rs`, 相应测试。

- [x] 先测试配置版本/引用、权限/Scope、UUID/摘要/限额、审计回滚、密文和安全分页，观察缺失接口失败。
- [x] 实现批准 HTTP/模型目录，全局维护需全部范围和配置写权限；同一 settings 不变量保护引用，不读回密钥。
- [x] 实现原 UUID 意图、私有加密输入/事件/结果、当前身份与范围证明、租约代际、取消、状态查询与原回执；后台只读恢复，不再生成。
- [x] 真实进程停止/重启、发送和提交竞争、当前角色/凭据/绑定变化、密钥不可用和未知结果通过；同步 JSON 和流式都验证。

Task 3 后台验收：11 项实际执行、5 项意图/范围/并发限额、4 项上游配置及 2 项分块密文测试通过。核心扩展为 34 项，补齐长终态与多次读取的独立字节预算。完整 workspace 首次为 508 通过/1 项 API 目录权限顺序失败/12 项外部专项保留；只调整三个声明的权限顺序后，目录 3 项和 AI 16 项重新通过，未将首次全量结果改写成全绿。最终严格检查、格式和实际 API 构建通过，开发库升级自身 ai v1/16 个就绪标记，12 个 AI 接口、3 项权限和私有空列表已实际只读核对并注销。实际厂商凭据/模型调用及 Task 4/5 仍未完成。

## Task 4：需求分析、会话与持久 Prompt 工作流

**Files:** `crates/workflow-runtime/{src,tests}/`、`services/admin-api/src/modules/workflows/{dto,models,plan,service,worker,export,router}.rs`，自身 schema 及测试。纯计划/分析/导出核心独立，以便同一规则用于接口、工作线程及验证。

- [x] 先测试分析/计划格式、消息分支与私有历史、任务 UUID/版本、取消、重启和 Markdown 大小/摘要/授权。
- [x] Rust 保存固定步骤/依赖/Prompt/预期输出，关联 AI 运行；模型提案经校验且需明确用户操作才进入代码生成或 MCP 调用。
- [x] 输出通过受控产物下载，不收任意主机目录；迟到 worker、过期内容、审计回滚和一项失败/未知不伪装整计划成功。
- [x] 运行真实任务/进程/存储流程，保留所有9C/9D要求，不以模板或元数据代替实际执行。

Task 4 后端验证：工作流核心 9 项、后端计划 4 项及实际执行/取消/主进程重启/加密会话与分支/私有导出 14 项通过；完整 workspace 539 项常规测试通过、0 失败、12 项外部专项保留，严格 all-targets Clippy、格式检查通过。结构化分析同时使用固定上游 JSON Schema 与本地语义校验，非法结果停止后续步骤；后台以固定父意图授权子运行，不伪造用户会话。会话读取引用已有运行，不再请求模型。此阶段不包含 Task 5 页面、用户应用代码生成建议的界面或真实厂商凭据验收。

开发服务已实际升级 workflows v1 / 17 个就绪标记，31 项工作流接口、3 个内置方案、私有计划/任务/会话空列表与现有 AI/MCP 功能只读核验通过并注销；未向开发库写入演示数据或发起模型调用。

## Task 5：管理界面与完整验收

**Files:** `apps/desktop/src/core/{ai,workflows}.ts`, 对应 views/components/tests，菜单与 API 目录、docs。

客户端基础已实现严格 AI/工作流数据模型、私有内容与固定定义摘要校验、仅 UUID/版本的恢复记录、8 MiB 有界私有 JSON、4 MiB Markdown 完整性下载，以及会话切换/异步校验后的隔离。工具 6 项、客户端 142 项、类型检查与生产构建通过；尚未注册 AI/工作流页面或菜单，页面验收继续下列步骤。

- [ ] 严格模型、元数据恢复、流式进度/取消/历史/导出先测试，再实现上游/模型选择、分析与 Prompt 工作流页面。
- [ ] 真实页面验证一次生成、重载恢复、私有内容隔离、只读/撤权、慢分页和配置/版本冲突，桌面/窄屏核对。
- [ ] 完整 Rust/Node/页面/严格检查、实际库升级和只读核验后注销、恢复隐藏服务并保存提交。真实厂商调用单独验收；缺凭据时保留该门禁，继续独立后续目标。
- [ ] 更新完整 V3 进度并继续 tok10/可靠游戏任务11/发布12；整个目标未满足前不调用 complete。
