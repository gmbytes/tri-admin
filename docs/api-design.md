# 新 API 规范

- 基址 `/api/v1`；资源名复数，HTTP 方法表达操作。
- 成功 `{data:...}`；列表 `{data:[...],meta:{page,pageSize,total}}`。
- 错误 `{error:{code,message}}`：400 JSON 语法错误、401 未登录、403 权限不足、404 不存在、405 方法不支持、409 冲突、413 内容超限、415 内容类型不支持、422 字段/参数错误、429 限流、503 依赖不可用。
- `X-Request-Id` 使用 UUID 用于请求追踪；非法头值会重新生成，合法 UUID 会规范化。JSON camelCase，ID 为字符串，时间 RFC3339。
- `/health/live` 表达进程存活；`/health/ready` 表达数据库、模型和初始化就绪。
- 认证 `/auth/login`、`/auth/refresh`、`/auth/logout`、`/auth/me`；访问凭据通过 Bearer JWT。
- 管理资源 `/users`、`/roles`、`/permissions`、`/menus`、`/dictionaries`、`/parameters`、`/files`、`/export-templates`、`/api-tokens`、`/applications`；游戏阶段继续实现 `/game-servers`、`/game-jobs`。
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
- `GET /imports` 分页返回操作者导入摘要；`GET /imports/{id}` 查询逐行状态及回执。其他操作者即使具备全范围权限也得到 404。预览保留在私有 MongoDB 记录中，原始值不进入审计或错误日志。
- `POST /imports/{id}/apply` 接收 `{rows:[0,1,...]}`，行号从零开始，每批 1–100 个且不能重复。每行创建、参数审计与 `applied/targetId` 回执在同一事务提交。预览后出现的唯一键冲突只影响对应行；应用响应丢失后可查询或重放，不会重复创建。24 小时后禁止新的行创建，已提交行的回执仍可查询。
- 权限为 `exports.read`（目录/模板/查询）、`exports.write`（导出/预览/应用），并同时要求资源读取权限；导入另需资源写入权限。预览后权限撤销会阻止应用。当前不接受密码、角色授权或密钥列。

具体边界与剩余资源接入见 [数据工具设计](design-data-tools.md)。

数据工具已扩展 `dictionaries`（name/key/description/enabled）和 `dictionaryItems`（dictionaryKey/label/value/sort/enabled），均使用 `dictionaries.read` / `dictionaries.write`。明细筛选为父字典标识前缀；导出先查活动父字典，再按每个父级受限关联活动明细，排序为父字典标识、明细值和稳定 ID。布尔与排序保持 XLSX 原生类型；省略或留空的 enabled/sort 分别为 true/0。

明细预览会绑定内部父字典 ID，应用时核对父级标识与身份；删除、改名或重新创建同标识父级后返回逐行 `PARENT_CHANGED`，缺少父级返回预览错误 `PARENT_NOT_FOUND`。父级未被替换时，实际创建复用正常明细写入的父级锁及删除约束。`data-tools` 自身 schema v2 为原有新项目预览补齐内部 `parentId:null`，保留原回执和定制菜单；没有旧系统适配分支。

导入响应和历史摘要的 `expiresAt` 使用标准 RFC3339 UTC 毫秒字符串，与全项目公共时间约定一致；内部仍用 MongoDB DateTime 比较到期时间。

`POST /api/v1/data-tools/inspect` 接收 `{resource,format,contentBase64}`，需要 `exports.write` 和所选资源的读写权限；返回 `{resource,format,sha256,headers,rowCount}`，不创建预览记录或业务数据。客户端以原始文件摘要核对结果后构造字段映射；真正创建预览时仍提交原内容及明确映射，由服务器重新验证。

导入历史使用 `GET /api/v1/data-tools/imports?page=1&pageSize=20&resource=parameters&status=pending`，pageSize 为 1–100，resource/status 可省略。状态为 pending/completed/partial/failed/expired；权限与操作者过滤在分页前执行，总数和当前页在同一快照读取。返回 `{data:[summary],meta:{page,pageSize,total}}`，摘要包含 id/requestId/resource/createdAt/expiresAt、rowCount/readyCount/appliedCount/errorCount/expiredCount、status、payloadState 和 redactedAt，不包含行值。

