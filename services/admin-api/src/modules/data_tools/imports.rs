use super::{
    formats::{self, Cell, Table},
    models::*,
    registry,
};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::{
        parameters::{dto::CreateParameter, service as parameters},
        rbac::shared::{self, Actor},
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{self, DateTime, Document, doc, oid::ObjectId},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

fn invalid() -> AppError {
    AppError::Validation("导入字段、映射或请求标识无效".into())
}
pub fn prepare(input: &PreviewInput) -> Result<(String, Vec<ImportRow>), AppError> {
    if uuid::Uuid::parse_str(&input.request_id)
        .ok()
        .is_none_or(|id| id.to_string() != input.request_id)
        || input.content_base64.len() > formats::MAX_FILE_BYTES.div_ceil(3) * 4
    {
        return Err(invalid());
    }
    let bytes = STANDARD
        .decode(&input.content_base64)
        .map_err(|_| invalid())?;
    let checksum = format!("{:x}", Sha256::digest(&bytes));
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(&input.resource, input.format, &input.mapping, checksum))
                .map_err(|_| invalid())?
        )
    );
    let table = formats::decode(input.format, &bytes)?;
    let rows = rows(table, &input.mapping)?;
    Ok((fingerprint, rows))
}
fn rows(table: Table, mapping: &BTreeMap<String, String>) -> Result<Vec<ImportRow>, AppError> {
    let mapping = if mapping.is_empty() {
        registry::PARAMETER_FIELDS
            .iter()
            .filter(|field| table.headers.iter().any(|h| h == **field))
            .map(|field| (field.to_string(), field.to_string()))
            .collect()
    } else {
        mapping.clone()
    };
    if mapping
        .keys()
        .any(|k| !registry::PARAMETER_FIELDS.contains(&k.as_str()))
        || ["name", "key", "valueType", "value"]
            .iter()
            .any(|k| !mapping.contains_key(*k))
        || mapping.values().collect::<HashSet<_>>().len() != mapping.len()
        || mapping
            .values()
            .any(|header| !table.headers.contains(header))
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
    let mut total = 0;
    for (index, row) in table.rows.into_iter().enumerate() {
        let cell = |key: &str| {
            indices
                .get(key)
                .map(|i| row[*i].clone())
                .unwrap_or(Cell::Empty)
        };
        let text = |key: &str| cell(key).text();
        let kind = text("valueType");
        let value_cell = cell("value");
        let value = match kind.as_str() {
            "string" => match &value_cell {
                Cell::Text(v) => Some(serde_json::Value::String(v.clone())),
                Cell::Empty => Some(serde_json::Value::String(String::new())),
                _ => None,
            },
            "number" | "boolean" | "json" => serde_json::from_str(&value_cell.text()).ok(),
            _ => None,
        };
        let mut fields = serde_json::json!({"name":text("name"),"key":text("key"),"group":if indices.contains_key("group"){text("group")}else{"general".into()},"description":text("description"),"valueType":kind,"value":value.clone().unwrap_or(serde_json::Value::Null)});
        let parsed = serde_json::from_value::<CreateParameter>(fields.clone()).ok();
        let valid = value.is_some()
            && ["name", "key", "group", "description", "valueType"]
                .iter()
                .all(|key| matches!(cell(key), Cell::Text(_) | Cell::Empty))
            && parsed.as_ref().is_some_and(|p| {
                parameters::validate(
                    &p.name,
                    &p.key,
                    &p.group,
                    &p.description,
                    p.value_type,
                    &p.value,
                )
                .is_ok()
            });
        if !valid {
            fields["value"] = serde_json::Value::String(value_cell.text());
        }
        let values_json = serde_json::to_string(&fields).map_err(|_| invalid())?;
        total += values_json.len();
        if total > 8 * 1024 * 1024 {
            return Err(AppError::PayloadTooLarge);
        }
        let key = text("key");
        *counts.entry(key.clone()).or_default() += 1;
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
        });
    }
    for row in &mut output {
        if counts.get(&row.key).copied().unwrap_or(0) > 1 {
            row.status = "duplicate".into();
            row.error = Some("DUPLICATE_IN_FILE".into());
        }
    }
    Ok(output)
}

