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
];

pub fn known(code: &str) -> bool {
    CATALOG.iter().any(|permission| permission.code == code)
}
