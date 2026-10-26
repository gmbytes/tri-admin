# Task 7 验收核对

服务端与浏览器能力已实现并验证；Task 7 的原生窗口/安装验收仍待用户恢复电脑自动操作。此记录不代表整个项目完成，也不以无窗口测试替代真实 Windows 安装流程。

| 要求 | 当前证据 | 状态 |
| --- | --- | --- |
| 分类、版本冲突、上传 UUID 重放、重名显示元数据和存储引用 | `services/admin-api/tests/file_uploads.rs` 覆盖并发创建、缺失引用、路径/文件名、分类删除竞争及稳定索引 | 已验证 |
| 分片乱序、精确长度/摘要、容量、取消及失败恢复 | 同文件覆盖正文竞争、4 MiB 正文与尾片、丢失回执、跨状态恢复、取消/IO 竞争和有界清理；`file_recovery.rs` 验证到期写入及迟到内容 | 已验证 |
| 不可变本地/S3 内容、鉴权下载、完整性和删除 | `file_storage.rs`、`file_publication.rs` 验证同卷原子发布、真实 S3 条件创建、篡改、跨范围读取、发布失败及精确删除；`files_navigation.rs` 验证 schema 与菜单 | 已验证 |
| 原生文件核心、能力句柄和持久私有恢复日志 | `crates/desktop-core/tests/{files,file_handles,file_recovery}.rs` 与 `apps/desktop/src-tauri/tests/file_ipc.rs`；已有真实子进程退出和 5 GiB 磁盘门禁证据，详见实施记录 | 核心已验证，真实窗口见下方 |
| 四种显式资源 CSV/XLSX、字段映射、受控过滤及边界 | `data_formats.rs`、`data_tools.rs`、`data_dictionary_tools.rs`、`data_inspection.rs`；不接收 SQL、集合名、任意表达式或密码/令牌字段 | 已验证 |
| 行级业务/审计/回执事务、历史分页和 24 小时内容保留 | `data_history.rs`、`data_retention.rs` 及资源测试；权限过滤先于分页，已清理回执不能重新创建业务对象 | 已验证 |
| 命名模板的保存/版本/复制/删除、实际导出和导入 | `export_templates.rs` 及浏览器 `export-templates.spec.ts`，包括配置删除后的请求恢复、列顺序、类型化筛选、只读及过期版本 | 已验证 |
| 大批量 CSV/XLSX、内存/磁盘/行数预算、清单与授权 | `export_writer.rs`、`export_authorization.rs`、`export_worker.rs`；实际 5001 行双格式、2501 条明细和 50 万行编码性能证据已记录 | 已验证 |
| 持久任务、配额、请求/操作幂等、取消/重试和授权下载 | `export_jobs.rs`、`export_retry.rs`、浏览器 `export-jobs.spec.ts`；下载检查原操作者和当前资源范围，不转为普通文件权限 | 已验证 |
| 未知结果、S3 故障、迟到写入及清理 | `export_s3.rs` 五项真实 SeaweedFS 测试：写后响应丢失、等长损坏、明确拒绝、412 后读取拒绝、取消竞争和迟到对象精确清理 | 已验证 |
| 真实服务生成/发布中终止及数据库提交响应丢失 | `export_service.rs`：10 万行 XLSX 生成中硬终止；S3 PUT 已发出但未响应时硬终止并恢复原对象；真实数据库完成事务返回成功后截断响应，同一事务 COMMIT 重试且完成正文不重跑 | 已验证 |
| 安装后的原生大文件、打开/保存对话框、取消和失败恢复 | 需要真实 WebView/OS 选择器和 Windows 安装包流程；用户此前按 Escape 停止电脑操作，尚未恢复 | 待验收 |

生产代码基线 `1605edb` 的完整回归为 Rust 309 项常规测试、工具 4 项、客户端 94 项、浏览器 41 项通过，严格检查和生产构建通过。后续本轮只增加测试与驱动脚本：四项常规进程测试通过，S3 五项故障及一项真实进程 PUT 终止测试通过，严格 Clippy/格式检查通过。默认忽略的专项测试须按其专用驱动显式运行，不能把 ignored 当作通过。

Mongo 故障代理只作用于专用 `tri_admin_test_*` 数据库的子进程连接，按目标任务完成更新、会话和事务编号识别 COMMIT。只有从真实数据库收到成功响应后才关闭连接；其他消息保持原字节转发，不伪造成功，不修改产品认证或事务代码。协议依据为 [MongoDB wire protocol](https://www.mongodb.com/docs/manual/reference/mongodb-wire-protocol/) 和 [事务重试规范](https://specifications.readthedocs.io/en/latest/transactions/transactions/)。

后续先推进不依赖原生电脑控制的 Task 8。原生待办保留在本计划及 Task 12 安装/升级验收中；不能将 Task 7 或整个目标提前标记完成。客户/客户端管理始终不在产品范围内，Tauri 桌面程序继续保留。
