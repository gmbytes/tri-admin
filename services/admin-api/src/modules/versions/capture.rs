use super::{dto::CaptureInput, models::*};
use crate::{
    common::{
        error::AppError,
        transaction::{Failure, fail},
        validation,
    },
    modules::{
        api_catalog,
        dictionaries::{
            self,
            models::{DictionaryRecord, ItemRecord},
        },
        menus,
        parameters::{self, models::ParameterRecord},
        rbac::{models::MenuRecord, shared::Actor},
    },
};
use futures_util::TryStreamExt;
use mongodb::{
    ClientSession, Database,
    bson::{doc, oid::ObjectId},
};
use std::collections::{BTreeSet, HashMap, HashSet};

pub struct Selection {
    pub menu_ids: Vec<ObjectId>,
    pub dictionary_ids: Vec<ObjectId>,
    pub parameter_ids: Vec<ObjectId>,
    pub apis: Vec<ApiDependency>,
    pub read_permissions: Vec<String>,
}
fn ids(raw: &[String], max: usize) -> Result<Vec<ObjectId>, AppError> {
    if raw.len() > max {
        return Err(AppError::Validation("选择项重复或超过数量上限".into()));
    }
    let ids = raw
        .iter()
        .map(|value| validation::object_id(value))
        .collect::<Result<Vec<_>, _>>()?;
    if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err(AppError::Validation("选择项重复".into()));
    }
    Ok(ids)
}
pub fn prepare(actor: &Actor, input: &CaptureInput) -> Result<Selection, AppError> {
    validation::text(&input.name, "版本名称", 80)?;
    if input.code.is_empty()
        || input.code.len() > 64
        || !input
            .code
            .bytes()
            .next()
            .is_some_and(|value| value.is_ascii_alphanumeric())
        || !input
            .code
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || b"._-".contains(&value))
        || input.description.chars().count() > 1000
    {
        return Err(AppError::Validation("版本编号或描述格式无效".into()));
    }
    let menu_ids = ids(&input.menu_ids, 1000)?;
    let dictionary_ids = ids(&input.dictionary_ids, 100)?;
    let parameter_ids = ids(&input.parameter_ids, 100)?;
    if input.api_operation_ids.len() > 1000
        || input.api_operation_ids.iter().collect::<HashSet<_>>().len()
            != input.api_operation_ids.len()
    {
        return Err(AppError::Validation("接口选择重复或超过数量上限".into()));
    }
    if menu_ids.is_empty()
        && dictionary_ids.is_empty()
        && parameter_ids.is_empty()
        && input.api_operation_ids.is_empty()
    {
        return Err(AppError::Validation("请至少选择一项配置或接口依赖".into()));
    }
    let mut reads = BTreeSet::new();
    for (selected, permission) in [
        (!menu_ids.is_empty(), "menus.read"),
        (!dictionary_ids.is_empty(), "dictionaries.read"),
        (!parameter_ids.is_empty(), "parameters.read"),
        (!input.api_operation_ids.is_empty(), "permissions.read"),
    ] {
        if selected {
            if !actor.can(permission) {
                return Err(AppError::Forbidden);
            }
            reads.insert(permission.to_owned());
        }
    }
    let available = api_catalog::endpoints()
        .into_iter()
        .map(|entry| (entry.operation_id, entry))
        .collect::<HashMap<_, _>>();
    let mut apis = vec![];
    for id in &input.api_operation_ids {
        let entry = available
            .get(id.as_str())
            .ok_or_else(|| AppError::Validation("所选接口不在当前服务目录中".into()))?;
        apis.push(ApiDependency {
            operation_id: id.clone(),
            method: entry.method.into(),
            path: entry.path.into(),
            required_permissions: entry
                .required_permissions
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
        });
    }
    apis.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
    Ok(Selection {
        menu_ids,
        dictionary_ids,
        parameter_ids,
        apis,
        read_permissions: reads.into_iter().collect(),
    })
}
pub async fn resources(
    database: &Database,
    selection: &Selection,
    session: &mut ClientSession,
) -> mongodb::error::Result<(Resources, Counts)> {
    let mut result = Resources::default();
    let mut counts = Counts::default();
    if !selection.menu_ids.is_empty() {
        let mut cursor = database
            .collection::<MenuRecord>("menus")
            .find(doc! {"deletedAt":null})
            .limit(1001)
            .session(&mut *session)
            .await?;
        let rows = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        if rows.len() > 1000 {
            return Err(fail(Failure::Validation("活动菜单超过快照上限")));
        }
        let rows = rows
            .into_iter()
            .map(|row| (row.id, row))
            .collect::<HashMap<_, _>>();
        let mut included = HashSet::new();
        for id in &selection.menu_ids {
            let mut next = Some(*id);
            let mut seen = HashSet::new();
            while let Some(id) = next {
                if !seen.insert(id) || seen.len() > 16 {
                    return Err(fail(Failure::Validation("源菜单层级无效")));
                }
                let row = rows
                    .get(&id)
                    .ok_or_else(|| fail(Failure::Validation("所选菜单或其父级不存在")))?;
                included.insert(id);
                next = row.parent_id;
            }
        }
        for id in included {
            let row = &rows[&id];
            menus::service::validate(
                &row.label,
                &row.path,
                &row.view_key,
                &row.icon,
                &row.permission,
            )
            .map_err(|_| fail(Failure::Validation("源菜单配置无效")))?;
            result.menus.push(Menu {
                label: row.label.clone(),
                path: row.path.clone(),
                parent_path: row.parent_id.map(|id| rows[&id].path.clone()),
                view_key: row.view_key.clone(),
                icon: row.icon.clone(),
                permission: row.permission.clone(),
                sort: row.sort,
                enabled: row.enabled,
            });
        }
        result.menus.sort_by(|a, b| a.path.cmp(&b.path));
        counts.menus = result.menus.len() as i64;
        counts.ancestors = counts.menus - selection.menu_ids.len() as i64;
    }
    if !selection.dictionary_ids.is_empty() {
        let mut cursor = database
            .collection::<DictionaryRecord>("dictionaries")
            .find(doc! {"_id":{"$in":&selection.dictionary_ids},"deletedAt":null})
            .session(&mut *session)
            .await?;
        let rows = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        if rows.len() != selection.dictionary_ids.len() {
            return Err(fail(Failure::Validation("所选字典不存在或已删除")));
        }
        let mut cursor = database
            .collection::<ItemRecord>("dictionary_items")
            .find(doc! {"dictionaryId":{"$in":&selection.dictionary_ids},"deletedAt":null})
            .limit(10001)
            .session(&mut *session)
            .await?;
        let items = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        if items.len() > 10000 {
            return Err(fail(Failure::PayloadTooLarge));
        }
        counts.dictionary_items = items.len() as i64;
        let mut grouped = HashMap::<ObjectId, Vec<Item>>::new();
        for item in items {
            dictionaries::service::validate_item(&item.label, &item.value)
                .map_err(|_| fail(Failure::Validation("源字典明细无效")))?;
            grouped.entry(item.dictionary_id).or_default().push(Item {
                label: item.label,
                value: item.value,
                sort: item.sort,
                enabled: item.enabled,
            });
        }
        for row in rows {
            dictionaries::service::validate_dictionary(&row.name, &row.key, &row.description)
                .map_err(|_| fail(Failure::Validation("源字典配置无效")))?;
            let mut items = grouped.remove(&row.stamp.id).unwrap_or_default();
            items.sort_by(|a, b| a.value.cmp(&b.value));
            result.dictionaries.push(Dictionary {
                name: row.name,
                key: row.key,
                description: row.description,
                enabled: row.enabled,
                items,
            });
        }
        result.dictionaries.sort_by(|a, b| a.key.cmp(&b.key));
        counts.dictionaries = result.dictionaries.len() as i64;
    }
    if !selection.parameter_ids.is_empty() {
        let mut cursor = database
            .collection::<ParameterRecord>("parameters")
            .find(doc! {"_id":{"$in":&selection.parameter_ids},"deletedAt":null})
            .session(&mut *session)
            .await?;
        let rows = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        if rows.len() != selection.parameter_ids.len() {
            return Err(fail(Failure::Validation("所选参数不存在或已删除")));
        }
        for row in rows {
            let value = serde_json::from_str(&row.value_json)
                .map_err(|_| fail(Failure::Validation("源参数 JSON 无效")))?;
            parameters::service::validate(
                &row.name,
                &row.key,
                &row.group,
                &row.description,
                row.value_type,
                &value,
            )
            .map_err(|_| fail(Failure::Validation("源参数配置无效")))?;
            result.parameters.push(Parameter {
                name: row.name,
                key: row.key,
                group: row.group,
                description: row.description,
                value_type: row.value_type,
                value,
            });
        }
        result.parameters.sort_by(|a, b| a.key.cmp(&b.key));
        counts.parameters = result.parameters.len() as i64;
    }
    counts.apis = selection.apis.len() as i64;
    Ok((result, counts))
}
