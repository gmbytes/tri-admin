use super::{capture, models::*};
use crate::{
    common::{canonical, error::AppError},
    modules::{api_catalog, dictionaries, menus, parameters, rbac::shared::Actor},
};
use std::collections::{BTreeSet, HashMap, HashSet};
pub struct Validated {
    pub bundle: Bundle,
    pub json: String,
    pub checksum: String,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
}
fn unique<'a>(values: impl Iterator<Item = &'a str>, max: usize) -> Result<(), AppError> {
    let values = values.collect::<Vec<_>>();
    if values.len() > max || values.iter().copied().collect::<HashSet<_>>().len() != values.len() {
        return Err(AppError::Validation("配置项重复或超过上限".into()));
    }
    Ok(())
}
pub fn validate(actor: &Actor, mut bundle: Bundle) -> Result<Validated, AppError> {
    if canonical::json(&bundle)?.len() > MAX_PACKAGE_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    if bundle.format != "tri-admin/config-bundle" || bundle.schema_version != 1 {
        return Err(AppError::Validation(
            "仅支持 tri-admin/config-bundle 新格式 schemaVersion=1".into(),
        ));
    }
    capture::validate_release(
        &bundle.release.name,
        &bundle.release.code,
        &bundle.release.description,
    )?;
    if bundle.release.captured_at.len() > 64
        || chrono::DateTime::parse_from_rfc3339(&bundle.release.captured_at).is_err()
    {
        return Err(AppError::Validation("发版捕获时间格式无效".into()));
    }
    unique(
        bundle.resources.menus.iter().map(|item| item.path.as_str()),
        1000,
    )?;
    unique(
        bundle
            .resources
            .dictionaries
            .iter()
            .map(|item| item.key.as_str()),
        100,
    )?;
    unique(
        bundle
            .resources
            .parameters
            .iter()
            .map(|item| item.key.as_str()),
        100,
    )?;
    unique(
        bundle
            .required_apis
            .iter()
            .map(|item| item.operation_id.as_str()),
        1000,
    )?;
    if bundle.resources.menus.is_empty()
        && bundle.resources.dictionaries.is_empty()
        && bundle.resources.parameters.is_empty()
        && bundle.required_apis.is_empty()
    {
        return Err(AppError::Validation("配置包不能全部为空".into()));
    }
    for item in &bundle.resources.menus {
        menus::service::validate(
            &item.label,
            &item.path,
            &item.view_key,
            &item.icon,
            &item.permission,
        )?;
    }
    let mut total = 0;
    for dictionary in &mut bundle.resources.dictionaries {
        dictionaries::service::validate_dictionary(
            &dictionary.name,
            &dictionary.key,
            &dictionary.description,
        )?;
        unique(
            dictionary.items.iter().map(|item| item.value.as_str()),
            10000,
        )?;
        total += dictionary.items.len();
        for item in &dictionary.items {
            dictionaries::service::validate_item(&item.label, &item.value)?;
        }
        dictionary.items.sort_by(|a, b| a.value.cmp(&b.value));
    }
    if total > 10000 {
        return Err(AppError::PayloadTooLarge);
    }
    for item in &bundle.resources.parameters {
        parameters::service::validate(
            &item.name,
            &item.key,
            &item.group,
            &item.description,
            item.value_type,
            &item.value,
        )?;
    }
    let catalog = api_catalog::endpoints()
        .into_iter()
        .map(|item| (item.operation_id, item))
        .collect::<HashMap<_, _>>();
    for dependency in &mut bundle.required_apis {
        let target = catalog
            .get(dependency.operation_id.as_str())
            .ok_or_else(|| AppError::Validation("接口依赖不在目标服务目录中".into()))?;
        dependency.required_permissions.sort();
        if dependency.method != target.method
            || dependency.path != target.path
            || dependency
                .required_permissions
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                != target.required_permissions
        {
            return Err(AppError::Validation("接口依赖与目标服务能力不匹配".into()));
        }
    }
    let mut reads = BTreeSet::new();
    let mut writes = BTreeSet::new();
    for (selected, read, write) in [
        (
            !bundle.resources.menus.is_empty(),
            "menus.read",
            Some("menus.write"),
        ),
        (
            !bundle.resources.dictionaries.is_empty(),
            "dictionaries.read",
            Some("dictionaries.write"),
        ),
        (
            !bundle.resources.parameters.is_empty(),
            "parameters.read",
            Some("parameters.write"),
        ),
        (!bundle.required_apis.is_empty(), "permissions.read", None),
    ] {
        if selected {
            if !actor.can(read) {
                return Err(AppError::Forbidden);
            }
            reads.insert(read.to_owned());
            if let Some(write) = write {
                writes.insert(write.to_owned());
            }
        }
    }
    bundle.resources.menus.sort_by(|a, b| a.path.cmp(&b.path));
    bundle
        .resources
        .dictionaries
        .sort_by(|a, b| a.key.cmp(&b.key));
    bundle
        .resources
        .parameters
        .sort_by(|a, b| a.key.cmp(&b.key));
    bundle
        .required_apis
        .sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
    let json = canonical::json(&bundle)?;
    let checksum = canonical::checksum(json.as_bytes());
    Ok(Validated {
        bundle,
        json,
        checksum,
        reads: reads.into_iter().collect(),
        writes: writes.into_iter().collect(),
    })
}