完整结果沿用 `GET /imports/{id}`，增加相同汇总。payloadState 为 full 时才返回行标识和值；到期后为 expired（等待后台清理），清理完成后为 redacted，行 key/values 均为 null，保留行号、结果、错误码和目标 ID。32 项配额只计未到期且存在待处理行的预览；完成、全部验证失败和原请求重放不占新名额。服务启动及约每 10 秒逐项事务清理最多 100 项到期内容，数据、回执、计数和审计保持一致；原业务资源不被清理。

数据工具响应设置 `Cache-Control: private, no-store`，避免私有行结果被 HTTP 缓存再次使用；这不替代每次请求的鉴权或服务器到期判断。

## 命名资源模板

- `GET/POST /api/v1/export-templates` 分页查询/创建共享模板，按当前资源读取权限先过滤。配置为 name/code/resource、按顺序排列的 columns（field/header）、导出 search/filters/sort/maxRows、allowImport/enabled。code/resource 创建后不变，maxRows 目前为 1–2000，表头唯一且拒绝控制字符、首尾空格和公式前缀。
- `GET/PATCH/DELETE /export-templates/{id}` 读取、版本化修改和软删除；修改体或删除查询需 version。`POST /{id}/copy` 接收 version/name/code，创建独立 ID 和版本1。失效版本返回409；软删除标识可被新模板复用，但旧 ID 不重定向。
- `GET /api/v1/data-tools/resources/{resource}/query-schema` 提供字段、类型、允许操作、枚举值及排序字段。filters 最多8项，支持登记字段的 eq/prefix/gt/gte/lt/lte/in，in 最多32个类型一致的值；所有条件取 AND。用户范围另外取 AND，模板条件不能覆盖范围。拒绝 MongoDB 操作符、表达式、私有字段及不匹配类型。
- `POST /export-templates/{id}/download` 与 `/export` 接收 version/format，分别生成保存表头的空文件及实际数据文件。导出使用保存的列顺序、表头、条件、排序和行数上限；超限报错，不静默截断。共享配置需要 exports.read/write 和资源读取权限，实际数据仍按当前账号范围读取。
- `POST /export-templates/{id}/import-previews` 接收 version/requestId/format/contentBase64/mapping；模板须启用且允许导入，mapping 只能使用模板列，资源必填字段必须存在。需要 exports.write 和资源读写权限，仍进入同一领域校验、父级绑定及事务回执流程；导出条件不限制导入数据。
- 导入摘要增加可空 template（id/code/name/version）。自身 schema v4 为原记录补 null，保留计数、已清理内容状态及回执；模板版本与有效输入一起绑定请求指纹，配置删除不改变已建立的导入。`GET /data-tools/imports/by-request/{request}` 仅按原操作者与规范 UUID 查询，创建响应丢失或配置删除后仍可确认已有结果，不产生新写入。

大批量持久导出任务继续实施；同步行数保护不代表最终批量能力已经完成。

## 持久导出任务接口基础

当前已接入任务创建、查询、取消、显式重试、后台生成、受控下载及结果清理，任务页面调用这些新接口。服务运行时会处理队列，202 仍只代表任务或操作已接受，须查询 completed 后下载。

