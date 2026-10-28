# 游戏管理范围收敛实施计划

> 使用 superpowers:executing-plans 内联执行；整体目标最后进行独立审查。

**Goal:** 完整移除已取消的通用平台和开发模块，保留可运行的游戏管理基础。
**Architecture:** 会话认证、RBAC、日志、导航独立；自身范围迁移清除退役入口/授权，不物理清空旧业务数据。
**Tech Stack:** Rust / MongoDB / Vue / Tauri。
**Spec:** `docs/superpowers/specs/2026-10-28-game-focused-scope-design.md`。

## 全局约束与审查重点

保留账号、密码、当前角色/范围和会话。API18088、前端1420、Mongo13317；保留 localhost/IPv4 CORS。无旧功能兼容接口、无伪造身份或虚构游戏数据。不输出凭据，不清空数据库，不停止未知进程。Rust/S3/E2E 串行、jobs2/incremental0。

## Task 1：核心后端与范围迁移

**Files:** `services/admin-api/tests/retired_features.rs`、`src/modules`、`src/{config,state,main,lib}.rs`、`src/common/observability.rs`、`src/storage/{mod,schema,game_scope_schema}.rs`、workspace Cargo。

- [x] 扩展退役回归：路由404、权限/目录/导航缺失、新库不创建取消集合；观察RED。
- [x] 测试旧库升级：停用取消入口/权限，原账号、密码、范围、会话和无关导航保留；幂等和未知当前版本拒绝。
- [x] 删除已取消模块、任务、Token/开放身份、专属配置/状态/迁移器/crate/依赖/示例和夹具；保留当前会话与角色复核、日志。
- [x] 运行当前基础后端回归并修复有证据的耦合。

## Task 2：浏览器和桌面

**Files:** `apps/desktop/src/{core,components,views,stores,platform,main,style}`、`apps/desktop/src-tauri`、`crates/desktop-core` 及测试。

- [x] 测试已取消入口与直接旧路由不可进入页面。
- [x] 删除专属页面、组件、类型、恢复记录和下载通道；保留文件管理所需的 Tauri 文件命令和权限，以及服务器设置/会话存储。
- [x] 工作台/登录页按实际游戏管理基础整理，不添加不可用游戏入口。
- [x] 当前 Node、类型、生产构建、页面和原生核心验证通过。

## Task 3：实际升级与范围审计

- [x] 当前全量 Rust/Node/页面、严格检查和格式通过；README、产品规范和活动计划更新。
- [x] 构建 API，核对当前 PID/UTCticks/路径/哈希及旧工作进程，再切换稳定镜像和执行自身范围迁移；不用旧升级脚本。
- [x] 验证精简菜单/权限/404、健康、旧前端路由跳转和现有管理员登录。
- [x] 保存提交和证据，当前只交付七模块，不自动推进 tok 或其他业务扩展。

验证结果：完整 Rust 159 项通过，客户端 72 项与工具 4 项通过，页面 17 项通过，严格 Clippy/格式/类型/生产构建通过；桌面无窗口命令 5 项、真实 S3 文件专项 3 项通过。运行服务已升级为七模块，实际管理员登录、7 项导航、14 项权限、退役接口 404 和原账号/角色/文件标识保持均已核对。原生 GUI/文件选择器沿用用户暂停状态，未操作。

最终审查：一次独立只读审查完成。发现的自定义保留子菜单被退役父菜单孤立的问题已通过实际 RED→GREEN 回归修复；范围迁移 v2 在菜单锁内将保留节点移至最近有效祖先，支持已升级 v1 的修复并保持幂等。随后完整 Rust 159 项、页面 17 项、严格检查和实际构建重新通过；本机迁移 v2、7 项导航、14 项权限、旧接口 404 及原管理员登录再次核验通过。已取消游戏入口承诺的旧文案也已移除，不增加功能。
