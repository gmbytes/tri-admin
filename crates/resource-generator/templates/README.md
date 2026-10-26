# 生成的 Rust / MongoDB / Vue 工程

这是独立工程，运行核心随源代码交付，不引用生成机器的路径或主后台凭据。schema.json 是已生成的资源模型，manifest.json 保存模板版本和源文件摘要。资源 API 为 /api/v1/<resource>，values 包含描述字段；版本和所有者由服务端控制。

需要 Rust 1.94+、Node 22+、MongoDB 副本集。先运行 `node tools/init-env.mjs`，在 .env 填写专用数据库的 TRI_MONGODB_URI，再执行：

```text
cargo run --locked -p generated-admin -- migrate
cargo run --locked -p generated-admin -- create-admin
cargo run --locked -p generated-admin -- serve
```

创建管理员时交互输入至少 12 字符密码，不提供默认密码。TRI_ADMIN_USERNAME 可指定账号；create-admin 可加 --read-only 或 --self。该 CLI 仅在服务机器上使用，没有远程创建管理员的接口。数据库、签名密钥、账号均与 TRI ADMIN 主后台独立。

在 web 目录执行 `npm ci`、`npm test`、`npm run build` 或 `npm run dev`。开发页面地址 http://localhost:5173，默认服务 http://127.0.0.1:18089。部署时设置明确的 HTTPS 页面来源及服务地址；前端只在内存保存短时凭据，关闭/注销或切换服务后清除，15 分钟后重新登录，不发长期刷新凭据。

后端真实 HTTP 测试需要先设置 TRI_TEST_MONGODB_URI 指向测试副本集，再执行 `cargo test --workspace --locked`；测试仅创建并清理随机 tri_generated_test_ 数据库。它验证登录、实际 CRUD、版本冲突、软删除及注销失效。生产数据库不能作为测试实例。

文本/整数/小数/布尔/时间/文本列表为普通业务字段，不是密码或密钥存储。每条规范化 values 最多 64 KiB，分页最多 100 条。读取受当前账号 all/self 范围限制；写入核对最新权限、版本并与审计同事务。模型变更会明确拒绝已有不同模型或未登记数据，不能直接改 JSON 后覆盖现有数据。生成新工程与后续正式模型演进需分别审查。
