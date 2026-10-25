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
- `GET /application`：认证后返回应用名称与服务版本；`GET /settings/runtime` 需 settings.read，只返回安全启动状态，不返回主数据库 URI、JWT 或加密密钥。
- `/settings/service` GET/PATCH：读取需 settings.read，修改需 settings.write 与 version；displayName、可空 loginLimitPerIpOverride、storageProfileId/mailProfileId。覆盖 null 恢复启动默认值，登录限制在下一次登录生效；只校验发生变化的默认绑定。
- `/settings/profiles` GET/POST；`/{id}` GET/PATCH/DELETE；`/{id}/rewrap` POST `{version}`。读取需 settings.read，写入/轮换需 settings.write。支持 mongodb/localStorage/s3/smtp/http，分页可按 kind 与 code 前缀过滤。secret 不读回；PATCH 省略保持、null 清除、对象替换，credentials 返回配置标志/密钥标识/密钥是否可用。变更有凭据的连接配置需显式替换或清除凭据，存储位置不可原地修改。
- 配置写入、版本及审计同事务；默认绑定保护与明文/损坏密钥拒绝规则见 [配置中心设计](design-settings.md)。SECRET_STORE_UNAVAILABLE 为 HTTP 503；配置注册不代表实际连接或存储驱动已验收。
- `POST /settings/profiles/{id}/probe`：settings.write，`{version}`，使用保存的配置/凭据执行限定范围检查。有效请求的目标检查失败仍返回 200 的结果（success=false/errorCode），配置前置错误继续用 403/404/409/422/503；并发容量不足 429。结果含 profileVersion、scope、upstreamStatus、durationMs、时间、isCurrent，不返回响应正文/头或内部异常。
- `GET /settings/profiles/{id}/checks`：settings.read，按 createdAt/ID 稳定分页。profile 的 lastCheck 同样为安全摘要；配置变化或服务重启后旧检查 isCurrent=false。scope 明确区分 HTTP HEAD、S3 bucket HEAD、MongoDB ping、SMTP 会话、本地临时文件写读删除，不能作为持续监控或整驱动验收。
- `/file-categories` GET/POST；`/{id}` GET/PATCH/DELETE，读取需 files.read，写入需 files.write。name/code/description，code 创建后固定；按 code 字面前缀搜索，支持 createdAt/code/name 排序分页，PATCH/DELETE 带 version。被活动文件或未结束上传引用时拒绝删除。
- `/uploads` GET/POST；`/{id}` GET。上传会话仅对创建者可见，即使另一账号具有 all 范围也不能读取或取消其他人的未发布内容。列表需 files.read，创建需 files.write；创建为 `{requestId,name,mediaType,size,sha256,categoryId?,storageProfileId?}`，UUID 与操作者绑定，同参重放、异参 409，省略存储选择时捕获当时默认配置，后续重放不改绑。
- 上传采用固定 4 MiB 分片，单对象规范上限 5 GiB、最多 1280 片，每账号最多 32 个活动会话。`PUT /uploads/{id}/chunks/{index}` 需 files.write、application/octet-stream 与 X-Chunk-Sha256，长度必须符合索引位置；同片同内容重放不重复计数，异内容 409，体积 413、媒体类型 415、字段或摘要 422，并发接收容量不足 429。
- GET 上传详情返回 receivedIndices、receivedCount、receivedBytes、state、cleanupPending 等白名单字段；列表不返回完整分片清单，所有响应均不暴露临时路径或内容存储键。分片候选先持久登记，内容 I/O 在事务外，接受清单、计数和成功审计同事务。
- `POST /uploads/{id}/cancel` 接受 `{}`，需 files.write，幂等取消并触发有界清理；`POST /uploads/{id}/cleanup` 重试清理，返回 `{uploadId,processed,pending,complete}`，每次最多处理 100 个确定候选。正在写入的尝试保留待清理状态，不能把取消响应当作内容已完全移除。FILE_STORAGE_UNAVAILABLE 为 503，结果不明时先查上传状态。
- `POST /uploads/{id}/complete` 接受 `{}`，需 files.write；首次进入持久发布队列返回 202，已完成的重放返回 200 的当前上传状态。GET 详情含 fileId/lastError/completedAt；只有 state=completed 表示文件已发布，202 不等于发布成功。缺片 409，完整摘要不匹配进入 failed；丢失/损坏片会失效并从接收计数中移除，按原分片摘要重传后可再发布。
- `POST /uploads/{id}/recover` 接受 `{}`，需 files.write，仅限创建者，返回 `{recovered,retired,unknown}`；每批最多 32 个过期或待确认的分片候选。验证通过且当前清单可接受时恢复一次；即时缺失仍待确认，不能误删迟到内容。
- `POST /uploads/{id}/retry` 接受 `{}`，需 files.write，仅限创建者。已完成返回 200，其余可处理状态返回 202；先查询和校验当前发布候选，已有正确内容直接发布。显式授权重试后，缺失或损坏候选使用全新内容标识，旧的未知写入继续追踪；重复请求不重复创建可见文件。普通 complete 不会因一次 404 自动重写存储。
- `/files` GET；`/{id}` GET/PATCH/DELETE；读取需 files.read，修改需 files.write。按所有者当前 all/self/roles 范围授权，范围过滤先于分页；范围外统一 404。列表支持名称字面前缀、categoryId、createdAt/name 排序。PATCH 支持 `{version,name?,categoryId?}`，分类 null 清除，省略保留；内容、所有者和存储定位不可原地改写。删除提交软删除及精确清理任务，完成请求重放不会恢复已删除文件。
- `GET /files/{id}/content` 返回经鉴权的原始字节，附件文件名含 UTF-8 filename*，设置 nosniff、private/no-store、Content-Length 与 SHA-256 ETag。读取流在完整性通过前保留最后一帧，损坏时中止；响应头后的失败仍写入安全错误日志，不记录内容或内部异常，也不改写成功业务审计。
- 发布工作线程使用持久候选、租约和令牌隔离，元数据/结果/成功审计同事务；已写入但提交失败或上游结果未知时，先核验候选内容再提交，不盲目重复 PUT。主服务重启会继续处理队列和到期发布租约，并按类型执行已知清理任务；未知清理类型保留待确认。
- 主服务周期执行有界文件维护；服务器 CLI `maintain-files` 可执行一轮并返回处理计数，不新增普通用户可调用的全局维护接口。过期原始片先取得恢复令牌；暂存目录只处理规范 UUID 文件、超过一小时且不持有进程锁的候选。S3 探测先持久登记并隔离后续写入，未知结果保留记录及存储引用，不能仅凭 404 宣称清理完成。
- 文件页面和原生核心文件流程已有实现；CSV/XLSX 数据工具及真实原生窗口验收继续按 Task 7 推进，不能将阶段验证等同完整交付。

