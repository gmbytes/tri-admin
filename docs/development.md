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

自身 schema v6 包含持久导出任务、候选、私有行清单和操作回执；升级先停止服务并执行 migrate。进入“数据工具”的“持久导出任务”，按上方所选字段或模板设置行数上限后创建；活动任务自动刷新，也可按请求 UUID 查询。完成后下载，失败或待确认时可明确重试；响应不明确会留下本机标记，重新登录原账号后“恢复查询”确认，不会自动重复写入。每账号最多两个、全局八个活动任务，服务有两个执行器。

浏览器保存最多 128 MiB，较大结果使用桌面程序。结果保留 24 小时，下载重新检查当前权限/范围；存储位置不可原地改写，未完成任务或未清理候选会阻止相关配置禁用/删除。只清除本机恢复标记不会取消服务器任务，历史中仍可查询。

服务约每 10 秒处理有界过期/遗留导出清理。`cargo run -p admin-api -- maintain-exports` 单独运行一轮，返回 scanned/retired/pending 计数；此 CLI 不启动队列执行器。未知写入只查询原候选，单次缺失不能认定未写入；pending 表示仍需确认或清理。工作目录不可用会退避，归属未确认或其他进程持锁的文件会保留。

任务显示“存储拒绝写入”时，确认存储配置和权限后可明确重试。已发出的写入结果不明或候选读取失败时仍显示待确认，服务先查询原候选，不自动再次上传。S3 专项验证使用 `npm run test:s3`；只运行 SeaweedFS 内容/发布/导出故障门禁可用 `node tools/test-s3-content.mjs`，只运行持久导出故障可加 `--exports-only`。测试容器仅绑定本机随机端口，使用临时凭据，并校验归属后清理。

导出专项同时验证真实服务在 S3 PUT 中被终止后的恢复。常规 `cargo test -p admin-api --test export_service` 还包含生成中终止及数据库成功 COMMIT 响应丢失：仅对隔离测试数据库的子进程连接注入故障，确认同一结果与审计不重复。Windows 上这些测试与其他 Rust/S3/E2E 构建串行执行，避免已运行 EXE 锁定链接目标。

当前开发 Compose 仅绑定本机，使用无认证测试副本集。正式部署需要单独的认证/TLS 配置，开发配置不作为生产配置。

管理员 API Token 引入自身 `open-access` schema v1，当前为 v5；升级先停止服务并执行 `cargo run -p admin-api -- migrate`，否则就绪检查返回 503。v5 在不覆盖定制导航、占用路径及已删除种子菜单的前提下，加入 API Token 与开放应用菜单。管理端写入要求交互管理会话。秘密只返回一次，响应丢失时按 requestId 查询元数据，不能取回原秘密。

应用创建与轮换均返回一次性服务密钥；轮换结果不确定时保留原 requestId 和原 version，先查询回执，不可取新版本后盲目再次轮换。用户创建只以应用内用户名唯一约束防止重复，不为密码建立快速指纹；不确定结果时先查询，重置密码需要明确的版本化操作。开放应用/用户认证与管理页面均使用新协议，不用管理员 Token 充当玩家凭据。

登录后在“API Token”签发本人自动化凭据，选择明确 Scope、有效天数、CIDR 与限额；可查看调用记录和撤销。在“开放应用”创建应用，保存一次性服务密钥；每行的“用户”管理该应用自己的用户，“调用”查看该应用日志，“轮换密钥”使旧应用服务密钥和服务会话失效。只读账号不显示相应写入控件；配置写入权限本身不授予应用用户数据或更高权限密钥。编辑冲突后使用“重新读取”，核对新版本再保存。

一次性秘密只在当前弹窗内存中保留，关闭、离开页面、退出登录或切换服务器后清除，复制由用户明确点击。本机仅按服务器与管理员保存最多 16 项待确认操作标记（请求 UUID、类型、必要的应用 ID/原版本和时间），不保存秘密、密码或表单。响应中断后使用“恢复查询”；恢复只能确认元数据或轮换回执，不能还原秘密，也不会自动发送第二次签发/轮换请求。

Token 预算和调用日志自动维护，也可用 `cargo run -p admin-api -- maintain-access` 单独清理一轮。日志记录 HTTP 处理结果；流式文件的正文传输故障继续使用现有错误日志与相同请求 ID 追踪，下载端仍须完成长度/摘要校验。

开放认证需要独立的 `TRI_OPEN_JWT_SIGNING_KEY`（至少 32 字节，不能与管理签名密钥相同）。新 `.env` 初始化会独立生成；已有开发环境运行 `npm run dev:open-key` 仅追加缺失项，不打印或覆盖已有密钥。缺少该项时管理功能仍可使用，开放认证返回明确的 503，不回退管理密钥。

