# 本地开发

## 环境

- Node.js 22+，Rust 工具链由 `rust-toolchain.toml` 锁定。
- Docker Desktop；MongoDB 使用独立的本机 13317 副本集。
- 新工程无 Go/MySQL 运行依赖，既有 8088/3306/27017 服务不受影响。

## 启动基础服务

```powershell
cd E:\hygd\work\backend\tri-admin
npm run dev:env
docker compose --env-file .env -f deploy/compose.dev.yaml up -d mongodb
cargo run -p admin-api -- bootstrap
cargo run -p admin-api -- serve
```

`dev:env` 随机生成本地签名密钥，已有 `.env` 不覆盖。`bootstrap` 在终端隐藏输入管理员密码（至少 12 字符），默认用户名 admin，可通过 `TRI_BOOTSTRAP_USERNAME` 修改。重复初始化不会重设已有密码。

服务默认监听 `127.0.0.1:18088`。`/health/live` 检查进程存活，`/health/ready` 在数据库模型和初始化完成后返回就绪。

更新后端版本时，先停止本项目开发进程、运行 `cargo run -p admin-api -- migrate`，再启动服务。`serve` 不自动修改数据库模型，缺少迁移时就绪检查会拒绝放行。

服务启动后周期处理上传发布和文件维护。排查文件遗留可在服务端运行 `cargo run -p admin-api -- maintain-files` 执行一轮有界维护，输出本轮分片恢复/退役/未决、暂存、探测及内容清理计数。计数仅表示本轮处理结果；未知远程写入继续保留定位，不应通过手工删记录来解除存储引用。

当前开发 Compose 仅绑定本机，使用无认证测试副本集。正式部署需要单独的认证/TLS 配置，开发配置不作为生产配置。

## 启动桌面客户端

在另一终端运行：

```powershell
cd E:\hygd\work\backend\tri-admin
npm ci
npm run desktop
```

原生开发需要 Windows Rust/MSVC 构建工具和 WebView2。调试默认服务为 `http://127.0.0.1:18088`；发布版本首次启动需要设置 HTTPS 服务地址。客户端不包含数据库凭据。使用 `bootstrap` 时自行设置的管理员密码登录；保持登录使用 Windows 系统凭据库。

只运行浏览器界面可使用 `npm run dev:desktop`，地址为 `http://127.0.0.1:1420`。浏览器开发模式不持久化刷新令牌，不能代替原生桌面验收。

## 验证

```powershell
npm test
npm run test:e2e
npm --workspace @tri-admin/desktop run build
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
```

数据库测试从忽略的 `.env` 读取测试实例地址，创建随机 `tri_admin_test_*` 数据库，正常结束时清理；失败时可能保留该测试库以诊断。生产实例不能作为测试地址。

Playwright 使用本机 Edge，由测试夹具启动独立临时 Rust 服务，创建随机数据库及随机管理员凭据；结束时停止服务并清理临时数据。原生测试会使用专用范围写入并清理系统凭据库，需在普通桌面会话运行。

桌面 UI、权限和其他业务模块的进度见实施记录；健康检查通过不代表全部产品功能已完成。
