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

## 连接检查

- POST `/settings/profiles/{id}/probe` 带 version，仅 settings.write 可执行；GET `/{id}/checks` 需 settings.read。只使用已保存配置，不接受临时目标或明文凭据。全局并发上限 8，连接检查部分最多 8 秒，DNS/连接另有限时。
- 检查启动时取得原子配置/凭据快照，网络期间不持有配置事务锁；结束时同事务保存检查和审计。配置已变更则结果不是当前结果，不能覆盖新版状态；服务重启后的旧结果也不表示当前进程已验证。
- HTTP/S3 使用固定目标解析结果，拒绝未指定、多播和链路本地地址（覆盖常见元数据网段），不使用环境代理、不跟随重定向。只保留安全状态码和静态摘要，不保存响应正文/头或内部异常。HTTP 只做 HEAD，S3 使用官方 SigV4 做 bucket HEAD，不代表对象写入已通过。
- MongoDB 使用官方驱动及配置的 TLS/副本集选项进行 ping/认证检查，声明主机先经过地址校验；此项不表示集合读写权限或整个拓扑持续健康。TLS 不允许跳过证书或主机校验。
- SMTP 检查会话和可选认证，STARTTLS 必须成功升级；不提交邮件。TLS 仍验证原目标主机；可通过部署信任根配置私有 CA，不允许由普通请求添加信任根。
- 本地存储通过目录能力句柄限制在启动存储根下，创建独立临时文件，验证写/读/移除；不能跟随目录链接越界，不递归清理用户内容。
- 检查只是一次具体范围的观察，不替代 Task 7 文件驱动读写/取消/恢复、实际外部服务能力和 Task 12 部署验收。

传输依据：[reqwest 客户端控制](https://docs.rs/reqwest/latest/reqwest/struct.ClientBuilder.html)、[AWS SigV4](https://docs.rs/aws-sigv4/latest/aws_sigv4/http_request/index.html)、[Lettre 会话检查](https://docs.rs/lettre/latest/lettre/transport/smtp/struct.AsyncSmtpTransport.html)、[cap-std 目录能力](https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html)。

独立 S3 验收使用固定摘要的 Garage 单节点临时容器；正确签名和错误密钥分别验证。MinIO 公共镜像与二进制在本机验证时已不可获取，未将其作为可复现验收依赖。夹具的配置与凭据随机生成，仅监听环回，按标签校验所有权后清理。依据：[Garage 快速启动](https://garagehq.deuxfleurs.fr/documentation/quick-start/)。
