# TRI ADMIN

Tauri 2 / Vue 3 客户端，Rust / MongoDB 管理服务。当前只保留用户确认的七个模块：

- 工作台
- 用户管理
- 角色管理
- 菜单管理
- 权限目录
- 日志中心
- 文件管理

字典、参数、配置中心、数据工具、API Token、开放应用、配置版本管理、代码生成、Skills/插件、MCP、AI 和工作流已取消。当前不增加其他业务功能，不提供旧 Go/SQL/旧接口兼容层。

范围以 [七模块设计](docs/superpowers/specs/2026-10-28-game-focused-scope-design.md) 与 [清理计划](docs/superpowers/plans/2026-10-28-game-focused-retirement.md) 为准。

## 本地启动

需要 Node.js 22+、Rust 1.94 和支持事务的 MongoDB 副本集。

```powershell
npm ci
npm run dev:env
docker compose -f deploy/compose.dev.yaml up -d --wait
cargo run -p admin-api -- migrate
cargo run -p admin-api -- bootstrap
cargo run -p admin-api -- serve
```

`bootstrap` 隐蔽输入管理员密码，默认账号 `admin`；重复执行不会重置已有密码。开发环境工具不会覆盖已有 `.env`，不会打印密钥。已有环境缺少文件存储加密密钥时，可运行 `npm run dev:settings-key` 补充，已有凭据保持不变。

另开终端运行 `npm run dev:desktop` 查看浏览器页面，或运行 `npm run desktop` 启动 Tauri 开发客户端。

- 页面：`http://127.0.0.1:1420/`
- API：`http://127.0.0.1:18088/api/v1`
- 开发 MongoDB：`127.0.0.1:13317`

默认允许 `localhost:1420` 和 `127.0.0.1:1420` 两个明确的开发来源。部署时使用 `TRI_CORS_ORIGINS` 显式配置来源。

## 保留能力

角色支持操作权限与全部/自身/指定角色数据范围；账号禁用和密码重置会撤销会话。菜单和权限目录只包含当前七模块。

日志保留登录、操作和错误记录，支持筛选及受控清理。文件管理保留分类、分片上传、续传、异步发布、完整性校验、授权下载和必要的存储驱动。文件发布返回 202 时仍在处理，只有上传状态为 `completed` 才表示发布成功。

文件默认使用服务器受控目录；存储选项仅返回必要的名称和标识。配置中心页面和配置 CRUD 接口已移除，文件内部存储配置不向界面返回凭据。

## 验证

```powershell
cargo test --workspace -- --test-threads=2
npm test
npm run test:e2e
```

对象存储独立验收使用 `npm run test:s3`，会创建带所有权标签的隔离 Docker 服务和随机测试凭据，完成真实文件读写、条件创建和发布验证后清理。默认测试中的外部专项单独记录，不计作已运行。

范围升级会清理已取消菜单和角色权限，保留现有账号、密码、会话、文件和未涉及的定制；不会物理清空旧业务集合。
