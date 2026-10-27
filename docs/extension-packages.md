# 扩展包格式与维护

扩展库用于管理 Skills 和本项目插件。包内容作为数据保存；上传、编辑、启用和导出不会自动执行包内脚本，也不会把 allowed-tools 转换为管理权限。扩展库 API 与管理界面已接入，MCP 能力执行继续按 Task 9 B 交付。

## 界面使用

打开“Skills 与插件”，可新建 Skill，或安装已经打包的 ZIP。安装后默认停用；“启用/停用”修改记录状态，“更新 ZIP”保存独立内容修订。

“管理文件”提供 SKILL.md、插件描述和配套文件的原始文本编辑。可在 scripts、resources、references、templates 或 assets 中新增文本，也可上传、替换二进制资源。保存成功会生成新修订并停用扩展；旧修订只读，可查看文件或导出精确原包。版本冲突会保留当前草稿，使用“重新载入当前版本”核对后再修改。

“已卸载”列表保留原扩展 ID 下的历史入口，同名新安装不会接管旧历史。拥有全局约束写权限及全部数据范围的账号可在“全局约束”页签编辑规则，其他读取账号只能查看。

页面出现“待确认扩展操作”时，使用“恢复查询”核对原 UUID 与原修订，不能换请求重做。恢复记录按服务器/账号隔离，只保存操作标识及版本，不保存包、脚本或正文。未查到结果时保留标记；人工核对服务器后可以移除本机标记，该动作不会撤销或重发操作。

## Skill

ZIP 中只有一个根目录，其名称与 SKILL.md 的 name 相同。名称为 1–64 位小写字母、数字或短横线，不以短横线开头或结尾，也不包含连续短横线。

```text
record-helper/
  SKILL.md
  scripts/check.py
  resources/sample.json
  references/guide.md
  templates/record.txt
  assets/icon.png
```

SKILL.md 使用 Agent Skills 前置信息和 Markdown 正文，例如：

```markdown
---
name: record-helper
description: Validate a record using the supplied reference and template.
license: MIT
metadata:
  version: "1.0.0"
allowed-tools: parameters.list
---
# Record helper
Read the reference before preparing a record.
```

支持 name、description、license、compatibility、metadata 字符串映射和 allowed-tools。description 必填，最多 1024 字符。SKILL.md 总量最多 256 KiB，前置信息最多 16 KiB；不支持 YAML 文件包含、环境替换、锚点、别名、合并或未知字段。脚本、模板、参考和二进制资源保留原字节与可移植权限位。

## TRI 插件

插件根目录包含 tri-plugin.json，不能同时包含根 SKILL.md。formatVersion 固定为 1，version 使用语义版本；skills 列出包内 Skill 目录。MCP 贡献仅引用部署批准的 capabilityId，不能填写 shell 程序或任意参数。

```json
{
  "formatVersion": 1,
  "name": "record-plugin",
  "version": "1.0.0",
  "description": "Record maintenance helpers",
  "skills": ["skills/record-helper"],
  "mcp": [],
  "tools": []
}
```

工具绑定包含 name、description、operation、对象 inputSchema 和 mappings。operation 只接受已实现的参数、字典/明细和代码生成业务工具；mappings 的键是目标字段，值是已声明的输入属性字面名称，不解释路径或表达式。create/update 的平面字段由显式适配器放入原处理器的 input，路由 id 等字段仍在外层；明细查询的分页字段放入 query。

后端启用验证和实际 MCP 业务工具调用已接入，工具名为 `plugin.<extensionId>.<revisionId>.<toolName>`。更新或停用后旧工具名不转向新内容。发现和执行要求当前 skills.read、MCP 权限、扩展范围及实际业务权限；包说明和工具注解不授予权限。输入 Schema 与原处理器 Schema 都会验证，原业务对数字、字段和版本的限制仍生效。

MCP 贡献启用时必须引用当前部署批准的能力，tools 不能扩大部署白名单。启用不启动程序；在「MCP连接」选择「插件 MCP」后，按当前插件范围分页选择固定修订并显式启动。贡献为空工具子集时不会隐式取得全部工具。插件启停版本和内容修订变化会使旧绑定失效并关闭自身旧进程，重新启用后仍需明确重新绑定；历史结果和未知调用保持原身份。详细步骤见 [MCP 管理说明](mcp-programs.md)。

全部启用业务绑定与 25 项原生工具共用 MCP 128 项、256 KiB、深度 32 和节点 4096 的目录预算，启用超额会明确冲突。Schema 最大 64 KiB；参数映射前后都限制为 64 KiB，防止重复映射放大。Schema 不读取外部引用，支持包内 Schema 的本地引用。安装/启用本身不执行工具。受管 MCP 连接发起的调用继续使用既有 UUID 恢复和结果管理；直接协议调用沿用各业务操作自身的幂等合同，JSON-RPC id 不作为业务 UUID。

## 修订、历史与限制

最大 ZIP 为 4 MiB，最多 256 个文件，单文件最大 4 MiB，展开合计最大 16 MiB。所有路径使用 `/`、Unicode NFC 和可移植文件名；拒绝绝对路径、目录穿越、驱动器、反斜杠、设备名、链接、大小写重复和文件/目录冲突。包不会解压到上传者指定的服务器路径。

安装和每次内容修改都生成独立修订，默认停用。启停和卸载使用记录 version 防止覆盖他人变更；内容修改另增修订号。修改现有扩展不能更换名称或类型。卸载后原历史仍按当前权限和原 ID 授权访问，同名重新安装获得新 ID。

下载返回所选修订的原始 ZIP；服务端核对包、文件清单和摘要，响应带长度与 SHA-256。文本预览只支持不含 NUL 的有界 UTF-8，最大 256 KiB；二进制资源通过原包导出。全局约束最多 64 KiB，其写入需要 `skills.constraints.write` 和当前 all 数据范围，历史版本与审计保留。

创建和修订提交带规范 UUID requestId。响应丢失时先用 `/api/v1/extensions/by-request/{requestId}` 查询原结果；不要换 UUID 盲目重发。回执保存原结果快照，不表示后续启停或编辑后的最新状态。
