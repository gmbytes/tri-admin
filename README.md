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

以具有 `settings.read` 权限的账号打开“配置中心”，可查看服务设置、集成配置和启动状态；`settings.write` 允许修改和检查连接。新增集成默认停用，填写完成后显式启用；凭据只能替换或清除，不能读回。升级到当前版本后先运行 `cargo run -p admin-api -- migrate`，再启动服务并重新登录以读取新增菜单。

验证命令：`cargo test --workspace`、`npm test`、`npm run test:e2e`。S3 独立验收使用 `npm run test:s3`：自动创建固定摘要的隔离 Garage 与 SeaweedFS 服务，使用随机凭据和存储桶，分别验证连接签名、不支持条件创建时拒绝发布，以及真实对象读写/并发/删除，最后清理容器与临时文件。默认 Rust 测试中这些外部夹具门禁会标记为 ignored，需另行运行该命令。

- [文件管理与数据工具设计](docs/design-files.md)

Windows 构建/测试前先退出正在运行的开发 API，释放 exe 文件锁。集成连接检查可通过部署环境的 `TRI_OUTBOUND_CA_FILE` 指定私有 CA 证书文件，修改部署信任后重启服务；没有跳过证书校验选项。
