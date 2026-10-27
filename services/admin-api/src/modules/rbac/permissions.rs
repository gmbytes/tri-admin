use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Permission {
    pub code: &'static str,
    pub label: &'static str,
    pub group: &'static str,
}

pub const CATALOG: &[Permission] = &[
    Permission {
        code: "ai.read",
        label: "查看 AI 上游与运行",
        group: "ai",
    },
    Permission {
        code: "ai.write",
        label: "管理批准的 AI 上游",
        group: "ai",
    },
    Permission {
        code: "ai.invoke",
        label: "执行和取消本人 AI 运行",
        group: "ai",
    },
    Permission {
        code: "applicationLogs.read",
        label: "查看应用调用日志",
        group: "applications",
    },
    Permission {
        code: "applications.read",
        label: "查看开放应用配置",
        group: "applications",
    },
    Permission {
        code: "applications.write",
        label: "管理开放应用及服务密钥",
        group: "applications",
    },
    Permission {
        code: "applicationUsers.read",
        label: "查看应用用户",
        group: "applicationUsers",
    },
    Permission {
        code: "applicationUsers.write",
        label: "管理应用用户及密码",
        group: "applicationUsers",
    },
    Permission {
        code: "apiTokens.read",
        label: "查看本人 API Token 与调用日志",
        group: "apiTokens",
    },
    Permission {
        code: "apiTokens.write",
        label: "签发和撤销本人 API Token",
        group: "apiTokens",
    },
    Permission {
        code: "dashboard.read",
        label: "查看工作台",
        group: "dashboard",
    },
    Permission {
        code: "users.read",
        label: "查看用户",
        group: "users",
    },
    Permission {
        code: "users.write",
        label: "管理用户",
        group: "users",
    },
    Permission {
        code: "users.resetPassword",
        label: "重置密码",
        group: "users",
    },
    Permission {
        code: "roles.read",
        label: "查看角色",
        group: "roles",
    },
    Permission {
        code: "roles.write",
        label: "管理角色",
        group: "roles",
    },
    Permission {
        code: "roles.assign",
        label: "分配角色",
        group: "roles",
    },
    Permission {
        code: "permissions.read",
        label: "查看权限目录",
        group: "permissions",
    },
    Permission {
        code: "menus.read",
        label: "查看菜单",
        group: "menus",
    },
    Permission {
        code: "menus.write",
        label: "管理菜单",
        group: "menus",
    },
    Permission {
        code: "dictionaries.read",
        label: "查看字典",
        group: "dictionaries",
    },
    Permission {
        code: "dictionaries.write",
        label: "管理字典",
        group: "dictionaries",
    },
    Permission {
        code: "parameters.read",
        label: "查看参数",
        group: "parameters",
    },
    Permission {
        code: "parameters.write",
        label: "管理参数",
        group: "parameters",
    },
    Permission {
        code: "audit.read",
        label: "查看审计日志",
        group: "audit",
    },
    Permission {
        code: "audit.delete",
        label: "清理日志",
        group: "audit",
    },
    Permission {
        code: "files.read",
        label: "查看文件",
        group: "files",
    },
    Permission {
        code: "files.write",
        label: "管理文件",
        group: "files",
    },
    Permission {
        code: "exports.read",
        label: "查看导出模板",
        group: "exports",
    },
    Permission {
        code: "exports.write",
        label: "管理导入导出",
        group: "exports",
    },
    Permission {
        code: "apps.read",
        label: "查看开放应用",
        group: "apps",
    },
    Permission {
        code: "apps.write",
        label: "管理开放应用",
        group: "apps",
    },
    Permission {
        code: "tokens.read",
        label: "查看 API Token",
        group: "tokens",
    },
    Permission {
        code: "tokens.write",
        label: "签发与撤销 Token",
        group: "tokens",
    },
    Permission {
        code: "codegen.read",
        label: "查看生成器",
        group: "codegen",
    },
    Permission {
        code: "codegen.write",
        label: "生成代码",
        group: "codegen",
    },
    Permission {
        code: "skills.read",
        label: "查看 Skills",
        group: "skills",
    },
    Permission {
        code: "skills.write",
        label: "管理 Skills 与插件",
        group: "skills",
    },
    Permission {
        code: "skills.constraints.write",
        label: "管理 Skill 全局约束",
        group: "skills",
    },
    Permission {
        code: "mcp.read",
        label: "查看 MCP",
        group: "mcp",
    },
    Permission {
        code: "mcp.invoke",
        label: "调用 MCP 工具",
        group: "mcp",
    },
    Permission {
        code: "mcp.write",
        label: "管理 MCP",
        group: "mcp",
    },
    Permission {
        code: "workflows.read",
        label: "查看工作流",
        group: "workflows",
    },
    Permission {
        code: "workflows.write",
        label: "运行工作流",
        group: "workflows",
    },
    Permission {
        code: "settings.read",
        label: "查看配置",
        group: "settings",
    },
    Permission {
        code: "settings.write",
        label: "修改配置",
        group: "settings",
    },
    Permission {
        code: "versions.read",
        label: "查看版本",
        group: "versions",
    },
    Permission {
        code: "versions.write",
        label: "管理版本",
        group: "versions",
    },
    Permission {
        code: "game.read",
        label: "查询游戏服",
        group: "game",
    },
    Permission {
        code: "game.commands",
        label: "执行游戏管理命令",
        group: "game",
    },
];

pub fn known(code: &str) -> bool {
    CATALOG.iter().any(|permission| permission.code == code)
}
