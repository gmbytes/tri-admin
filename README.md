# tri-admin

Tauri 2 + Vue 3 桌面管理客户端、远程 Rust/MongoDB 管理服务，以及面向 tok 游戏服务器的管理协议。

全新架构与实现；相邻的 `vue-admin` 仅用于调研功能范围，不保留旧代码或接口兼容层。

- [全新实现 V3 计划](docs/superpowers/plans/2026-10-24-tri-admin-greenfield.md)
- [实施记录](docs/implementation-progress.md)

开发工具：Node.js 22+、Rust 1.94.0 及 Docker（MongoDB 副本集集成环境）。

本地首次启动：

```powershell
npm ci
npm run dev:env
npm run dev:settings-key
docker compose -f deploy/compose.dev.yaml up -d --wait
cargo run -p admin-api -- migrate
cargo run -p admin-api -- bootstrap
cargo run -p admin-api -- serve
```

`bootstrap` 隐蔽输入至少 12 字符的管理员密码，默认账号为 admin；重复初始化不会重置现有密码。另开终端运行 `npm run desktop` 启动 Tauri 开发应用，或用 `npm run dev:desktop` 启动 1420 端口的页面预览。开发 API 位于 `http://127.0.0.1:18088/api/v1`。

开发环境工具不覆盖已有 `.env`。升级已有工程时，`dev:settings-key` 仅补充缺失的独立配置加密密钥，不打印密钥或改动已有 JWT/数据库凭据。生产环境独立提供密钥环并安全备份，不能使用示例占位符。

- [配置中心与凭据规则](docs/design-settings.md)
