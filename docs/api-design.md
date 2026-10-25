# 新 API 规范

- 基址 `/api/v1`；资源名复数，HTTP 方法表达操作。
- 成功 `{data:...}`；列表 `{data:[...],meta:{page,pageSize,total}}`。
- 错误 `{error:{code,message}}`：400 JSON 语法错误、401 未登录、403 权限不足、404 不存在、405 方法不支持、409 冲突、413 内容超限、415 内容类型不支持、422 字段/参数错误、429 限流、503 依赖不可用。
- `X-Request-Id` 用于请求追踪；JSON camelCase，ID 为字符串，时间 RFC3339。
- `/health/live` 表达进程存活；`/health/ready` 表达数据库、模型和初始化就绪。
- 认证 `/auth/login`、`/auth/refresh`、`/auth/logout`、`/auth/me`；访问凭据通过 Bearer JWT。
- 管理资源 `/users`、`/roles`、`/permissions`、`/menus`、`/dictionaries`、`/parameters`、`/files`、`/export-templates`、`/api-tokens`、`/apps`、`/game-servers`、`/game-jobs`。
- 分页默认 page=1/pageSize=20，上限 100；筛选与排序字段按资源白名单。
- 更新支持 version 乐观并发检查，冲突返回 409；接口服务端依据权限操作码鉴权。
- 本规范没有旧接口别名或旧字段转换。
- JSON 入口接受 application/json 及 application/*+json，默认请求体上限 2 MiB；先完整验证语法，再解析字段，错误不回显请求值。方法错误保留 Allow 和 X-Request-Id，未知路径继续返回 404。

## 已实现的认证协议

- `POST /auth/login`：`{username,password}`；返回 `data:{accessToken,refreshToken,expiresIn,user}`。
- `POST /auth/refresh`：`{refreshToken}`；轮换为新的刷新令牌，复用旧令牌撤销会话。
- `POST /auth/logout`：`{refreshToken}`，不依赖有效访问令牌；成功 HTTP 204，幂等撤销该刷新凭证所属会话。
- `GET /auth/me`：Bearer 访问令牌；返回最新用户和角色权限。
- 访问令牌 15 分钟，会话绝对期限 7 天；过期/撤销/禁用立即拒绝，不依赖 TTL 删除时机。
- 登录默认每账号 60 秒 10 次、每 IP 60 秒 100 次；后者由 TRI_LOGIN_LIMIT_PER_IP 配置。
- `POST /auth/password`：`{currentPassword,newPassword}`，成功 HTTP 204 并撤销全部会话。

## 已实现的管理资源

- `/users` GET/POST；`/users/{id}` GET/PATCH/DELETE；`/users/{id}/password` POST 管理员重置。
- `/roles` GET/POST；`/roles/{id}` PATCH/DELETE。
- `/roles/assignable` GET：仅需 roles.assign，分页与标识前缀搜索，仅返回操作者可分配的 `{id,name,slug}`，不要求 roles.read。
- `/menus` GET/POST；`/menus/{id}` PATCH/DELETE；`/navigation` GET 可见树；`/permissions` GET 操作码目录。
- PATCH body 包含 version；DELETE 查询包含 version。冲突 HTTP 409，不覆盖并发修改。
- 用户、角色列表返回分页；菜单为有界树资源。用户名搜索为字面前缀，排序字段白名单且追加 ID 稳定排序。
- 菜单字段 viewKey/parentId/permission/icon 可显式设 null；未提供字段保持原值。
- `/dictionaries` GET/POST；`/dictionaries/{id}` GET/PATCH/DELETE；`/dictionaries/{id}/items` GET/POST；`/dictionaries/{id}/items/{itemId}` PATCH/DELETE。字段和删除约束见 [字典设计](design-dictionaries.md)。
- `/parameters` GET/POST；`/parameters/{id}` GET/PATCH/DELETE。支持有类型的业务配置，字段、空值和数值边界见 [参数设计](design-parameters.md)。
- 角色 `dataScope` 支持 all/self/roles，`dataScopeRoleIds` 为显式选择，`dataScopeRoles` 返回可读标签。当前用户摘要的 `dataScopeRoleIds` 是有效范围并集；all/self 模式为空。授权、成员资格及范围边界见 [客户与范围设计](design-customer-scope.md)。
- `/customers` GET/POST；`/customers/{id}` GET/PATCH/DELETE；`/customers/owners` GET 最小归属人选项。查询支持 status=active/archived、名称/编号前缀及白名单排序；范围外客户返回 404，转移到范围外归属人返回 403。有未删除客户的用户不能删除。
- `/api-catalog` GET，需要 permissions.read，返回有限的完整编译目录 `{schemaVersion,checksum,endpoints}`。端点含 operationId/method/path/requiredPermissions/access/notes；规范与注册方式见 [API 目录设计](design-api-catalog.md)。