- `POST /api/v1/data-tools/exports` 接收 requestId、format、maxRows（默认 100000，范围 1–1000000）、可省略的 storageProfileId 和 source。source.kind 为 resource 时包含 resource、columns（有序 field/header）、search/filters/sort；为 template 时包含 id/version。所有字段和条件沿用资源白名单，拒绝 SQL、任意表达式、私有字段和危险表头。
- 首次创建返回 202，按原操作者/规范 UUID 重放相同规范输入返回 200；不同输入返回 409。模板解析为不可变快照；重放先查原任务，模板已删除或默认存储改变不会替换原条件。省略默认字段和显式默认值归一后绑定同一指纹。
- 每账号最多两个活动任务，全局最多八个；活动包括 queued/running/publishing/reconciling/unknown，超额返回 429。配额、创建及审计在同一事务内，重放和终态任务不占新配额。
- `GET /data-tools/exports` 支持 page/pageSize（1–100）、resource/state，固定按 createdAt/ID 倒序；资源权限与原操作者过滤先于同快照计数和分页。`GET /{id}` 和 `GET /by-request/{requestId}` 查询原任务，其他操作者即使为 Owner 也返回 404。
- `POST /data-tools/exports/{id}/cancel` 接收空对象，取消可重放且只记一次成功审计。取消撤销执行代际并保留必要的清理引用；已完成或已过期任务返回 409，旧执行器无法继续登记私有行清单。
- 创建/取消需要 exports.write 和资源读取权限，查询需要 exports.read 和资源读取权限。公共元数据包含状态、行数、格式、模板摘要、文件元数据及时间；配置、创建范围、存储定位、租约和行 ID 清单不返回。全部响应为 private/no-store；过期即隐藏文件元数据，并在实际内容尚存时显示 cleanupPending。
- `GET /data-tools/exports/{id}/content` 仅原操作者可读，需要 exports.read、当前资源读取权限、completed 且未过期。用户资料还须完整行清单满足当前范围；不满足时在发送正文前返回 409。返回附件原始字节，提供长度、SHA-256 ETag、private/no-store 和 nosniff，末帧通过存储完整性检查后才释放。结果不出现在普通 `/files` 目录，也不能绕过任务授权下载。
- `POST /data-tools/exports/{id}/retry` 接收新的操作 requestId（规范 UUID），需要 exports.write 和资源读取权限，仅 failed/unknown 可新建重试意图。首次接受 202，同一操作重放 200 并返回当前任务；即使上次重试已经再次失败，也不重复启动。每项任务最多保留 128 条操作回执，激活失败任务时重新检查活动配额，操作、意图与审计同事务。
- `GET /data-tools/exports/{id}/actions/{requestId}` 仅原操作者在当前资源权限下查询，返回 jobId/requestId/kind/generation/createdAt。回执确认的是重试意图已接受，任务执行结果仍从详情查询。取消/过期任务不能被新重试复活，源配置和创建范围不被修改。
- 存在已写候选时，包括 failed 状态，也先查询原候选：正确且仍授权可读则复用；只在明确重试后，确认缺失或范围变化才允许新代际。旧未知候选保留清理定位和配置引用；读取鉴权/网络失败不等同于缺失。有效清理租约期间返回冲突，由调用方查询后再决定重试。

## 管理员 API Token

自身 `open-access` schema v1 引入第 12 项就绪标记，当前应用管理扩展为 v2，标记总数仍为 12。所有 Token 管理响应为 `private, no-store`，仅管理本人记录，其他 Owner 也不能读取或撤销。

- `POST /api/v1/api-tokens`：交互会话和 apiTokens.write；输入 requestId（规范 UUID）、name、scopes，以及 expiresInDays（默认 30，1–365）、allowedIps（最多 32 条 CIDR）、minuteLimit（默认 600，1–10000）、dailyQuota（默认 100000，1–10000000）。Scope 为已实现操作权限，不能含 `*`、重复项或超过当前用户权限。
- `GET /api/v1/api-tokens/scopes`：交互会话和 apiTokens.write；返回当前账号可授予、已实现且支持自动化的操作码/标签/分组，排除仅交互会话的凭据与账号写入操作。签发界面不需要额外取得权限目录读取权限；最终签发仍在事务中重新核对当前权限。
- 首次返回 201 的 `data:{token,secret}`，secret 是完整不透明 Bearer 凭据，只在这次响应返回。相同 UUID/规范输入重放为 200、同一 token、secret:null；异参 409。每人最多 32 个活动 Token，签发、并发额度及审计同事务。默认字段、省略字段、CIDR 网络地址和 Scope 顺序归一化后绑定指纹。
- `GET /api/v1/api-tokens`：apiTokens.read；page/pageSize、名称字面前缀 search、createdAt/name 排序；返回 Page。`GET /{id}`、`GET /by-request/{requestId}` 只返回元数据，包含状态、Scope、IP/额度、创建/过期/撤销/最近使用时间，不含摘要或秘密。
- `POST /api/v1/api-tokens/{id}/revoke`：交互会话、apiTokens.write 和空对象；可重放，只登记一次成功审计。过期、撤销、所属用户禁用/删除或凭据重置后，Token 新调用均被拒绝。
- Token 调用使用 `Authorization: Bearer tri_at1.<id>.<secret>`；每次与所属用户最新权限相交，数据范围使用当前账号策略。管理员账号写入、本人改密和 Token 签发/撤销要求交互会话，不能通过有限 Token 取得新的管理员凭据。API 目录中的 Scope 不免除额外的交互会话条件。
- 调用额度为固定 UTC 分钟/日期窗口，跨实例原子预留，Scope 拒绝也计数；来源拒绝仅记录，错误秘密走认证 IP 限制且不扣目标额度。429 预算拒绝提供 Retry-After；来源只取 socket IP，不信任任意转发头，IPv4 映射地址规范化。
- `GET /api/v1/api-tokens/calls`：apiTokens.read，本人调用记录；page/pageSize、tokenId、status、from/to（RFC3339）。开始日志和预算同事务，结束记录安全 HTTP 状态/错误码/耗时；结束日志失败保留 started/空状态，不伪造确认结果。只含已登记路由与请求 ID 等元数据，不含正文、原查询或凭据。
- 调用日志 30 天后立即不返回；主服务启动及约每分钟有界清理，每类最多 100 项。`maintain-access` CLI 单独运行一轮，返回 calls/buckets/attempts，不执行任何业务请求。TTL 不作为授权或内容可见性的依据。