自身 open-access v3 添加开放刷新摘要、共享应用预算和调用日志，v4 扩展服务用户业务及日志查询索引；升级先停服 migrate。v2 管理阶段没有签发开放令牌，缺少严格开放会话版本的记录在升级时撤销，不当作有效旧身份。`/open/v1` 已提供认证、刷新、注销、本人资料和应用服务用户管理；管理员可在应用页面按身份、用户 ID、结果状态、HTTP 状态、时间和分页查询日志。日志保留 30 天，“待确认”不代表成功或失败。

## 代码生成

升级先停服执行 migrate，新增自身 codegen v1 和第 13 项就绪标记。在“代码生成”新建定义，填写资源标识、显示名称与字段，配置必填/搜索/唯一并调整顺序；编辑使用原版本，冲突后“重新读取”再修改。点击“生成工程”保存不可变源码包，在预览窗口选择文件查看，再“下载 ZIP”。历史保留精确定义与模板版本，删除或修改定义不会改变旧包。

定义和历史遵循当前账号的数据范围，只读账号不显示写入控件。请求响应丢失后，本机保留仅含 UUID、类型、必要定义 ID/版本与时间的标记，使用“恢复查询”确认，不自动重新生成。清除本机标记不会取消或删除服务器结果；本机记录损坏时可先核对历史，再明确清除当前账号/服务的本机记录。

当前模板 v2 修复了 Vue 数字输入在浏览器内被转换为 number 后的表单处理；已有 v1 历史包保持原样，需要修复的工程可用新请求重新生成。生成内容不包含现有账号、签名密钥或数据库配置；独立工程部署时另行配置并创建其管理员。资源描述合同见 `docs/design-code-generation.md`。

### 产物核心验证

本机已有测试 MongoDB 副本集时，在停止开发 API 后用 PowerShell 7 执行：

```powershell
pwsh -File tools/test-generated-project.ps1
```

此门禁创建隔离样例，生成 ZIP 后实际解包并逐文件核对摘要，再运行 Rust 格式检查、锁定依赖构建/真实 HTTP 测试、前端客户端测试和类型/生产构建。结果和日志保存在 `.superpowers/generated-projects/<run-id>`，输出 ZIP 路径和 SHA-256；成功后清理样例的依赖目录，测试数据库使用随机 `tri_generated_test_` 名称并在成功后删除。测试连接只从环境或忽略的开发配置读入内存，不复制到产物。

开发者也可显式生成一个新目录及可选 ZIP：

```powershell
cargo run -p tri-resource-generator --example render -- schema.json new-project new-project.zip
```

目标目录与 ZIP 必须不存在。生成工程附带独立 README、依赖锁和随机签名配置初始化工具；运行时另行配置专用副本集并交互创建管理员，没有默认密码。该产物只含新 Rust/MongoDB/Vue 实现，读写与 all/self 范围在其服务端核对。

## 启动桌面客户端

在另一终端运行：

```powershell
cd E:\hygd\work\backend\tri-admin
npm ci
npm run desktop
```

原生开发需要 Windows Rust/MSVC 构建工具和 WebView2。调试默认服务为 `http://127.0.0.1:18088`；发布版本首次启动需要设置 HTTPS 服务地址。客户端不包含数据库凭据。使用 `bootstrap` 时自行设置的管理员密码登录；保持登录使用 Windows 系统凭据库。

只运行浏览器界面可使用 `npm run dev:desktop`，地址为 `http://localhost:1420`，与默认 CORS 允许源一致。浏览器开发模式不持久化刷新令牌，不能代替原生桌面验收。

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

扩展包格式见 [扩展包说明](extension-packages.md)。包核心测试可用 `cargo test -p tri-extension-package --locked`，扩展库数据库测试可用 `cargo test -p admin-api --test extensions --locked`；后者覆盖创建、修订、范围、去重、审计回滚、原包导出、损坏保护与模型迁移。升级到 extensions v2 后需要 14 项就绪标记，先停服运行 migrate，再启动服务。扩展管理界面使用 `npx playwright test tests/e2e/extensions.spec.ts` 验收，MCP 生命周期仍按子计划继续实现。

Windows 全量构建若出现分页文件不足（os error 1455），优先降低并行编译数，例如 `cargo test --workspace --locked -j 2`，不必清空依赖或修改系统配置。页面夹具的 Cargo 构建可通过当前命令环境的 `CARGO_BUILD_JOBS=2` 使用相同限制；Rust、S3 和页面夹具仍串行运行，避免可执行文件锁冲突。