pub async fn create(
    database: &Database,
    actor: Actor,
    input: PreviewInput,
    fingerprint: String,
    rows: Vec<ImportRow>,
) -> Result<ImportView, AppError> {
    registry::resource(&input.resource, &actor, true)?;
    let id = ObjectId::new();
    let actor_id = actor.id;
    let request_id = input.request_id.clone();
    let result = transaction::run(
        database,
        (database.clone(), actor, input, fingerprint.clone(), rows),
        |session, (database, actor, input, fingerprint, rows)| {
            async move {
                let records = database.collection::<ImportRecord>("data_imports");
                if let Some(existing) = records
                    .find_one(doc! {"actorId":actor.id,"requestId":&input.request_id})
                    .session(&mut *session)
                    .await?
                {
                    if existing.fingerprint != *fingerprint {
                        return Err(fail(Failure::Conflict("请求标识已用于不同导入内容")));
                    }
                    return Ok(existing);
                }
                shared::lock(database, "data-tools", session).await?;
                let now = DateTime::now();
                if records
                    .count_documents(doc! {"actorId":actor.id,"expiresAt":{"$gt":now}})
                    .session(&mut *session)
                    .await?
                    >= 32
                {
                    return Err(fail(Failure::RateLimited));
                }
                let keys = rows
                    .iter()
                    .filter(|r| r.status == "ready")
                    .map(|r| r.key.clone())
                    .collect::<Vec<_>>();
                let mut cursor = database
                    .collection::<Document>("parameters")
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
                for row in rows.iter_mut() {
                    if row.status == "ready" && existing.contains(&row.key) {
                        row.status = "duplicate".into();
                        row.error = Some("KEY_EXISTS".into());
                    }
                }
                let record = ImportRecord {
                    id,
                    actor_id: actor.id,
                    request_id: input.request_id.clone(),
                    fingerprint: fingerprint.clone(),
                    resource: input.resource.clone(),
                    created_at: now,
                    expires_at: DateTime::from_millis(now.timestamp_millis() + 86_400_000),
                    rows: rows.clone(),
                };
                records.insert_one(&record).session(&mut *session).await?;
                shared::audit(database, actor, "dataTools.import.preview", id, session).await?;
                Ok(record)
            }
            .boxed()
        },
    )
    .await;
    // The owner/request unique index also protects simultaneous first submissions.
    let record = match result {
        Err(AppError::Conflict(_)) => {
            let existing = database
                .collection::<ImportRecord>("data_imports")
                .find_one(doc! {"actorId":actor_id,"requestId":&request_id})
                .await?;
            match existing {
                Some(existing) if existing.fingerprint == fingerprint => existing,
                _ => return Err(AppError::Conflict("请求标识已用于不同导入内容".into())),
            }
        }
        other => other?,
    };
    record.view()
}
pub async fn get(
    database: &Database,
    actor: &Actor,
    id: ObjectId,
) -> Result<ImportRecord, AppError> {
    let record = database
        .collection::<ImportRecord>("data_imports")
        .find_one(doc! {"_id":id,"actorId":actor.id})
        .await?
        .ok_or(AppError::NotFound)?;
    registry::resource(&record.resource, actor, false)?;
    Ok(record)
}
pub async fn list(database: &Database, actor: &Actor) -> Result<Vec<serde_json::Value>, AppError> {
    if !actor.can("parameters.read") {
        return Ok(Vec::new());
    }
    let records = database
        .collection::<Document>("data_imports")
        .find(doc! {"actorId":actor.id})
        .projection(doc! {"requestId":1,"resource":1,"createdAt":1,"expiresAt":1})
        .sort(doc! {"createdAt":-1,"_id":-1})
        .limit(20)
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    records.into_iter().map(|d|Ok(serde_json::json!({"id":d.get_object_id("_id").map_err(|_|AppError::Internal)?.to_hex(),"requestId":d.get_str("requestId").map_err(|_|AppError::Internal)?,"resource":d.get_str("resource").map_err(|_|AppError::Internal)?,"expiresAt":d.get_datetime("expiresAt").map_err(|_|AppError::Internal)?.timestamp_millis()}))).collect()
}
pub async fn apply(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: ApplyInput,
) -> Result<ImportView, AppError> {
    let record = get(database, &actor, id).await?;
    registry::resource(&record.resource, &actor, true)?;
    if input.rows.is_empty()
        || input.rows.len() > 100
        || input.rows.iter().collect::<HashSet<_>>().len() != input.rows.len()
        || input.rows.iter().any(|i| *i >= record.rows.len())
    {
        return Err(invalid());
    }
    for index in input.rows {
        // A concurrent unique-key insert may abort the first transaction. Re-open a fresh
        // snapshot once to observe that key and persist a conflict without a second create.
        for retry in 0..2 {
            let target = ObjectId::new();
            let result = transaction::run(
                database,
                (database.clone(), actor.clone()),
                |session, (database, actor)| {
                    async move {
                        let record = database
                            .collection::<ImportRecord>("data_imports")
                            .find_one(doc! {"_id":id,"actorId":actor.id})
                            .session(&mut *session)
                            .await?
                            .ok_or_else(|| fail(Failure::NotFound))?;
                        let mut row = record
                            .rows
                            .get(index)
                            .cloned()
                            .ok_or_else(|| fail(Failure::Validation("导入行号无效")))?;
                        if row.status != "ready" {
                            return Ok(());
                        }
                        if record.expires_at <= DateTime::now() {
                            return Err(fail(Failure::Conflict("预览已过期，请重新导入")));
                        }
                        let parameter: CreateParameter = serde_json::from_str(&row.values_json)
                            .map_err(|_| fail(Failure::NotReady))?;
                        if database
                            .collection::<Document>("parameters")
                            .find_one(doc! {"key":&parameter.key,"deletedAt":null})
                            .session(&mut *session)
                            .await?
                            .is_some()
                        {
                            row.status = "conflict".into();
                            row.error = Some("KEY_EXISTS".into());
                        } else {
                            parameters::create_in(database, actor, &parameter, target, session)
                                .await?;
                            row.status = "applied".into();
                            row.target_id = Some(target.to_hex());
                            row.error = None;
                        }
                        let mut set = doc! {};
                        set.insert(
                            format!("rows.{index}"),
                            bson::serialize_to_bson(&row).map_err(|_| fail(Failure::NotReady))?,
                        );
                        database
                            .collection::<Document>("data_imports")
                            .update_one(doc! {"_id":id}, doc! {"$set":set})
                            .session(session)
                            .await?;
                        Ok(())
                    }
                    .boxed()
                },
            )
            .await;
            match result {
                Err(AppError::Conflict(_)) if retry == 0 && record.expires_at > DateTime::now() => {
                    continue;
                }
                other => {
                    other?;
                    break;
                }
            }
        }
    }
    get(database, &actor, id).await?.view()
}