管理员 Token 与开放应用管理 API 已接入；开放身份认证与管理界面仍按后续步骤实施，不以管理 Token 作为玩家令牌。

## 开放应用管理

当前是管理 API，开放应用/用户登录刷新在后续 `/open/v1` 阶段接入；所有响应为 private/no-store，DTO 不返回密钥摘要或密码摘要。

- `POST /api/v1/applications`：交互会话、applications.write；requestId/name/code/allowedScopes，以及 enabled（默认 true）、allowedIps/minuteLimit/dailyQuota（同 Token 策略默认值和边界）。code 规范化为小写，3–64 个 ASCII 标识字符，以字母开头；创建后固定，软删除也不复用。Scope 仅四项 open.profile.read/write、open.users.read/write，并检查相应 applicationUsers.read/write 授权。
- 首次 201 返回 `data:{application,secret}`，密钥格式 tri_app1，32 字节随机秘密；同操作者/UUID/规范输入重放 200，同一应用元数据和 secret:null，异参 409。不存在获取已签发秘密的接口。
- `GET /api/v1/applications`：applications.read，Page、按 code 字面前缀 search、createdAt/code/name 排序。`GET /{id}` 返回活动配置；`GET /by-request/{requestId}` 只查本人创建回执，可返回软删除元数据，不复活配置。
- `PATCH /{id}`：交互会话、applications.write 和 version，可修改 name/enabled/allowedScopes/allowedIps/minuteLimit/dailyQuota，未知或空修改字段 422，旧版本 409。策略/Scope/启用核对用户数据授权；名称修改不要求取得服务密钥权限。`DELETE /{id}?version=` 软删除，撤销该应用会话并保留 code/创建回执。
- `POST /{id}/secret`：交互会话、applications.write、相应 Scope 的用户权限；输入独立 requestId/version。成功 200 返回当前 application、首次 secret 和固定 receipt（applicationId/requestId/kind/version/credentialVersion/createdAt）。同请求只确认原操作，空秘密；异参或其他过期版本 409。`GET /{id}/actions/{requestId}` 仅原操作者在 applications.read 下查询，其他 Owner 也不能读取其操作回执。
- 轮换、应用版本、凭据版本、会话撤销、回执和成功审计同事务。回执不存秘密，不用历史总次数限制后续安全轮换；读写/查询仍有固定字段与分页边界。

## 应用用户管理

`/api/v1/applications/{applicationId}/users` 始终绑定父应用。读取要求 applications.read + applicationUsers.read，管理端写入还要求 applicationUsers.write 与交互会话。管理员数据范围和应用用户身份不混用；应用配置是共享管理资源，应用数据隔离在每个 applicationId 条件内执行。

