# 新 API 规范

- 基址 `/api/v1`；资源名复数，HTTP 方法表达操作。
- 成功 `{data:...}`；列表 `{data:[...],meta:{page,pageSize,total}}`。
- 错误 `{error:{code,message}}`：400 JSON 语法错误、401 未登录、403 权限不足、404 不存在、405 方法不支持、409 冲突、413 内容超限、415 内容类型不支持、422 字段/参数错误、429 限流、503 依赖不可用。
- `X-Request-Id` 使用 UUID 用于请求追踪；非法头值会重新生成，合法 UUID 会规范化。JSON camelCase，ID 为字符串，时间 RFC3339。
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
- 角色 `dataScope` 支持 all/self/roles，`dataScopeRoleIds` 为显式选择，`dataScopeRoles` 返回可读标签。当前用户摘要的 `dataScopeRoleIds` 是有效范围并集；all/self 模式为空。授权、成员资格及范围边界见 [数据范围设计](design-data-scope.md)。
- `/api-catalog` GET，需要 permissions.read，返回有限的完整编译目录 `{schemaVersion,checksum,endpoints}`。端点含 operationId/method/path/requiredPermissions/access/notes；规范与注册方式见 [API 目录设计](design-api-catalog.md)。
- `/versions` GET/POST；`/versions/{id}` GET/DELETE；`/versions/{id}/download` GET。POST 为 `{name,code,description?,menuIds?,dictionaryIds?,parameterIds?,apiOperationIds?}`，至少选择一项，自动包含菜单祖先及字典明细。版本不可变，历史编号忽略大小写唯一，删除后不复用。
- 版本下载为原始 JSON 文件正文，附 Content-Disposition/ETag；摘要接口继续使用 data 包装。下载除了 versions.read，还需包内各类配置的读取权限。新格式与 8 MiB 上限见 [配置版本设计](design-versions.md)。
- `POST /versions/import-previews`：`{bundle,strategy?}`，strategy 默认为 createOnly，可显式选择 merge。返回 201 的 data 包含 id/checksum/expiresAt/strategy、汇总 counts、最多 100 条 changes、detailsTruncated、versionAction、canApply 和 missingWritePermissions；预览只写短期预览记录，不改配置。版本模块请求上限 10 MiB，规范化包体上限 8 MiB。
- `POST /versions/import-previews/{id}/apply`：`{checksum}`，需要 versions.write。首次应用复查各资源读写权限、操作者、SHA-256、10 分钟有效期和目标版本指纹，冲突返回 409，其他操作者返回 404。配置、版本包、应用结果和摘要审计在同一事务写入，成功返回 200 的 `{previewId,versionId,checksum,appliedAt,counts,versionReused}`。同一操作者携带相同校验和重复提交返回原结果；成功后不受原预览期限影响，不再修改配置或重复审计。
- `GET /logs`：需要 audit.read；kind=login/operation/error（默认 operation），支持 page/pageSize、sort=±createdAt、action（精确）、username（登录账号精确匹配）、actorId、requestId、outcome、status、from/to 时间范围（包含 from、不含 to）。只返回白名单摘要和当前操作者名称，不返回正文、查询或凭据。
- `POST /logs/cleanup`：需要 audit.read 与 audit.delete；`{requestId,kind,before,limit?}`，body requestId 是清理幂等 UUID，与每次传输的 X-Request-Id 分开。只清理至少 24 小时以前、最多 1000 条的确定批次，返回 `{requestId,kind,before,deletedCount,hasMore,completedAt}`；同键异参 409，其他操作者 404，重放不再删除下一批。更多规则见 [日志设计](design-logs.md)。
- 框架产生的失败响应也归一化为标准错误 JSON，保留状态和 Allow 等响应头；不向响应或错误日志复制原始拒绝正文。处理器 panic 返回 500 的 INTERNAL_ERROR，日志不保留 panic 内容。
