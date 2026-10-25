use super::{
    dto::{CreateParameter, PatchParameter, ValueType},
    models::{ParameterRecord, ParameterView},
};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
        validation,
    },
    modules::rbac::shared::{self, Actor},
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use serde_json::Value;

fn depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}
fn safe_numbers(value: &Value) -> bool {
    match value {
        Value::Number(number) => number
            .as_f64()
            .is_some_and(|number| number.is_finite() && number.abs() <= 9_007_199_254_740_991.0),
        Value::Array(values) => values.iter().all(safe_numbers),
        Value::Object(values) => values.values().all(safe_numbers),
        _ => true,
    }
}
fn validate(
    name: &str,
    key: &str,
    group: &str,
    description: &str,
    kind: ValueType,
    value: &Value,
) -> Result<String, AppError> {
    validation::text(name, "参数名称", 80)?;
    validation::text(group, "分组", 64)?;
    if key.is_empty()
        || key.len() > 128
        || !key.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        || !key
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
    {
        return Err(AppError::Validation("参数标识格式无效".into()));
    }
    if description.chars().count() > 1000 {
        return Err(AppError::Validation("描述最多 1000 个字符".into()));
    }
    let matches = match kind {
        ValueType::String => value.is_string(),
        ValueType::Number => value
            .as_f64()
            .is_some_and(|number| number.is_finite() && number.abs() <= 9_007_199_254_740_991.0),
        ValueType::Boolean => value.is_boolean(),
        ValueType::Json => true,
    };
    if !matches {
        return Err(AppError::Validation(
            "参数值与所选类型不匹配或数字超出安全范围".into(),
        ));
    }
    if depth(value) > 16 {
        return Err(AppError::Validation("JSON 嵌套最多 16 层".into()));
    }
    if !safe_numbers(value) {
        return Err(AppError::Validation(
            "JSON 中的数字超出桌面安全数值范围".into(),
        ));
    }
    let serialized = serde_json::to_string(value)
        .map_err(|_| AppError::Validation("参数值无法编码为 JSON".into()))?;
    if serialized.len() > 16384 {
        return Err(AppError::Validation("参数值最多 16 KiB".into()));
    }
    Ok(serialized)
}
fn version(value: i64) -> Result<(), AppError> {
    if !(1..i64::MAX).contains(&value) {
        return Err(AppError::Validation("版本号无效".into()));
    }
    Ok(())
}
pub async fn list(database: &Database, input: PageInput) -> Result<Page<ParameterView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "key", "name", "group"])?;
    let mut filter = doc! {"deletedAt":null};
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let mut prefix = "^".to_owned();
        for character in search.chars() {
            if ".+*?^${}()|[]\\".contains(character) {
                prefix.push('\\');
            }
            prefix.push(character);
        }
        filter.insert("key", doc! {"$regex":prefix});
    }
    let total = database
        .collection::<Document>("parameters")
        .count_documents(filter.clone())
        .await?;
    let data = database
        .collection::<ParameterRecord>("parameters")
        .find(filter)
        .sort(sort)
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(ParameterRecord::view)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page::new(data, &input, total))
}
pub async fn get(database: &Database, id: ObjectId) -> Result<ParameterView, AppError> {
    database
        .collection::<ParameterRecord>("parameters")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?
        .view()
}
pub async fn create(
    database: &Database,
    actor: Actor,
    input: CreateParameter,
) -> Result<ParameterView, AppError> {
    let serialized = validate(
        &input.name,
        &input.key,
        &input.group,
        &input.description,
        input.value_type,
        &input.value,
    )?;
    let id = ObjectId::new();
    transaction::run(
        database,
        (database.clone(), actor, input, serialized),
        |session, (database, actor, input, serialized)| {
            async move {
                let now = DateTime::now();
                let record = doc! {
                    "_id":id, "name":&input.name, "key":&input.key,
                    "group":&input.group, "description":&input.description,
                    "valueType":input.value_type.as_str(), "valueJson":&*serialized,
                    "version":1i64, "createdAt":now, "updatedAt":now, "deletedAt":null
                };
                database
                    .collection::<Document>("parameters")
                    .insert_one(record)
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "parameters.create", id, session).await?;
                database
                    .collection::<ParameterRecord>("parameters")
                    .find_one(doc! {"_id":id})
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn patch(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: PatchParameter,
) -> Result<ParameterView, AppError> {
    version(input.version)?;
    transaction::run(
        database,
        (database.clone(), actor, input),
        |session, (database, actor, input)| {
            async move {
                let mut record = database
                    .collection::<ParameterRecord>("parameters")
                    .find_one(doc! {"_id":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if record.version != input.version {
                    return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));
                }
                if let Some(name) = &input.name {
                    record.name = name.clone();
                }
                if let Some(key) = &input.key {
                    record.key = key.clone();
                }
                if let Some(group) = &input.group {
                    record.group = group.clone();
                }
                if let Some(description) = &input.description {
                    record.description = description.clone();
                }
                if let Some(kind) = input.value_type {
                    record.value_type = kind;
                }
                let value = match &input.value {
                    Some(value) => value.clone().unwrap_or(Value::Null),
                    None => serde_json::from_str(&record.value_json)
                        .map_err(|_| fail(Failure::Validation("已存参数值无效")))?,
                };
                record.value_json = validate(
                    &record.name,
                    &record.key,
                    &record.group,
                    &record.description,
                    record.value_type,
                    &value,
                )
                .map_err(|_| fail(Failure::Validation("参数字段、值类型或大小无效")))?;
                record.version += 1;
                record.updated_at = DateTime::now();
                let fields = doc! {
                    "name":&record.name, "key":&record.key, "group":&record.group,
                    "description":&record.description, "valueType":record.value_type.as_str(),
                    "valueJson":&record.value_json, "version":record.version,
                    "updatedAt":record.updated_at
                };
                database
                    .collection::<Document>("parameters")
                    .update_one(
                        doc! {"_id":id,"version":input.version,"deletedAt":null},
                        doc! {"$set":fields},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "parameters.update", id, session).await?;
                Ok(record)
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn delete(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    expected: i64,
) -> Result<(), AppError> {
    version(expected)?;
    transaction::run(
        database,
        (database.clone(), actor),
        |session, (database, actor)| {
            async move {
                let record = database
                    .collection::<ParameterRecord>("parameters")
                    .find_one(doc! {"_id":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if record.version != expected {
                    return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));
                }
                let now = DateTime::now();
                database
                    .collection::<Document>("parameters")
                    .update_one(
                        doc! {"_id":id,"version":expected,"deletedAt":null},
                        doc! {"$set":{"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "parameters.delete", id, session).await
            }
            .boxed()
        },
    )
    .await
}