- `GET/POST /users`：分页/创建；创建 username/displayName/password，scopes 默认 open.profile.read，只允许应用当前范围内的 profile.read/write；status 默认为 active，也可 disabled。用户名 3–64 个字符，应用内大小写规范化唯一；同名跨应用允许。密码使用 Argon2id，最短 12 字符，不使用快速密码指纹。
- `GET/PATCH/DELETE /users/{id}`：固定 appId/id 交叉校验，属于其他应用的合法用户 ID 也返回 404。修改 version 和 username/displayName/scopes/status，删除须 query version。子用户写入不改变应用的公开 version，父记录内部栅栏保护与应用停用/Scope 修改并发。
- `POST /users/{id}/password`：version/password；更新密码、内部认证代际、撤销目标用户会话及成功审计同事务，返回 204。其他应用、其他用户和应用服务会话不受该操作影响。
- 父应用软删除后用户接口返回 404，用户记录保留；用户软删除后旧 ID 不再读取，同名新用户 ID 不同，旧会话不能重新绑定。用户状态切换和删除撤销其会话，重新启用不恢复旧会话。
- 自身 open-access v1→v2 扩展应用、用户、操作回执和开放会话集合/索引；v3 引入独立开放认证，不重写既有 Token、应用密钥或用户密码，不增加旧系统运行时兼容分支。

## 开放认证与本人资料

此命名空间不接受管理 JWT 或管理员 API Token；开放 JWT 也不能访问管理 API。目录 schema v2 用 `requiredScopes` 与管理 `requiredPermissions` 分开描述，显示应用服务、应用用户和开放刷新凭据访问类型。

- `POST /open/v1/auth/application`：applicationId/secret、可省略 scopes；验证一次性应用密钥，返回应用服务访问令牌，refreshToken 为 null。访问会话 15 分钟，过期后重新证明服务密钥，不用用户刷新令牌替代。
- `POST /open/v1/auth/login`：applicationId/username/password、可省略 scopes；用户名在应用内规范化查找。返回访问令牌、轮换刷新凭据、expiresIn、applicationId、principalKind 和授权 scopes，不返回管理员角色。访问最多 15 分钟、会话绝对期限 7 天。
- `POST /open/v1/auth/refresh`：applicationId/refreshToken；刷新凭据为 tri_orf1 类型随机秘密，仅保存摘要。校验当前会话与用户状态、认证代际、原授权上限及当前用户/应用 Scope；并发或旧凭据重用提交会话撤销后返回 401，不通过刷新扩大权限。
- `POST /open/v1/auth/logout`：同刷新凭据证明，注销对应用户会话，不依赖未过期访问 JWT。`POST /open/v1/auth/application/logout`：只接受应用服务访问 JWT，幂等撤销其自身会话。两类注销不被业务预算用尽阻止，不授予业务访问。
- `GET /open/v1/me`：仅应用用户、open.profile.read；`PATCH /me`：仅应用用户、open.profile.write，body 只允许 version/displayName。旧版本 409，未知字段 422。资料与管理用户集合独立；返回应用用户安全资料，不含密码或认证代际。
- 应用/用户会话的调用共享应用分钟/UTC 日预算和来源策略；服务身份访问用户本人资料即使有同名 Scope 仍 403，Scope 拒绝计入业务预算。调用记录只包含真实 applicationId/principalKind/userId/sessionId，不伪装为应用创建管理员。认证尝试有独立 IP/账号限流；登录/刷新和注销不消费业务请求预算。
- 新鲜登录或刷新授权与当前父应用状态在事务中核对，用户密码证明的认证代际也重新核对，防止密码重置后迟到的旧证明创建有效会话。过期判断不依赖 TTL，应用停用/删除不复活，服务密钥轮换不误撤销用户会话。

## 开放应用服务用户业务

`/open/v1/users` 不接收 applicationId 参数，父应用始终取已验证的服务身份。管理员 JWT/API Token 返回 401，应用用户身份返回 403，即使出现同名 Scope 也不成为服务主体。

