# 系统参数

Task 6 的系统参数是有类型的业务配置资源，按全新 API 实现，不解析旧配置格式。采用 `parameters.read/write` 权限管理共享参数；数据库连接、邮件等服务秘密由后续配置中心负责脱敏与存储。

- `/api/v1/parameters` GET/POST，`/{id}` GET/PATCH/DELETE；字符串 ID、RFC3339、分页及版本冲突沿用新规范。
- 字段 name、key、group、description、valueType、value。key 为小写字母开头的 1–128 位字母、数字、点、下划线或短横线，活动记录唯一。name 最多 80 字符、group 最多 64 字符、description 最多 1000 字符。
- valueType 为 string/number/boolean/json。字符串允许空值；所有数字（包含 JSON 嵌套数字）采用跨前后端可准确表示的有限数值范围（绝对值不超过 9007199254740991），大整数应以字符串表达；JSON 支持 null 和嵌套值，总序列化长度不超过 16 KiB、嵌套不超过 16 层。
- BSON 持久化使用受限的 `valueJson` 字符串，返回时解析为 JSON 值，避免任意 JSON 键被当作 BSON 扩展类型或数据库字段路径，保留 JSON 类型和空值。该字段不暴露为 API 字段。
- PATCH 省略 value 保持原值；显式 null 仅对 json 类型有效。修改类型后必须与合并后的值匹配，不能悄悄转型。清空描述使用空字符串，元数据字段不接受 null。
- 版本更新、软删除与审计在同一 MongoDB 事务中提交；审计只记录动作和目标，不记录参数值。唯一索引保证并发创建时只有一个成功。
- 列表支持 key 的字面前缀搜索与 createdAt/key/name/group 排序，追加 ID 稳定排序；独立 schema 与菜单，就绪检查要求迁移完成。
- 页面提供类型选择和相应值输入、明确 JSON 错误、独立查询/分页/编辑状态；不在客户端做隐式值转换后直接保存。

验收覆盖类型往返、空值、超限/深度边界、唯一性、软删除、权限撤销、并发更新和 schema，以及真实 API 上的页面增改删。
