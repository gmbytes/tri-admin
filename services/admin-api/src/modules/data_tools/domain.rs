use super::{
    formats::{Cell, Table},
    models::ImportRow,
    registry,
};
use crate::{
    common::{
        error::AppError,
        transaction::{Failure, fail},
    },
    modules::{
        dictionaries::{
            dto::{CreateDictionary, CreateItem},
            service as dictionaries,
        },
        parameters::{dto::CreateParameter, service as parameters},
        rbac::shared::Actor,
    },
};
use futures_util::TryStreamExt;
use mongodb::{
    ClientSession, Database,
    bson::{Document, doc, oid::ObjectId},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ItemInput {
    dictionary_key: String,
    label: String,
    value: String,
    sort: i32,
    enabled: bool,
}
impl ItemInput {
    fn item(&self) -> CreateItem {
        CreateItem {
            label: self.label.clone(),
            value: self.value.clone(),
            sort: self.sort,
            enabled: self.enabled,
        }
    }
}
fn invalid() -> AppError {
    AppError::Validation("导入字段或映射无效".into())
}
fn boolean(cell: Cell) -> Option<bool> {
    match cell {
        Cell::Empty => Some(true),
        Cell::Text(v) if v.is_empty() => Some(true),
        Cell::Boolean(v) => Some(v),
        Cell::Text(v) if v == "true" => Some(true),
        Cell::Text(v) if v == "false" => Some(false),
        _ => None,
    }
}
fn integer(cell: Cell) -> Option<i32> {
    match cell {
        Cell::Empty => Some(0),
        Cell::Text(v) if v.is_empty() => Some(0),
        Cell::Number(v)
            if v.is_finite()
                && v.fract() == 0.0
                && v >= i32::MIN as f64
                && v <= i32::MAX as f64 =>
        {
            Some(v as i32)
        }
        Cell::Text(v) => v.parse().ok(),
        _ => None,
    }
}
pub fn rows(
    resource: &str,
    table: Table,
    mapping: &BTreeMap<String, String>,
) -> Result<Vec<ImportRow>, AppError> {
    let definition = registry::definition(resource)?;
    let mapping = if mapping.is_empty() {
        definition
            .fields
            .iter()
            .filter(|f| table.headers.iter().any(|h| h == **f))
            .map(|f| (f.to_string(), f.to_string()))
            .collect()
    } else {
        mapping.clone()
    };
    if mapping
        .keys()
        .any(|k| !definition.fields.contains(&k.as_str()))
        || definition
            .required_fields
            .iter()
            .any(|k| !mapping.contains_key(*k))
        || mapping.values().collect::<HashSet<_>>().len() != mapping.len()
        || mapping.values().any(|h| !table.headers.contains(h))
    {
        return Err(invalid());
    }
    let indices = mapping
        .iter()
        .map(|(field, header)| {
            (
                field.as_str(),
                table.headers.iter().position(|h| h == header).unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut output = Vec::new();
    let mut counts = HashMap::<String, usize>::new();
    let mut identities = Vec::new();
    let mut total = 0;
    for (index, row) in table.rows.into_iter().enumerate() {
        let cell = |key: &str| {
            indices
                .get(key)
                .map(|i| row[*i].clone())
                .unwrap_or(Cell::Empty)
        };
        let text = |key: &str| cell(key).text();
        let enabled = boolean(cell("enabled"));
        let (values, valid, key, identity) = match resource {
            "dictionaries" => {
                let values = json!({"name":text("name"),"key":text("key"),"description":text("description"),"enabled":enabled.map(Value::Bool).unwrap_or_else(||Value::String(text("enabled")))});
                let valid = enabled.is_some()
                    && ["name", "key", "description"]
                        .iter()
                        .all(|k| matches!(cell(k), Cell::Text(_) | Cell::Empty))
                    && dictionaries::validate_dictionary(
                        &text("name"),
                        &text("key"),
                        &text("description"),
                    )
                    .is_ok();
                (values, valid, text("key"), text("key"))
            }
            "dictionaryItems" => {
                let sort = integer(cell("sort"));
                let values = json!({"dictionaryKey":text("dictionaryKey"),"label":text("label"),"value":text("value"),"sort":sort.map(|v|json!(v)).unwrap_or_else(||Value::String(text("sort"))),"enabled":enabled.map(Value::Bool).unwrap_or_else(||Value::String(text("enabled")))});
                let valid = enabled.is_some()
                    && sort.is_some()
                    && ["dictionaryKey", "label", "value"]
                        .iter()
                        .all(|k| matches!(cell(k), Cell::Text(_) | Cell::Empty))
                    && dictionaries::validate_dictionary("parent", &text("dictionaryKey"), "")
                        .is_ok()
                    && dictionaries::validate_item(&text("label"), &text("value")).is_ok();
                let identity = serde_json::to_string(&[text("dictionaryKey"), text("value")])
                    .map_err(|_| invalid())?;
                (
                    values,
                    valid,
                    format!("{} / {}", text("dictionaryKey"), text("value")),
                    identity,
                )
            }
            _ => return Err(invalid()),
        };
        let values_json = serde_json::to_string(&values).map_err(|_| invalid())?;
        total += values_json.len();
        if total > 8 * 1024 * 1024 {
            return Err(AppError::PayloadTooLarge);
        }
        *counts.entry(identity.clone()).or_default() += 1;
        identities.push(identity);
        output.push(ImportRow {
            index,
            key,
            status: if valid { "ready" } else { "invalid" }.into(),
            error: if valid {
                None
            } else {
                Some("INVALID_FIELDS".into())
            },
            target_id: None,
            values_json,
            parent_id: None,
        });
    }
    for (row, identity) in output.iter_mut().zip(identities) {
        if counts[&identity] > 1 {
            row.status = "duplicate".into();
            row.error = Some("DUPLICATE_IN_FILE".into());
        }
    }
    Ok(output)
}
fn fields(row: &ImportRow) -> mongodb::error::Result<Value> {
    serde_json::from_str(&row.values_json).map_err(|_| fail(Failure::NotReady))
}
fn row_error(row: &mut ImportRow, status: &str, error: &str) {
    row.status = status.into();
    row.error = Some(error.into());
    row.target_id = None;
}
pub async fn annotate(
    database: &Database,
    resource: &str,
    rows: &mut [ImportRow],
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    if matches!(resource, "parameters" | "dictionaries") {
        let keys = rows
            .iter()
            .filter(|r| r.status == "ready")
            .map(|r| r.key.clone())
            .collect::<Vec<_>>();
        let mut cursor = database
            .collection::<Document>(resource)
            .find(doc! {"key":{"$in":keys},"deletedAt":null})
            .projection(doc! {"key":1})
            .session(&mut *session)
            .await?;
        let existing = cursor
            .stream(&mut *session)
            .try_collect::<Vec<_>>()
            .await?
            .into_iter()
            .filter_map(|d| d.get_str("key").ok().map(String::from))
            .collect::<HashSet<_>>();
        for row in rows {
            if row.status == "ready" && existing.contains(&row.key) {
                row_error(row, "duplicate", "KEY_EXISTS");
            }
        }
        return Ok(());
    }
    if resource != "dictionaryItems" {
        return Err(fail(Failure::NotReady));
    }
    let mut keys = HashSet::new();
    for row in rows.iter().filter(|r| r.status == "ready") {
        keys.insert(
            fields(row)?["dictionaryKey"]
                .as_str()
                .ok_or_else(|| fail(Failure::NotReady))?
                .to_owned(),
        );
    }
    let mut cursor = database
        .collection::<Document>("dictionaries")
        .find(doc! {"key":{"$in":keys.into_iter().collect::<Vec<_>>()},"deletedAt":null})
        .projection(doc! {"key":1})
        .session(&mut *session)
        .await?;
    let parents = cursor
        .stream(&mut *session)
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(|d| Ok((d.get_str("key")?.to_owned(), d.get_object_id("_id")?)))
        .collect::<Result<HashMap<_, _>, mongodb::bson::error::Error>>()
        .map_err(|_| fail(Failure::NotReady))?;
    let mut pairs = Vec::new();
    for row in rows.iter_mut().filter(|r| r.status == "ready") {
        let value = fields(row)?;
        let key = value["dictionaryKey"]
            .as_str()
            .ok_or_else(|| fail(Failure::NotReady))?;
        if let Some(parent) = parents.get(key) {
            row.parent_id = Some(*parent);
            pairs.push(doc!{"dictionaryId":parent,"value":value["value"].as_str().ok_or_else(||fail(Failure::NotReady))?});
        } else {
            row_error(row, "invalid", "PARENT_NOT_FOUND");
        }
    }
    if pairs.is_empty() {
        return Ok(());
    }
    let mut cursor = database
        .collection::<Document>("dictionary_items")
        .find(doc! {"$or":pairs,"deletedAt":null})
        .projection(doc! {"dictionaryId":1,"value":1})
        .session(&mut *session)
        .await?;
    let existing = cursor
        .stream(&mut *session)
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(|d| {
            Ok((
                d.get_object_id("dictionaryId")?,
                d.get_str("value")?.to_owned(),
            ))
        })
        .collect::<Result<HashSet<_>, mongodb::bson::error::Error>>()
        .map_err(|_| fail(Failure::NotReady))?;
    for row in rows.iter_mut().filter(|r| r.status == "ready") {
        let value = fields(row)?["value"]
            .as_str()
            .ok_or_else(|| fail(Failure::NotReady))?
            .to_owned();
        if existing.contains(&(row.parent_id.ok_or_else(|| fail(Failure::NotReady))?, value)) {
            row_error(row, "duplicate", "KEY_EXISTS");
        }
    }
    Ok(())
}
pub async fn apply_row(
    database: &Database,
    actor: &Actor,
    resource: &str,
    row: &mut ImportRow,
    target: ObjectId,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    match resource {
        "parameters" | "dictionaries" => {
            if database
                .collection::<Document>(resource)
                .find_one(doc! {"key":&row.key,"deletedAt":null})
                .session(&mut *session)
                .await?
                .is_some()
            {
                row_error(row, "conflict", "KEY_EXISTS");
                return Ok(());
            }
            if resource == "parameters" {
                let input: CreateParameter =
                    serde_json::from_str(&row.values_json).map_err(|_| fail(Failure::NotReady))?;
                parameters::create_in(database, actor, &input, target, session).await?;
            } else {
                let input: CreateDictionary =
                    serde_json::from_str(&row.values_json).map_err(|_| fail(Failure::NotReady))?;
                dictionaries::create_in(database, actor, &input, target, session).await?;
            }
        }
        "dictionaryItems" => {
            let input: ItemInput =
                serde_json::from_str(&row.values_json).map_err(|_| fail(Failure::NotReady))?;
            let parent = row.parent_id.ok_or_else(|| fail(Failure::NotReady))?;
            if database
                .collection::<Document>("dictionaries")
                .find_one(doc! {"_id":parent,"key":&input.dictionary_key,"deletedAt":null})
                .session(&mut *session)
                .await?
                .is_none()
            {
                row_error(row, "conflict", "PARENT_CHANGED");
                return Ok(());
            }
            if database
                .collection::<Document>("dictionary_items")
                .find_one(doc! {"dictionaryId":parent,"value":&input.value,"deletedAt":null})
                .session(&mut *session)
                .await?
                .is_some()
            {
                row_error(row, "conflict", "KEY_EXISTS");
                return Ok(());
            }
            dictionaries::create_item_in(database, actor, parent, &input.item(), target, session)
                .await?;
        }
        _ => return Err(fail(Failure::NotReady)),
    }
    row.status = "applied".into();
    row.error = None;
    row.target_id = Some(target.to_hex());
    Ok(())
}