- `GET /users`、`GET /users/{id}`：应用服务身份与 open.users.read，使用应用内用户名、分页和安全资料 DTO。属于其他应用的有效用户 ID 仍 404。
- `POST /users`、`PATCH/DELETE /users/{id}`、`POST /users/{id}/password`：应用服务身份与 open.users.write，沿用管理端字段、版本、Argon2id、唯一性和会话撤销规则。创建、授予 Scope、重新启用或重置密码时，目标 profile Scope 必须同时属于当前应用和当前令牌的有效授权；仅持有 users.write 不能发放额外 profile 权限。
- 管理端与服务端共用写事务，但分别用真实管理员/应用授权。应用写入在业务事务内重新加载父应用、会话与有效 Scope，不能依赖请求开始时的缓存证明；权限已收缩时不提交。应用审计记录真实 app/session/target，actorId 为空，不借用创建管理员。
- 密码变更、认证代际、会话撤销、数据与成功审计同事务；审计失败回滚业务，开始调用日志失败不执行业务。结束调用记录失败则保留 started/空结果，不能把已成功业务重做或伪装确认。

## 应用调用管理查询

`GET /api/v1/applications/{id}/calls` 要求 applications.read + applicationLogs.read。page/pageSize（1–100）、principalKind（application/user）、userId、state（started/finished）、status（100–599）、from/to（RFC3339）为受控查询。先按路径 applicationId 和 30 天保留期限过滤，再在同一快照计数/分页，按 createdAt/ID 倒序。

返回真实 applicationId/principalKind/userId/sessionId、请求 ID、已登记操作/方法/路由、socket IP、安全 HTTP 状态/错误码/耗时与时间。没有管理员 ownerId、请求/响应正文或凭据。应用软删除后仍允许有权限者按原 ID 查询保留期内历史；其他应用日志不会混入。自身 open-access v4 扩展相应索引，保留既有凭据和回执。

四项 Scope 的服务端业务和相应管理页面已接入；Token/应用的实际签发结果还须通过页面和真实 HTTP 使用验收，不以 API 目录文字代替。

## 代码生成定义与不可变产物

此模块使用 `codegen.read/write` 与当前 all/self/roles 范围。定义和生成历史均按创建者归属，范围在分页前过滤；写入同时要求 read+write。管理 API Token 可在固定 Scope 与所属用户当前权限交集内操作，事务中重新核对有效凭据、角色和范围，不伪造交互会话。请求正文最多 64 KiB，所有响应禁止共享缓存。

- `GET/POST /api/v1/codegen/definitions`：列表支持 page/pageSize（1–100）、资源标识字面前缀 search 和 createdAt 排序。创建输入规范 UUID requestId 和 schema；schema 为 resource/label/fields，字段约束见代码生成设计。首次 201，同一创建者/UUID/描述重放 200，异参 409。活动资源标识按创建者唯一。
- `GET/PATCH/DELETE /definitions/{id}`：读取、以 version 和完整 schema 编辑、以 query version 软删除。外部范围 ID 返回 404，旧版本 409。`GET /definitions/by-request/{requestId}` 仅原创建者查询，删除后的创建回执仍返回同一 ID 与 deletedAt，不复活定义。
- `GET/POST /api/v1/codegen/generations`：历史支持 page/pageSize、资源标识前缀 search 与 definitionId。生成输入 requestId/definitionId/definitionVersion，首次 201、重放 200、异参或版本改变 409。渲染在受控 CPU 工作中执行固定模板，提交时再核对定义与权限；元数据、ZIP 二进制和审计同事务。
- `GET /generations/{id}` 与 `GET /generations/by-request/{requestId}`：返回精确定义/模板版本、描述快照、schemaChecksum、ZIP sha256/size、文件清单及创建时间；请求查询限原创建者。没有二进制或私有去重指纹。旧请求先查询原回执，不因当前定义编辑/删除或模板升级而重新执行。
- `GET /generations/{id}/preview?path=...`：只读原 ZIP 清单中的 UTF-8 文件，返回 path/content/size/sha256。拒绝相对目录穿越、驱动器、反斜杠和清单外文件，核对 ZIP 与文件摘要，单文件最多 512 KiB，不访问服务文件目录或执行源码。
- `GET /generations/{id}/download`：重新授权并核对原包长度/摘要后返回 application/zip、Content-Length、固定 ID 文件名、带引号的 SHA-256 ETag 和 X-Content-Sha256，ZIP 最大 1 MiB。客户端使用现有会话刷新/取消与完整性写入通道，按 ZIP 精确媒体类型验证后才开始保存。

