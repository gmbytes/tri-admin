use super::{
    formats,
    models::public_time,
    query::{self, DataFilter, DataSort},
    registry,
};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        request::{optional_non_null, optional_nullable},
        transaction::{self, Failure, fail},
        validation,
    },
    modules::rbac::shared::{self, Actor},
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{self, DateTime, Document, doc, oid::ObjectId},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
fn yes() -> bool {
    true
}
fn limit() -> u32 {
    2000
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Column {
    pub field: String,
    pub header: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateInput {
    pub name: String,
    pub code: String,
    pub resource: String,
    pub columns: Vec<Column>,
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub filters: Vec<DataFilter>,
    pub sort: Option<DataSort>,
    #[serde(default = "limit")]
    pub max_rows: u32,
    #[serde(default)]
    pub allow_import: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplatePatch {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub columns: Option<Vec<Column>>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub search: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub filters: Option<Vec<DataFilter>>,
    #[serde(default, deserialize_with = "optional_nullable")]
    pub sort: Option<Option<DataSort>>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub max_rows: Option<u32>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub allow_import: Option<bool>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub enabled: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyInput {
    pub version: i64,
    pub name: String,
    pub code: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    #[serde(flatten)]
    pub config: TemplateInput,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub deleted_at: Option<DateTime>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateView {
    pub id: String,
    #[serde(flatten)]
    pub config: TemplateInput,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}
impl TemplateRecord {
    pub fn view(self) -> Result<TemplateView, AppError> {
        Ok(TemplateView {
            id: self.id.to_hex(),
            config: self.config,
            version: self.version,
            created_at: public_time(self.created_at)?,
            updated_at: public_time(self.updated_at)?,
        })
    }
}
pub fn validate(input: &TemplateInput) -> Result<(), AppError> {
    validation::text(&input.name, "模板名称", 80)?;
    if input.code.is_empty()
        || input.code.len() > 64
        || !input.code.as_bytes()[0].is_ascii_lowercase()
        || !input
            .code
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
    {
        return Err(AppError::Validation("模板标识格式无效".into()));
    }
    let definition = registry::definition(&input.resource)?;
    if input.columns.is_empty()
        || input.columns.len() > 32
        || !(1..=formats::MAX_ROWS as u32).contains(&input.max_rows)
    {
        return Err(AppError::Validation("模板列或同步行数上限无效".into()));
    }
    let fields = input
        .columns
        .iter()
        .map(|c| c.field.as_str())
        .collect::<HashSet<_>>();
    let headers = input
        .columns
        .iter()
        .map(|c| c.header.as_str())
        .collect::<HashSet<_>>();
    if fields.len() != input.columns.len()
        || headers.len() != input.columns.len()
        || input.columns.iter().any(|column| {
            !definition.fields.contains(&column.field.as_str())
                || column.header.is_empty()
                || column.header.len() > 128
                || column.header.trim() != column.header
                || column.header.chars().any(char::is_control)
                || column.header.starts_with(['=', '+', '-', '@'])
        })
    {
        return Err(AppError::Validation(
            "模板字段或表头无效、重复或含公式前缀".into(),
        ));
    }
    if input.allow_import
        && (!definition.importable
            || definition
                .required_fields
                .iter()
                .any(|field| !fields.contains(field)))
    {
        return Err(AppError::Validation(
            "导入模板缺少必填字段或资源不支持导入".into(),
        ));
    }
    query::plan(
        &input.resource,
        &input.search,
        &input.filters,
        input.sort.as_ref(),
    )?;
    if serde_json::to_vec(input)
        .map_err(|_| AppError::Internal)?
        .len()
        > 64 * 1024
    {
        return Err(AppError::PayloadTooLarge);
    }
    Ok(())
}
fn version(value: i64) -> Result<(), AppError> {
    if !(1..i64::MAX).contains(&value) {
        return Err(AppError::Validation("模板版本无效".into()));
    }
    Ok(())
}
pub async fn get(
    database: &Database,
    actor: &Actor,
    id: ObjectId,
) -> Result<TemplateRecord, AppError> {
    let record = database
        .collection::<TemplateRecord>("export_templates")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?;
    registry::resource(&record.config.resource, actor, false)?;
    Ok(record)
}
pub async fn list(
    database: &Database,
    actor: &Actor,
    input: PageInput,
    resource: Option<String>,
) -> Result<Page<TemplateView>, AppError> {
    let skip = input.validate()?;
    if input
        .search
        .as_ref()
        .is_some_and(|value| value.contains('\0'))
    {
        return Err(AppError::Validation("模板搜索前缀无效".into()));
    }
    let sort = input.sort_document(&["createdAt", "name", "code"])?;
    let resources = if let Some(key) = resource {
        vec![registry::resource(&key, actor, false)?.key]
    } else {
        registry::RESOURCES
            .iter()
            .filter(|r| actor.can(r.read_permission))
            .map(|r| r.key)
            .collect()
    };
    let mut filter = doc! {"deletedAt":null,"resource":{"$in":resources}};
    if let Some(search) = &input.search
        && !search.trim().is_empty()
    {
        filter.insert("code", doc! {"$regex":query::prefix(search.trim())});
    }
    let total = database
        .collection::<Document>("export_templates")
        .count_documents(filter.clone())
        .await?;
    let rows = database
        .collection::<TemplateRecord>("export_templates")
        .find(filter)
        .sort(sort)
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(TemplateRecord::view)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page::new(rows, &input, total))
}
pub async fn create(
    database: &Database,
    actor: Actor,
    input: TemplateInput,
) -> Result<TemplateView, AppError> {
    registry::resource(&input.resource, &actor, false)?;
    validate(&input)?;
    let id = ObjectId::new();
    transaction::run(
        database,
        (database.clone(), actor, input),
        |session, (database, actor, input)| {
            async move {
                let now = DateTime::now();
                let record = TemplateRecord {
                    id,
                    config: input.clone(),
                    version: 1,
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                };
                let document =
                    bson::serialize_to_document(&record).map_err(|_| fail(Failure::NotReady))?;
                database
                    .collection::<Document>("export_templates")
                    .insert_one(document)
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "exportTemplates.create", id, session).await?;
                Ok(record)
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
    input: TemplatePatch,
) -> Result<TemplateView, AppError> {
    version(input.version)?;
    transaction::run(
        database,
        (database.clone(), actor, input),
        |session, (database, actor, input)| {
            async move {
                let mut record = database
                    .collection::<TemplateRecord>("export_templates")
                    .find_one(doc! {"_id":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                registry::resource(&record.config.resource, actor, false)
                    .map_err(|_| fail(Failure::Forbidden))?;
                if record.version != input.version {
                    return Err(fail(Failure::Conflict("模板已改变，请刷新后重试")));
                }
                if let Some(v) = &input.name {
                    record.config.name = v.clone()
                }
                if let Some(v) = &input.columns {
                    record.config.columns = v.clone()
                }
                if let Some(v) = &input.search {
                    record.config.search = v.clone()
                }
                if let Some(v) = &input.filters {
                    record.config.filters = v.clone()
                }
                if let Some(v) = &input.sort {
                    record.config.sort = v.clone()
                }
                if let Some(v) = input.max_rows {
                    record.config.max_rows = v
                }
                if let Some(v) = input.allow_import {
                    record.config.allow_import = v
                }
                if let Some(v) = input.enabled {
                    record.config.enabled = v
                }
                validate(&record.config)
                    .map_err(|_| fail(Failure::Validation("模板字段、条件或导入规则无效")))?;
                record.version += 1;
                record.updated_at = DateTime::now();
                let mut fields = bson::serialize_to_document(&record.config)
                    .map_err(|_| fail(Failure::NotReady))?;
                fields.insert("version", record.version);
                fields.insert("updatedAt", record.updated_at);
                database
                    .collection::<Document>("export_templates")
                    .update_one(
                        doc! {"_id":id,"version":input.version,"deletedAt":null},
                        doc! {"$set":fields},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "exportTemplates.update", id, session).await?;
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
                    .collection::<TemplateRecord>("export_templates")
                    .find_one(doc! {"_id":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                registry::resource(&record.config.resource, actor, false)
                    .map_err(|_| fail(Failure::Forbidden))?;
                if record.version != expected {
                    return Err(fail(Failure::Conflict("模板已改变，请刷新后重试")));
                }
                let now = DateTime::now();
                database
                    .collection::<Document>("export_templates")
                    .update_one(
                        doc! {"_id":id,"version":expected,"deletedAt":null},
                        doc! {"$set":{"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "exportTemplates.delete", id, session).await
            }
            .boxed()
        },
    )
    .await
}
pub async fn copy(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: CopyInput,
) -> Result<TemplateView, AppError> {
    version(input.version)?;
    let new_id = ObjectId::new();
    transaction::run(
        database,
        (database.clone(), actor, input),
        |session, (database, actor, input)| {
            async move {
                let source = database
                    .collection::<TemplateRecord>("export_templates")
                    .find_one(doc! {"_id":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                registry::resource(&source.config.resource, actor, false)
                    .map_err(|_| fail(Failure::Forbidden))?;
                if source.version != input.version {
                    return Err(fail(Failure::Conflict("模板已改变，请刷新后重试")));
                }
                let mut config = source.config;
                config.name = input.name.clone();
                config.code = input.code.clone();
                validate(&config)
                    .map_err(|_| fail(Failure::Validation("复制模板的名称或标识无效")))?;
                let now = DateTime::now();
                let record = TemplateRecord {
                    id: new_id,
                    config,
                    version: 1,
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                };
                let document =
                    bson::serialize_to_document(&record).map_err(|_| fail(Failure::NotReady))?;
                database
                    .collection::<Document>("export_templates")
                    .insert_one(document)
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "exportTemplates.copy", new_id, session).await?;
                Ok(record)
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn active(
    database: &Database,
    actor: &Actor,
    id: ObjectId,
    expected: i64,
    import: bool,
) -> Result<TemplateRecord, AppError> {
    version(expected)?;
    let record = get(database, actor, id).await?;
    if record.version != expected {
        return Err(AppError::Conflict("模板已改变，请刷新后重试".into()));
    }
    if !record.config.enabled || import && !record.config.allow_import {
        return Err(AppError::Conflict("模板已停用或不支持导入".into()));
    }
    if import {
        registry::resource(&record.config.resource, actor, true)?;
    }
    validate(&record.config)?;
    Ok(record)
}