## 数据工具第一批资源

- `GET /api/v1/data-tools/resources` 返回当前可读资源的字段白名单；`POST /template` 和 `POST /export` 接收 `{resource,format,fields?,search?}`。格式为 `csv` 或 `xlsx`，只支持标识前缀筛选；用户导出在 all/self/roles 范围过滤后限制到 2000 行，禁止静默截断。当前参数可导入/导出，用户资料仅导出安全字段。
- 下载响应仍使用 `{data}`，包含 `name,mimeType,size,sha256,contentBase64,rowCount`。客户端先验证大小及 SHA-256，再通过已实现的原生目标文件端口或浏览器 Blob 保存真实 CSV/XLSX。
- `POST /api/v1/data-tools/imports` 接收 `{requestId,resource,format,contentBase64,mapping}`。同一操作者的 UUID 请求标识绑定内容摘要和字段映射；重复内容返回原预览，不同内容冲突。JSON 请求最多 9 MiB，文件最多 6 MiB；解析在持有全局两个计算名额的阻塞任务里执行，取消 HTTP 请求不会提前释放仍在运行的计算名额。
- `GET /imports` 返回操作者最近 20 次导入摘要；`GET /imports/{id}` 查询逐行值、状态及回执。其他操作者即使具备全范围权限也得到 404。预览保留在私有 MongoDB 记录中，原始值不进入审计或错误日志。
- `POST /imports/{id}/apply` 接收 `{rows:[0,1,...]}`，行号从零开始，每批 1–100 个且不能重复。每行创建、参数审计与 `applied/targetId` 回执在同一事务提交。预览后出现的唯一键冲突只影响对应行；应用响应丢失后可查询或重放，不会重复创建。24 小时后禁止新的行创建，已提交行的回执仍可查询。
- 权限为 `exports.read`（目录/模板/查询）、`exports.write`（导出/预览/应用），并同时要求资源读取权限；导入另需资源写入权限。预览后权限撤销会阻止应用。当前不接受密码、角色授权或密钥列。

具体边界与剩余资源接入见 [数据工具设计](design-data-tools.md)。

数据工具已扩展 `dictionaries`（name/key/description/enabled）和 `dictionaryItems`（dictionaryKey/label/value/sort/enabled），均使用 `dictionaries.read` / `dictionaries.write`。明细筛选为父字典标识前缀；导出先查活动父字典，再按每个父级受限关联活动明细，排序为父字典标识、明细值和稳定 ID。布尔与排序保持 XLSX 原生类型；省略或留空的 enabled/sort 分别为 true/0。

明细预览会绑定内部父字典 ID，应用时核对父级标识与身份；删除、改名或重新创建同标识父级后返回逐行 `PARENT_CHANGED`，缺少父级返回预览错误 `PARENT_NOT_FOUND`。父级未被替换时，实际创建复用正常明细写入的父级锁及删除约束。`data-tools` 自身 schema v2 为原有新项目预览补齐内部 `parentId:null`，保留原回执和定制菜单；没有旧系统适配分支。

导入响应和历史摘要的 `expiresAt` 使用标准 RFC3339 UTC 毫秒字符串，与全项目公共时间约定一致；内部仍用 MongoDB DateTime 比较到期时间。

`POST /api/v1/data-tools/inspect` 接收 `{resource,format,contentBase64}`，需要 `exports.write` 和所选资源的读写权限；返回 `{resource,format,sha256,headers,rowCount}`，不创建预览记录或业务数据。客户端以原始文件摘要核对结果后构造字段映射；真正创建预览时仍提交原内容及明确映射，由服务器重新验证。