历史保存原 ZIP 字节，模板 v2 的数字表单修复不会改写旧模板产物。新请求采用当前模板，旧请求和历史下载保持原结果。自身 codegen schema v1 添加定义、生成元数据、私有包和索引；主服务就绪标记由 12 项变为 13 项，升级先停服 migrate，没有旧标记数量的运行时回退。

## Skills 与插件扩展库

此模块使用 `skills.read/write` 和当前 all/self/roles 范围。写入需要 read+write；JWT 会话与 API Token 都在事务中重新核对有效凭据、角色、Scope 和记录归属。响应禁止缓存并设置 nosniff；JSON 请求上限 6 MiB，不接受服务端文件路径或任意命令。自身 extensions schema v2 支持从本项目 v1 升级并加入导航，就绪标记总数保持 14，升级需要停服 migrate；没有旧标记数量或旧项目接口回退。

- `GET/POST /api/v1/extensions`：按名称字面前缀、page/pageSize（1–100）与 createdAt/name 排序查询；status 仅接受 active（默认）或 uninstalled，分别列出已安装或已卸载的原记录。范围在计数/分页前过滤。安装输入规范 UUID requestId 和 source，首次 201，同操作者/UUID/输入重放 200，异参 409。活动的 ownerId/kind/name 唯一；初始停用。
- source 的 kind 为 `archive` 时提供 contentBase64（原 ZIP 最大 4 MiB）；为 `skill` 时提供 Agent Skills meta、markdown 和可选 files。文件包含 path/contentBase64/mode（默认 420，即 0644）；只保存内容，不执行脚本。包必须有单个规范名称根目录，根内为 SKILL.md 或 tri-plugin.json。格式及路径预算见扩展设计。
- `GET/PATCH/DELETE /extensions/{id}`：读取、以 version/enabled 启停、以 query version 卸载。旧版本返回 409，外部范围 ID 返回 404。启用前核对原包和摘要；内容更新默认停用。当前 MCP/工具绑定能力尚未接入，含这类贡献的插件启用返回明确冲突。
- `POST /extensions/{id}/revisions`：requestId/version/source 创建新内容修订，保留名称/类型及旧修订。source 可为新原包、完整 Skill，或 `files`；files 提供 write 文件列表与 remove 路径列表，拒绝重复、穿越、无效的必需文件和超限展开。成功增加记录版本和修订号，停用扩展；写入、原包、请求回执及审计同事务。
- `GET /extensions/by-request/{requestId}`：仅原操作者查询其创建/修订结果快照，仍核对当前权限及原记录范围。回执不包含输入正文或私有指纹；卸载、后续编辑或同名新安装不会改写原快照。
- `GET /extensions/{id}/revisions` 与 `GET /revisions/{revisionId}`：有界分页和原修订元数据。卸载后保留原 ID 下可授权的历史。`GET /revisions/{revisionId}/preview?path=...` 返回原包内有界 UTF-8 文本的 path/content/sha256/size，二进制或超过 256 KiB 的文件不能文本预览。`GET /revisions/{revisionId}/download` 核对并下载原字节，返回 ZIP 媒体类型、长度、SHA-256 ETag 与 X-Content-Sha256。
- `GET/PATCH /api/v1/skill-constraints`：读取或以 version/content 更新全局约束（UTF-8 内容最大 64 KiB）。更新额外要求 skills.constraints.write 和当前 all 范围，版本冲突返回 409；`GET /skill-constraints/revisions` 分页读取独立历史。约束和审计原子提交。

扩展库接口和管理页面已经实现；STDIO MCP 管理服务及业务 MCP 服务另见下节，受控 HTTP MCP 客户端和管理界面仍按 Task 9 B 后续交付，不能把停用内容管理等同于完整插件能力验收。

## MCP 持久 STDIO 管理

当前接入 STDIO 后台管理，配置格式和操作步骤见 [MCP 部署程序说明](mcp-programs.md)。自身 mcp schema v1 增加连接、实例、操作回执和加密结果集合，就绪标记由 14 项增至 15 项；升级需要停服 migrate，没有旧数量回退。业务 MCP 服务见下节；HTTP MCP 客户端、插件能力激活及管理界面仍在后续范围。

