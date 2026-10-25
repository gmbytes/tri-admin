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

导入预览内容保留 24 小时。服务启动和约每 10 秒自动清理到期预览，每轮最多 100 项；原始行值、行标识与父级绑定清除，创建结果、目标 ID 和请求回执保留。可在服务端运行 `cargo run -p admin-api -- maintain-data-tools` 执行一轮并返回 scanned/redacted 计数。清理与系统审计在同一事务中，不修改业务资源；清理失败会由后续轮次重试。新版本先执行自身 schema v3 迁移，已有回执不会被重复初始化。

命名模板在“数据工具”底部管理。新建时选择资源、列和表头，可调整列顺序、导出前缀、类型化条件与排序；模板是共享配置，实际数据仍按执行账号的权限和范围读取。“使用模板”后，顶部显示所选名称与精确版本，下载空文件或导出数据使用保存配置。允许导入的模板可选择 CSV/XLSX，确认来源列和逐行预览后应用。配置修改后需重新选择最新版本；删除不删除已有导入回执，响应不明确时可“查询上次预览请求”确认已有结果。自身 schema v4 增加模板配置和版本摘要，重复迁移不会恢复已清理内容。

大批量持久导出正在实现，当前页面仍使用同步限制。独立编码器的真实磁盘探针可在停止开发 API 后单独运行：设置 `TRI_EXPORT_PROBE_ROWS=500000`、`TRI_EXPORT_PROBE_FORMAT=csv` 或 `xlsx`，执行 `cargo test -p admin-api --test export_writer export_writer_memory_probe -- --exact --ignored --nocapture`。探针生成并清理私有测试文件，输出行数、长度和摘要；峰值内存需另行观察测试子进程，不应把 Cargo/编译器内存当作编码器内存。

自身 schema v5 增加持久导出任务、候选和私有行清单。`/api/v1/data-tools/exports` 支持创建/分页/查询/取消，原操作者可按 UUID 确认丢失响应；每账号最多两个活动任务、全局八个。服务已接入两个执行器，查询 completed 后可从 `/{id}/content` 下载；任务页面和显式重试仍待接入。存储位置依旧不可原地改写，未完成任务或未清理候选会阻止相关配置禁用/删除。

服务约每 10 秒处理有界过期/遗留导出清理。`cargo run -p admin-api -- maintain-exports` 单独运行一轮，返回 scanned/retired/pending 计数；此 CLI 不启动队列执行器。未知写入只查询原候选，单次缺失不能认定未写入；pending 表示仍需确认或清理。工作目录不可用会退避，归属未确认或其他进程持锁的文件会保留。

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

原生文件核心与无窗口 IPC 验证：

```powershell
cargo test -p desktop-core --locked
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --locked
```

5 GiB 磁盘门禁默认忽略，需确认工程所在盘有足够空间后显式运行。它在 `.superpowers/native-file-tests` 下临时生成源和目标两份 5 GiB 文件，验证后清理：

```powershell
cargo test -p desktop-core --test files native_large_disk_streams_five_gib --locked -- --ignored --exact --nocapture
```

开发配置对锁定的 sha2 0.10.9 单独启用优化，避免调试构建的大文件摘要过慢；更新该依赖时同步检查两处 profile。无窗口 IPC 测试使用 Common Controls v6 测试清单；实际打开/保存对话框、安装包及原生页面验收单独执行。

桌面 UI、权限和其他业务模块的进度见实施记录；健康检查通过不代表全部产品功能已完成。
