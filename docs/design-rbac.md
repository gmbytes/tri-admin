# 用户、角色、菜单与权限设计

Task 4 使用全新数据结构与 REST API，不保留旧系统语义或适配。

## 资源

- `/api/v1/users`：GET 分页、POST 创建；`/{id}` GET/PATCH/DELETE；`/{id}/password` POST 重置密码。
- `/api/v1/roles`：GET/POST；`/{id}` PATCH/DELETE。
- `/api/v1/permissions`：GET 内置操作码目录。
- `/api/v1/menus`：GET/POST；`/{id}` PATCH/DELETE。
- `/api/v1/navigation`：当前用户可见的启用菜单树；仅需有效登录。

列表默认 page=1/pageSize=20，最大 100，使用有索引的稳定排序（末尾追加 `_id`）。用户名搜索是转义后的字面前缀，不允许任意正则或查询对象。

## 模型与规则

- 用户：username、usernameKey、displayName、passwordHash、roleIds、status(active/disabled)、version、createdAt/updatedAt/deletedAt。
- 创建用户名为 3–64 个字符，仅字母/数字/下划线/点/短横线；密码至少 12 字符，最大 1024 字节；显示名 1–80 字符。验证逻辑在 bootstrap、用户创建和登录中统一。
- 更新使用 version 乐观并发控制；角色数组不能重复，必须引用活动角色。
- 角色：name、slug、permissions、dataScope(all/self/roles)、dataScopeRoleIds、protected、version 和时间。slug 唯一，权限只能来自目录；`*` 仅系统 Owner 使用，外部角色接口不能创建通配权限。指定角色模式、聚合和委派规则见 [客户与范围设计](design-customer-scope.md)。
- Owner 角色不可删除或改变其授权；至少保留一名启用且未删除的 Owner 用户。
- Owner 用户的禁用、删除和移除 Owner 角色须在事务中完成；事务首先更新共享 `_invariants/rbac` 记录，再检查数量，防止并发写偏斜导致最后两名 Owner 同时被禁用。
- 非 Owner 不能修改 Owner 用户，也不能分配 Owner 角色。角色分配要求 `roles.assign`；所授权限不得超过操作者已有权限，不能把 self 数据范围扩大为 all。
- self 数据范围的用户列表只包含自己；不能创建或管理其他用户。roles 范围包含自己和指定角色成员，范围过滤先于分页；新建/调整成员后仍须处于操作者范围，角色分配也须通过成员资格、权限与范围检查。客户资源继续接入同一能力。
- 用户禁用、删除、密码重置时撤销其全部会话；下一次请求立即失效。
- 角色权限从数据库读取，角色变更立即生效。正在进行中的请求仍遵循该请求认证时得到的授权上下文。
- 菜单：label、path、viewKey、icon、permission、parentId、sort、enabled、version 和时间；viewKey 来自新客户端注册表，不执行任意组件代码。
- 菜单路径活动唯一；父级不能形成环，深度上限 16；存在子节点时删除返回 409。导航树过滤禁用分支和无权限节点，无可见子节点的纯目录不展示。
- 角色使用操作码控制菜单和按钮，不建立旧菜单/按钮关联表。
- 所有成功的写操作与审计事件同事务；审计不记录密码或密码摘要。

## 关键测试

1. 未登录 401，无权限 403；Owner 可管理，Viewer 仅获授权读取。
2. 重名用户/角色 409，非法权限/角色引用/参数 422。
3. 新用户能登录，返回数据不包含密码材料；用户禁用、删除和重置密码撤销已有会话。
4. 旧 version 更新 409，重复提交不丢失其他修改。
5. 删除/禁用最后 Owner、改受保护角色 409。
6. 两个 Owner 并发禁用自己，最终必须仍有一名活动 Owner。
7. 有限权限操作者不能授予 Owner、通配或超出自身的权限。
8. self 范围列表不能泄漏其他用户。
9. 菜单环与非法 viewKey 拒绝；删除带子节点菜单拒绝；导航只展示可见启用树。

新模型演进由版本化 MongoDB 迁移负责；这属于本项目自身的升级机制，不是旧系统兼容层。
