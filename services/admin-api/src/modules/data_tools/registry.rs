use crate::{common::error::AppError, modules::rbac::shared::Actor};
use serde::Serialize;
pub const PARAMETER_FIELDS: &[&str] =
    &["name", "key", "group", "description", "valueType", "value"];
pub const USER_FIELDS: &[&str] = &["id", "username", "displayName", "status", "version"];
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub key: &'static str,
    pub label: &'static str,
    pub fields: &'static [&'static str],
    pub importable: bool,
    pub read_permission: &'static str,
}
pub const RESOURCES: &[Resource] = &[
    Resource {
        key: "parameters",
        label: "系统参数",
        fields: PARAMETER_FIELDS,
        importable: true,
        read_permission: "parameters.read",
    },
    Resource {
        key: "users",
        label: "用户资料",
        fields: USER_FIELDS,
        importable: false,
        read_permission: "users.read",
    },
];
pub fn resource(key: &str, actor: &Actor, write: bool) -> Result<&'static Resource, AppError> {
    let resource = RESOURCES
        .iter()
        .find(|r| r.key == key)
        .ok_or_else(|| AppError::Validation("未注册的数据资源".into()))?;
    if !actor.can(resource.read_permission)
        || write && (!resource.importable || !actor.can("parameters.write"))
    {
        return Err(AppError::Forbidden);
    }
    Ok(resource)
}
pub fn fields(input: &[String], resource: &Resource) -> Result<Vec<String>, AppError> {
    let fields = if input.is_empty() {
        resource.fields.iter().map(|v| v.to_string()).collect()
    } else {
        input.to_vec()
    };
    if fields
        .iter()
        .any(|v| !resource.fields.contains(&v.as_str()))
        || fields
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != fields.len()
    {
        return Err(AppError::Validation("导出字段无效或重复".into()));
    }
    Ok(fields)
}