- `GET /api/v1/mcp/capabilities` 只公开部署能力 ID、标签和工具白名单，不返回路径、参数、环境或配置指纹。`GET/POST /mcp/connections` 支持范围内分页与创建；创建输入 requestId/name/transport/enabled，transport 当前为 kind=stdio + capabilityId。未知命令/路径/环境字段拒绝。
- `GET/PATCH/DELETE /connections/{id}` 读取、以 version 和完整连接配置修改、按 query version 删除；未确认退出或有控制操作时不能修改/删除。部署配置指纹变化需显式重新确认绑定。`GET /connections/by-request/{requestId}` 只查询本人创建身份，原 UUID 重放不受当前能力移除影响。
- `POST /connections/{id}/actions` 提交 requestId/version/kind（start/stop/restart），首次 202，原操作者/UUID/输入重放 200。意图、实例预留和审计先提交，实际启动在事务外执行；停止可打断启动，重启等待原实例确认退出。权限与版本在 I/O 前后核对，状态提交不重新执行外部效果。
- `GET /connections/{id}/tools` 读取实际发现清单与部署白名单的交集；`POST /connections/{id}/calls` 输入 requestId/version/name/arguments，要求 mcp.read + mcp.invoke。管理生命周期要求 mcp.read + mcp.write。原请求恢复使用 `GET /actions/by-request/{requestId}`，读取 `GET /actions/{id}` 或范围内 `/connections/{id}/actions` 分页历史。
- `GET /actions/{id}/result` 额外要求当前 invoke 权限、当前范围且为原调用者，返回原 JSON 的 content 文本、size、sha256 与 kind。结果加密、AAD 绑定操作/调用者/连接，最大 512 KiB，载荷保留 30 天；其他读者的元数据不含参数或结果摘要。
- `POST /actions/{id}/cancel` 用新 requestId 记录取消意图，允许原调用者或具有全局管理权限的调用者取消。发出后超时、取消、断线的效果保留 unknown，不自动重发；发送前取消/忙碌单独记录。input_required 和 task 只保存中间响应，不自动执行更多轮。
- `POST /connections/{id}/confirm-orphan` 要求交互会话、当前 all 范围及 write 权限，输入 requestId/version/confirmed=true/reason。它记录运维人员已清理遗留程序的声明，不根据旧 PID 终止进程，也不伪造 OS 退出证明；过期服务的未知调用仍保持 unknown。

所有响应禁止缓存，连接和历史采用当前 all/self/roles，范围在计数及分页前过滤。周期维护按租约识别失联实例；主服务关闭仅停止自身注册且持有所有权的程序。结果、状态和审计同事务提交，原始 SDK 日志及 stderr 不进入主日志或公开结果。

## 后台业务 MCP 协议

`POST /api/v1/mcp/server` 是实际 MCP 2026-07-28 Streamable HTTP 端点，使用当前管理 JWT/PAT，要求 mcp.read；tools/call 还要求 mcp.invoke 和相应业务权限。每请求附 MCP-Protocol-Version、Mcp-Method，tools/call 附 Mcp-Name；params._meta 携带协议版本、客户端资料和能力。只支持 server/discover、tools/list 及已实现工具等现代方法，拒绝旧 initialize 回退。GET SSE 会话端点不提供。

25 项工具分别映射现有参数、字典/明细、代码生成定义/历史的业务服务。列表按当前权限过滤，写操作保留原版本及审计；代码生成继承 all/self/roles 范围和原 UUID 去重。MCP 结果使用 structuredContent，分页保留 data/meta，单条结果包含 data，创建/生成代码记录还包含 created。业务失败返回 isError=true 和 error.code/message/effect；数据库提交结果不明确时 effect=unknown，不能自动重发。参数/字典创建无 UUID 回执，响应不明确应先查已有资料。

输入、结果、Host/Origin 与错误边界、完整工具类别和后续未交付范围见 [后台业务 MCP 设计](design-mcp-server.md)。本次不增加数据库模型版本，仍需 15 项当前就绪标记。
