# 来源基线

来源目录：`E:\hygd\work\backend\vue-admin`，Git 提交 `5f3683b`。

原工作区已有 `.codex/rules/project_rules.md` 删除、`server/config.yaml` 和 `web/.env.development` 修改，以及未跟踪的 zip。迁移不修改这些文件，也不把 zip、私有配置、上传文件或日志复制为基线。

2026-10-24 通过只注册 Gin 路由、不加载配置、不连接数据库、不启动端口的工具采集实际路由：

- 运行时注册：210 个方法/路径组合（包含健康检查、Swagger 和文件静态路由）。
- Swagger：127 个操作。
- 运行时、源码、前端、Swagger 合并：232 个候选项。
- 其中 22 个候选未在实际 Router 注册，单独标记为 `unregistered`，不计为已实现接口。

重新采集（在来源 server 目录执行）：

```powershell
go run E:/hygd/work/backend/tri-admin/tools/go-route-inventory/main.go -output E:/hygd/work/backend/tri-admin/contracts/runtime-routes.json
```

随后在 tri-admin 执行 `npm run inventory` 和 `npm test`。`node tools/inventory.mjs --check` 检查基线漂移。

当前 `contracts/fixtures/response-*.json` 是从已检查的响应协议整理的合成夹具；它们不冒充真实 HTTP 捕获。后续隔离数据库联调会补充实际响应证据。
