use super::{import_model::*, models::*};
use crate::{
    common::{
        canonical,
        error::AppError,
        transaction::{Failure, fail},
    },
    modules::{
        dictionaries::models::{DictionaryRecord, ItemRecord},
        parameters::models::ParameterRecord,
        rbac::models::MenuRecord,
    },
};
use futures_util::TryStreamExt;
use mongodb::{
    ClientSession, Database,
    bson::{Bson, Document, doc},
};
use std::collections::HashMap;

pub async fn load(
    database: &Database,
    bundle: &Bundle,
    checksum: &str,
    session: &mut ClientSession,
) -> mongodb::error::Result<TargetState> {
    let mut state = TargetState::default();
    if !bundle.resources.menus.is_empty() {
        let mut cursor = database
            .collection::<MenuRecord>("menus")
            .find(doc! {"deletedAt":null})
            .limit(1001)
            .session(&mut *session)
            .await?;
        let rows = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        if rows.len() > 1000 {
            return Err(fail(Failure::Validation("目标菜单超过上限")));
        }
        let paths = rows
            .iter()
            .map(|row| (row.id, row.path.clone()))
            .collect::<HashMap<_, _>>();
        for row in rows {
            let parent_path = row
                .parent_id
                .map(|id| {
                    paths
                        .get(&id)
                        .cloned()
                        .ok_or_else(|| fail(Failure::Validation("目标菜单父级缺失")))
                })
                .transpose()?;
            state.menus.push(MenuState {
                id: row.id,
                version: row.version,
                config: Menu {
                    label: row.label,
                    path: row.path,
                    parent_path,
                    view_key: row.view_key,
                    icon: row.icon,
                    permission: row.permission,
                    sort: row.sort,
                    enabled: row.enabled,
                },
            });
        }
        state
            .menus
            .sort_by(|a, b| a.config.path.cmp(&b.config.path));
    }
    if !bundle.resources.dictionaries.is_empty() {
        let keys = bundle
            .resources
            .dictionaries
            .iter()
            .map(|item| item.key.clone())
            .collect::<Vec<_>>();
        let mut cursor = database
            .collection::<Document>("dictionaries")
            .find(doc! {"key":{"$in":keys},"deletedAt":null})
            .session(&mut *session)
            .await?;
        let rows = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        for raw in rows {
            let fence = raw
                .get_i64("writeFence")
                .map_err(|_| fail(Failure::Validation("目标字典缺少写入版本")))?;
            let row: DictionaryRecord = mongodb::bson::deserialize_from_document(raw)
                .map_err(|_| fail(Failure::Validation("目标字典结构无效")))?;
            state.dictionaries.push(DictionaryState {
                id: row.stamp.id,
                version: row.stamp.version,
                fence,
                config: Dictionary {
                    name: row.name,
                    key: row.key,
                    description: row.description,
                    enabled: row.enabled,
                    items: vec![],
                },
                items: vec![],
            });
        }
        let mut filters = vec![];
        for target in &state.dictionaries {
            let desired = bundle
                .resources
                .dictionaries
                .iter()
                .find(|item| item.key == target.config.key)
                .ok_or_else(|| fail(Failure::Validation("字典目标键无效")))?;
            if !desired.items.is_empty() {
                filters.push(doc!{"dictionaryId":target.id,"value":{"$in":desired.items.iter().map(|item|item.value.clone()).collect::<Vec<_>>()}});
            }
        }
        if !filters.is_empty() {
            let mut cursor = database
                .collection::<ItemRecord>("dictionary_items")
                .find(doc! {"$or":filters,"deletedAt":null})
                .limit(10001)
                .session(&mut *session)
                .await?;
            let items = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
            if items.len() > 10000 {
                return Err(fail(Failure::Validation("目标字典明细超过预览上限")));
            }
            let mut positions = state
                .dictionaries
                .iter()
                .enumerate()
                .map(|(index, item)| (item.id, index))
                .collect::<HashMap<_, _>>();
            for row in items {
                let index = *positions
                    .get_mut(&row.dictionary_id)
                    .ok_or_else(|| fail(Failure::Validation("目标字典明细引用无效")))?;
                state.dictionaries[index].items.push(ItemState {
                    id: row.stamp.id,
                    version: row.stamp.version,
                    config: Item {
                        label: row.label,
                        value: row.value,
                        sort: row.sort,
                        enabled: row.enabled,
                    },
                });
            }
        }
        for dictionary in &mut state.dictionaries {
            dictionary
                .items
                .sort_by(|a, b| a.config.value.cmp(&b.config.value));
            dictionary.config.items = dictionary
                .items
                .iter()
                .map(|item| item.config.clone())
                .collect();
        }
        state
            .dictionaries
            .sort_by(|a, b| a.config.key.cmp(&b.config.key));
    }
    if !bundle.resources.parameters.is_empty() {
        let keys = bundle
            .resources
            .parameters
            .iter()
            .map(|item| item.key.clone())
            .collect::<Vec<_>>();
        let mut cursor = database
            .collection::<ParameterRecord>("parameters")
            .find(doc! {"key":{"$in":keys},"deletedAt":null})
            .session(&mut *session)
            .await?;
        let rows = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        for row in rows {
            let value = serde_json::from_str(&row.value_json)
                .map_err(|_| fail(Failure::Validation("目标参数 JSON 无效")))?;
            state.parameters.push(ParameterState {
                id: row.id,
                version: row.version,
                config: Parameter {
                    name: row.name,
                    key: row.key,
                    group: row.group,
                    description: row.description,
                    value_type: row.value_type,
                    value,
                },
            });
        }
        state
            .parameters
            .sort_by(|a, b| a.config.key.cmp(&b.config.key));
    }
    if let Some(row) = database
        .collection::<Document>("versions")
        .find_one(doc! {"codeKey":bundle.release.code.to_ascii_lowercase()})
        .session(&mut *session)
        .await?
    {
        let id = row
            .get_object_id("_id")
            .map_err(|_| fail(Failure::Validation("目标版本标识无效")))?;
        let existing = row
            .get_str("checksum")
            .map_err(|_| fail(Failure::Validation("目标版本校验和无效")))?
            .to_owned();
        let deleted = row
            .get("deletedAt")
            .is_some_and(|value| !matches!(value, Bson::Null));
        let mut valid = false;
        if !deleted
            && existing == checksum
            && let Some(payload) = database
                .collection::<PackageRecord>("version_packages")
                .find_one(doc! {"_id":id})
                .session(&mut *session)
                .await?
        {
            valid = canonical::checksum(payload.package_json.as_bytes()) == existing
                && row.get_i64("byteLength").ok() == Some(payload.package_json.len() as i64);
        }
        state.version = Some(VersionState {
            id,
            version: row
                .get_i64("version")
                .map_err(|_| fail(Failure::Validation("目标版本计数无效")))?,
            checksum: existing,
            deleted,
            payload_valid: valid,
        });
    }
    Ok(state)
}
pub fn fingerprint(state: &TargetState) -> Result<String, AppError> {
    Ok(canonical::checksum(canonical::json(state)?.as_bytes()))
}
