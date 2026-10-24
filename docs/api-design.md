# 新 API 规范

- 基址 `/api/v1`；资源名复数，HTTP 方法表达操作。
- 成功 `{data:...}`；列表 `{data:[...],meta:{page,pageSize,total}}`。
- 错误 `{error:{code,message}}`：400 格式错误、401 未登录、403 权限不足、404 不存在、409 冲突、422 参数错误、429 限流、503 依赖不可用。
- `X-Request-Id` 用于请求追踪；JSON camelCase，ID 为字符串，时间 RFC3339。
- `/health/live` 表达进程存活；`/health/ready` 表达数据库、模型和初始化就绪。
- 认证 `/auth/login`、`/auth/refresh`、`/auth/logout`、`/auth/me`；访问凭据通过 Bearer JWT。
- 管理资源 `/users`、`/roles`、`/permissions`、`/menus`、`/dictionaries`、`/parameters`、`/files`、`/export-templates`、`/api-tokens`、`/apps`、`/game-servers`、`/game-jobs`。
- 分页默认 page=1/pageSize=20，上限 100；筛选与排序字段按资源白名单。
- 更新支持 version 乐观并发检查，冲突返回 409；接口服务端依据权限操作码鉴权。
- 本规范没有旧接口别名或旧字段转换。
