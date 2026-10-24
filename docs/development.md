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

当前开发 Compose 仅绑定本机，使用无认证测试副本集。正式部署需要单独的认证/TLS 配置，开发配置不作为生产配置。

## 验证

```powershell
npm test
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

数据库测试从忽略的 `.env` 读取测试实例地址，创建随机 `tri_admin_test_*` 数据库，正常结束时清理；失败时可能保留该测试库以诊断。生产实例不能作为测试地址。

桌面 UI、权限和其他业务模块的进度见实施记录；健康检查通过不代表全部产品功能已完成。
