# API 目录

新服务的规范接口目录位于 `/api/v1/api-catalog`，需要 permissions.read。目录是当前编译服务的只读元数据，包含 schemaVersion、checksum 和按 operationId 排序的 endpoints，不保存可伪造运行接口的数据库记录。

- 每个端点包含 operationId、method、path、group、label、access、requiredPermissions 和 notes。必需权限为列表，条件性规则单独说明；静态操作标识不使用数据库 ID。
- access 的 public 表示不要求已有管理会话，登录仍校验账号密码；authenticated 要求当前会话；refreshCredential 使用刷新凭证；permission 还要求指定操作码。
- `api_routes!` 同一声明同时生成实际 MethodRouter 与端点元数据；模块注册表同时供主路由和目录使用。添加模块只登记一次，避免分别维护一份可能遗漏的目录。
- GET 自动支持 HEAD，CORS OPTIONS 属于 HTTP 协议处理，不作为独立业务操作列出。
- 处理器继续执行真实权限和领域校验；逐端点矩阵缺少任一必需操作码时必须得到 403，仅授予声明的最小权限时必须通过入口鉴权，并验证无会话访问分类。创建用户同时需要 roles.assign/users.write；修改成员角色时的额外分配权限注明在 notes。
- checksum 是端点数组的规范 JSON SHA-256：数组按 operationId 排序、对象键排序。目录变化会改变指纹，供配置包依赖和运维核对；它不包含服务凭据或数据库连接信息。
- 接口数量由编译模块限定，返回完整有限目录；页面提供本地搜索、方法筛选和分页。操作权限目录仍保留独立视图。
- 新格式配置发版包将引用真实 operationId/method/path/requiredPermissions，导入时核验目标能力，不从包中安装 Rust 处理器。
