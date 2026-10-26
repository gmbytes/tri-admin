# 扩展包格式与维护

扩展库用于管理 Skills 和本项目插件。包内容作为数据保存；上传、编辑、启用和导出不会自动执行包内脚本，也不会把 allowed-tools 转换为管理权限。扩展库 API 已接入，管理界面与 MCP 能力执行继续按 Task 9 B 交付。

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

工具绑定包含 name、description、operation、对象 inputSchema 和 mappings；映射值只能引用已声明的输入属性，不接受表达式。含 MCP 或工具贡献的插件当前可停用保存，能力登记与执行仍待后续实现，启用会返回明确冲突。

## 修订、历史与限制

最大 ZIP 为 4 MiB，最多 256 个文件，单文件最大 4 MiB，展开合计最大 16 MiB。所有路径使用 `/`、Unicode NFC 和可移植文件名；拒绝绝对路径、目录穿越、驱动器、反斜杠、设备名、链接、大小写重复和文件/目录冲突。包不会解压到上传者指定的服务器路径。

安装和每次内容修改都生成独立修订，默认停用。启停和卸载使用记录 version 防止覆盖他人变更；内容修改另增修订号。修改现有扩展不能更换名称或类型。卸载后原历史仍按当前权限和原 ID 授权访问，同名重新安装获得新 ID。

下载返回所选修订的原始 ZIP；服务端核对包、文件清单和摘要，响应带长度与 SHA-256。文本预览只支持不含 NUL 的有界 UTF-8，最大 256 KiB；二进制资源通过原包导出。全局约束最多 64 KiB，其写入需要 `skills.constraints.write` 和当前 all 数据范围，历史版本与审计保留。

创建和修订提交带规范 UUID requestId。响应丢失时先用 `/api/v1/extensions/by-request/{requestId}` 查询原结果；不要换 UUID 盲目重发。回执保存原结果快照，不表示后续启停或编辑后的最新状态。
