# tri-admin 实施记录

计划：`docs/superpowers/plans/2026-10-24-tri-admin-tauri-rust-tok.md`。

## 约定与裁定

- Ruling: 使用用户指定的新目录 `tri-admin` 作为独立 Git 工程，在 `codex/tauri-rust-migration` 分支实施；来源 `vue-admin` 不修改。原因：目标目录本身已隔离于来源项目。若之后需要合并仓库，需另行处理历史归属。
- Ruling: 当前由主实现者在本会话逐项推进；最终安排独立代码审查，避免每项任务重复加载上下文。
- Ruling: Windows 环境中的技能 Bash 辅助脚本若不兼容，使用等价本机脚本保持 ledger、任务 brief、基准提交和验证记录；不跳过验证。

## 预检

| 依赖任务 | 接口 | 处理 |
| --- | --- | --- |
| 1 → 3/5–9 | 路由清单、来源证据、契约夹具 | 规范化方法/路径，数据类型不自动转换 |
| 2 → 5–11 | AppState、ApiResponse、AppError、数据库迁移 | 共用基础类型，统一 schema 迁移所有权 |
| 3 → 4–9 | `/api` 网关、路由所有者 | 仅剥离一次前缀，写请求不回退或双发 |
| 5 → 6–11 | AuthContext、令牌与撤销 | 先验证跨实现互通，再切换依赖模块 |
| 10 → 11 | 管理协议、命令状态、deadline | 游戏身份与管理员身份分离，未知执行结果不盲重试 |

## 任务状态

Task 1: in progress — 建立接口清单与契约工具。

Task 2–12: pending。

## 验证记录

尚未产生功能验证结果。
