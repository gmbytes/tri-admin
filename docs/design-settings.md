# 配置中心

新实现分为启动状态、在线服务设置、集成配置与加密凭据。使用 settings.read/write；不读取或重写旧 Go YAML，不提供旧字段/数据库适配。

- `/settings/runtime` 只读展示监听、数据库名、CORS、会话策略、存储根和密钥配置状态。主数据库 URI、JWT 和加密密钥永不返回；启动项通过部署环境修改并重启。
- `/settings/service` 管理应用名称、可空的每 IP 登录限制覆盖、默认存储和邮件配置 ID，采用 version 并发检查。空覆盖使用当前启动值；下一次登录读取在线策略，应用名称通过认证 `/application` 提供给客户端。默认绑定必须指向启用且类型正确的配置。
- `/settings/profiles` 提供分页/读取/创建/PATCH/软删除；code 为稳定小写标识，配置类型不可变。支持 mongodb、localStorage、s3、smtp、http；字段白名单与共享校验，不接受任意 JSON 或带凭据 URL。
- MongoDB 使用有界主机列表、数据库、认证库、副本集、TLS/直连选项；凭据单独加密。存储位置创建后不可变，变更通过新建配置并切换绑定，避免后续文件定位被静默改写。localStorage 子目录限定安全相对路径，S3/HTTP 仅 HTTPS 或环回开发地址。
- SMTP 显式 TLS/startTLS/plain 与发件地址；明文认证只允许环回开发地址。HTTP 凭据仅为经过验证的授权头，不允许注入 Host/长度/转发/连接控制头。
- 凭据与公开配置分离：创建可传匹配类型的 secret；PATCH 省略保持、null 清除、对象完整替换。没有 `******` 回写约定或读取明文接口，日志和审计不包含配置正文。启用 S3 必须有凭据，其他匿名模式明确允许；非环回 MongoDB 明文凭据需要 TLS。
- 使用 RustCrypto XChaCha20Poly1305、随机 24 字节 nonce 和资源 ID/类型/公开配置摘要/密钥标识关联数据加密；密文、格式、keyId 单独持久化。已有凭据时更改连接配置必须显式重新提交或清除凭据，不能隐式重定向旧秘密。密钥环只来自 TRI_SETTINGS_KEYS（id:64hex，最多8项）和 TRI_SETTINGS_ACTIVE_KEY，支持保留旧密钥解密、逐配置 rewrap 到活动密钥。缺失/损坏密钥明确拒绝凭据操作，不降级明文或借用 JWT 密钥。
- 版本化 profile/服务设置与审计同事务。默认绑定配置不可禁用/删除；删除保留加密历史，活动 code 可在删除后复用。运行时消费者只在服务内部取得已验证配置及解密凭据，不向普通资源读取者泄漏秘密。
- 只在绑定字段发生修改时校验目标与凭据；邮件等未修改配置的密钥暂不可用，不阻断应用名称或登录策略的独立修复。
- 集成注册与配置 CRUD、实际连接/驱动验证、页面分阶段完成；配置记录的存在不能代替 MongoDB/S3/SMTP/HTTP 连接或文件驱动验收。存储驱动与数据工具继续按 Task 7，外部/AI 消费按 Task 8/9 接入。

依据：[RustCrypto 0.11 实现与用法](https://docs.rs/chacha20poly1305/0.11.0/src/chacha20poly1305/lib.rs.html)、[发布记录与 MSRV](https://github.com/RustCrypto/AEADs/blob/master/chacha20poly1305/CHANGELOG.md)。密码库使用应用独立密钥；备份/恢复必须同时安全保留匹配的密钥环，原生/服务发布阶段另行验收。
