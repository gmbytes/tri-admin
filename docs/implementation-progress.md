# tri-admin 实施记录

当前规范：[全新实现计划 V3](superpowers/plans/2026-10-24-tri-admin-greenfield.md)。

## 用户决定与实施裁定

- 新项目直接在指定 tri-admin 目录的 codex/tauri-rust-migration 分支实施，原 vue-admin 与现有数据库不修改。
- 用户确认后台使用 MongoDB，随后进一步确认重构过程中也不做兼容；V1/V2 共库、旧接口、导入和 Go 过渡方案全部废止。已删除当前树中的兼容工具与旧契约，历史仅保留在 Git。
- 新 REST API、ObjectId 字符串 ID、camelCase DTO、标准 HTTP 状态、Argon2id 与新权限操作码。
- Rust 后台固定本机已有 1.94.0；tok 的 1.99.0 在独立游戏验证工程处理。
- 使用原生 ledger 代替本机无法运行的技能 Bash 辅助脚本，保留任务、失败/通过结果、裁定和提交证据。
- 开发密钥由跨平台 Node 工具随机生成，已有环境不覆盖，不打印秘密；数据库测试使用独立本机 13317 副本集。

## 任务状态

| 任务 | 状态 | 当前证据 |
| --- | --- | --- |
| 1 新规范与工程清理 | complete | product-spec/API 设计；当前树无 Go/SQLx/旧接口适配，来源工程未变 |
| 2 Rust/MongoDB 基础 | complete | 11 项 Rust 测试、2 项 Node 测试、Clippy 通过；CLI 隐藏输入初始化；实际 live/ready HTTP 200 |
| 3 新认证/会话 | API verified | 10 项认证测试；真实 HTTP 登录/刷新/注销通过；桌面集成验收随 Task 5 完成 |
| 4 用户/角色/菜单/权限 | in progress | 新 RBAC 设计与并发 Owner 保护测试准备中 |
| 5–12 | pending | 按 V3 继续推进，尚未宣称交付 |

## Task 2 验证

- 真实副本集：tri-admin-rs，MongoDB 8.2.1，13317；固定镜像 digest。
- Red → Green：缺少服务实现的测试先失败；刷新摘要唯一约束测试观察到重复插入成功的失败，再增加唯一索引后通过。
- 实际事务验证：并发仅创建一个管理员；重复初始化保留 Argon2id 密码摘要；插入校验失败回滚 Owner 角色和初始化标记；禁用用户不被重启初始化重新启用。
- schema 版本不受支持时拒绝初始化；软删除菜单允许复用路径；准备状态要求事务已提交的初始化。
- cargo test --workspace：11 passed / 0 failed；npm test：2 passed / 0 failed。
- cargo clippy --workspace --all-targets -- -D warnings：通过。
- 真实 CLI bootstrap 创建开发验证管理员，密码只保存在忽略的本地 scratch 文件中，不提交。
- 真实服务 18088：GET /health/live、GET /health/ready 均 HTTP 200，并有 X-Request-Id。该次验证进程已停止，避免影响后续构建。

完整目标仍包含认证、权限、桌面界面、其他管理模块、tok 与跨平台发布；当前基础验证不等于整个目标完成。

## Task 3 验证与裁定

- 新接口 `/api/v1/auth/login`、`refresh`、`logout`、`me`；Argon2id、HS256 严格 issuer/audience/有效期、15 分钟访问令牌及 7 天可撤销会话。
- 刷新令牌只保存 SHA-256 摘要，原子轮换；复用和并发复用会提交会话撤销，再返回拒绝结果。
- TTL 只负责清理；授权显式检查会话有效期、撤销、用户状态及数据库中的最新权限。
- MongoDB 账号/IP 双层限流，切换用户名不能绕过同 IP 上限；账户错误不暴露是否存在。
- 新增实际失败测试：认证模型未就绪时原先仍返回就绪 200，修正后 503；更换用户名原先绕过 IP 限流，修正后 429。
- 全部 Rust 测试 21/21，Node 2/2；Clippy 在 CARGO_INCREMENTAL=0 环境下通过（绕过本机缓存硬链接告警，未修改项目编译策略）。
- 真实网络验证：登录/个人信息/刷新 200，注销 204，注销后访问和刷新 401。验证输出未包含密码或令牌；服务验证进程随后停止。
- Ruling: Task 3 的原生桌面登录/注销验收放在 Task 5，因为界面尚未构建；当前只记录 API verified，不把它冒充原生验收。若客户端适配发现问题，在 Task 5 修正后补齐证据。
