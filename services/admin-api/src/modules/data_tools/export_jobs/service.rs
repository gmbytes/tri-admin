use super::super::{
    export_writer::MAX_EXPORT_ROWS,
    models::TemplateOrigin,
    query, registry,
    templates::{self, TemplateRecord},
};
use super::models::*;
use crate::{
    common::{
        canonical,
        error::AppError,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
        validation,
    },
    config::Config,
    modules::{
        rbac::shared::{self, Actor},
        settings::{
            models::{ServiceSettings, stable_id},
            service as settings,
        },
    },
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{self, DateTime, Document, doc, oid::ObjectId},
};

fn request_id(value: &str) -> Result<(), AppError> {
    if uuid::Uuid::parse_str(value)
        .ok()
        .is_none_or(|id| id.to_string() != value)
    {
        return Err(AppError::Validation("导出请求标识需为规范 UUID".into()));
    }
    Ok(())
}
fn validate_spec(spec: &ExportSpec) -> Result<(), AppError> {
    templates::validate_columns(&spec.resource, &spec.columns, false)?;
    query::plan(
        &spec.resource,
        &spec.search,
        &spec.filters,
        spec.sort.as_ref(),
    )?;
    Ok(())
}
fn validate(input: &CreateInput) -> Result<(String, Option<ObjectId>), AppError> {
    request_id(&input.request_id)?;
    if !(1..=MAX_EXPORT_ROWS).contains(&input.max_rows) {
        return Err(AppError::Validation(
            "异步导出行数上限需为 1–1000000".into(),
        ));
    }
    match &input.source {
        Source::Resource {
            resource,
            columns,
            search,
            filters,
            sort,
        } => validate_spec(&ExportSpec {
            resource: resource.clone(),
            columns: columns.clone(),
            search: search.clone(),
            filters: filters.clone(),
            sort: sort.clone(),
        })?,
        Source::Template { id, version } => {
            validation::object_id(id)?;
            if !(1..i64::MAX).contains(version) {
                return Err(AppError::Validation("模板版本无效".into()));
            }
        }
    }
    let storage = input
        .storage_profile_id
        .as_deref()
        .map(validation::object_id)
        .transpose()?;
    let serialized = canonical::json(input)?;
    if serialized.len() > 64 * 1024 {
        return Err(AppError::PayloadTooLarge);
    }
    Ok((canonical::checksum(serialized.as_bytes()), storage))
}
async fn resolve(
    database: &Database,
    actor: &Actor,
    source: &Source,
    session: &mut ClientSession,
) -> mongodb::error::Result<(ExportSpec, Option<TemplateOrigin>)> {
    let (spec, origin) = match source {
        Source::Resource {
            resource,
            columns,
            search,
            filters,
            sort,
        } => (
            ExportSpec {
                resource: resource.clone(),
                columns: columns.clone(),
                search: search.clone(),
                filters: filters.clone(),
                sort: sort.clone(),
            },
            None,
        ),
        Source::Template { id, version } => {
            let id =
                ObjectId::parse_str(id).map_err(|_| fail(Failure::Validation("模板标识无效")))?;
            let row = database
                .collection::<TemplateRecord>("export_templates")
                .find_one(doc! {"_id":id,"deletedAt":null})
                .session(&mut *session)
                .await?
                .ok_or_else(|| fail(Failure::NotFound))?;
            if row.version != *version || !row.config.enabled {
                return Err(fail(Failure::Conflict("模板版本已改变或已停用")));
            }
            let origin = TemplateOrigin {
                id: id.to_hex(),
                code: row.config.code,
                name: row.config.name,
                version: row.version,
            };
            (
                ExportSpec {
                    resource: row.config.resource,
                    columns: row.config.columns,
                    search: row.config.search,
                    filters: row.config.filters,
                    sort: row.config.sort,
                },
                Some(origin),
            )
        }
    };
    registry::resource(&spec.resource, actor, false).map_err(|_| fail(Failure::Forbidden))?;
    validate_spec(&spec).map_err(|_| fail(Failure::Validation("导出配置无效")))?;
    Ok((spec, origin))
}
pub async fn create(
    database: &Database,
    config: &Config,
    actor: Actor,
    input: CreateInput,
) -> Result<(bool, JobView), AppError> {
    let (fingerprint, storage) = validate(&input)?;
    let id = ObjectId::new();
    let (created, row) = transaction::run(
        database,
        (database.clone(), config.clone(), actor, input, fingerprint),
        |session, (database, config, actor, input, fingerprint)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                shared::lock(database, "settings", session).await?;
                shared::lock(database, "files", session).await?;
                shared::lock(database, "data-exports", session).await?;
                if let Some(row) = database
                    .collection::<JobRecord>("data_exports")
                    .find_one(doc! {"actorId":actor.id,"requestId":&input.request_id})
                    .session(&mut *session)
                    .await?
                {
                    registry::resource(&row.meta.resource, actor, false)
                        .map_err(|_| fail(Failure::Forbidden))?;
                    if row.fingerprint != *fingerprint {
                        return Err(fail(Failure::Conflict("请求标识已用于不同导出条件")));
                    }
                    return Ok((false, row));
                }
                let (spec, template) = resolve(database, actor, &input.source, session).await?;
                if database
                    .collection::<Document>("data_exports")
                    .count_documents(doc! {"state":{"$in":ACTIVE.to_vec()}})
                    .session(&mut *session)
                    .await?
                    >= 8
                    || database
                        .collection::<Document>("data_exports")
                        .count_documents(doc! {"actorId":actor.id,"state":{"$in":ACTIVE.to_vec()}})
                        .session(&mut *session)
                        .await?
                        >= 2
                {
                    return Err(fail(Failure::RateLimited));
                }
                let storage = match storage {
                    Some(id) => id,
                    None => database
                        .collection::<ServiceSettings>("service_settings")
                        .find_one(doc! {"_id":stable_id("service")})
                        .session(&mut *session)
                        .await?
                        .ok_or_else(|| fail(Failure::NotReady))?
                        .storage_profile_id
                        .ok_or_else(|| fail(Failure::Validation("请先设置默认文件存储")))?,
                };
                settings::binding(database, Some(storage), true, config, session).await?;
                let now = DateTime::now();
                let row = JobRecord {
                    meta: JobMeta {
                        id,
                        request_id: input.request_id.clone(),
                        resource: spec.resource.clone(),
                        format: input.format,
                        max_rows: input.max_rows,
                        template,
                        state: JobState::Queued,
                        row_count: 0,
                        output: None,
                        last_error: None,
                        cleanup_pending: false,
                        version: 1,
                        created_at: now,
                        updated_at: now,
                        started_at: None,
                        completed_at: None,
                        expires_at: None,
                    },
                    actor_id: actor.id,
                    fingerprint: fingerprint.clone(),
                    spec,
                    creation_scope: ScopeSnapshot {
                        scope: actor.scope,
                        scope_role_ids: actor.scope_role_ids.clone(),
                    },
                    storage_profile_id: storage,
                    storage_retained: true,
                    generation: 0,
                    lease_token: None,
                    lease_expires_at: None,
                    candidate_id: None,
                    next_run_at: now,
                };
                let document =
                    bson::serialize_to_document(&row).map_err(|_| fail(Failure::NotReady))?;
                database
                    .collection::<Document>("data_exports")
                    .insert_one(document)
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "dataTools.export.create", id, session).await?;
                Ok((true, row))
            }
            .boxed()
        },
    )
    .await?;
    Ok((created, row.meta.view(DateTime::now())?))
}
pub async fn get_record(
    database: &Database,
    actor: &Actor,
    id: ObjectId,
) -> Result<JobRecord, AppError> {
    let row = database
        .collection::<JobRecord>("data_exports")
        .find_one(doc! {"_id":id,"actorId":actor.id})
        .await?
        .ok_or(AppError::NotFound)?;
    registry::resource(&row.meta.resource, actor, false)?;
    Ok(row)
}
pub async fn by_request(
    database: &Database,
    actor: &Actor,
    request: &str,
) -> Result<JobView, AppError> {
    request_id(request)?;
    let row = database
        .collection::<JobRecord>("data_exports")
        .find_one(doc! {"actorId":actor.id,"requestId":request})
        .await?
        .ok_or(AppError::NotFound)?;
    registry::resource(&row.meta.resource, actor, false)?;
    row.meta.view(DateTime::now())
}
pub async fn list(
    database: &Database,
    actor: &Actor,
    input: ListInput,
) -> Result<Page<JobView>, AppError> {
    let page = PageInput {
        page: input.page,
        page_size: input.page_size,
        search: None,
        sort: None,
    };
    let skip = page.validate()?;
    let resources = match &input.resource {
        Some(resource) => vec![registry::resource(resource, actor, false)?.key],
        None => registry::RESOURCES
            .iter()
            .filter(|resource| actor.can(resource.read_permission))
            .map(|resource| resource.key)
            .collect(),
    };
    let now = DateTime::now();
    let mut filter = doc! {"actorId":actor.id,"resource":{"$in":resources}};
    if let Some(state) = input.state {
        match state {
            JobState::Completed => {
                filter.insert("state", "completed");
                filter.insert("expiresAt", doc! {"$gt":now});
            }
            JobState::Expired => {
                filter.insert(
                    "$or",
                    vec![
                        doc! {"state":"expired"},
                        doc! {"state":"completed","expiresAt":{"$lte":now}},
                    ],
                );
            }
            other => {
                filter.insert("state", other.as_str());
            }
        }
    }
    let limit = page.page_size as i64;
    let (total,rows)=transaction::run_snapshot(database,(database.clone(),filter),|session,(database,filter)|async move{
        let total=database.collection::<Document>("data_exports").count_documents(filter.clone()).session(&mut *session).await?;
        let mut cursor=database.collection::<JobMeta>("data_exports").find(filter.clone()).projection(doc!{"requestId":1,"resource":1,"format":1,"maxRows":1,"template":1,"state":1,"rowCount":1,"output":1,"lastError":1,"cleanupPending":1,"version":1,"createdAt":1,"updatedAt":1,"startedAt":1,"completedAt":1,"expiresAt":1}).sort(doc!{"createdAt":-1,"_id":-1}).skip(skip).limit(limit).session(&mut *session).await?;
        let rows=cursor.stream(session).try_collect::<Vec<_>>().await?;Ok((total,rows))
    }.boxed()).await?;
    Ok(Page::new(
        rows.into_iter()
            .map(|row| row.view(now))
            .collect::<Result<Vec<_>, _>>()?,
        &page,
        total,
    ))
}
pub async fn cancel(database: &Database, actor: Actor, id: ObjectId) -> Result<JobView, AppError> {
    let row = transaction::run(
        database,
        (database.clone(), actor),
        |session, (database, actor)| {
            async move {
                shared::lock(database, "data-exports", session).await?;
                let mut row = database
                    .collection::<JobRecord>("data_exports")
                    .find_one(doc! {"_id":id,"actorId":actor.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                registry::resource(&row.meta.resource, actor, false)
                    .map_err(|_| fail(Failure::Forbidden))?;
                if row.meta.state == JobState::Cancelled {
                    return Ok(row);
                }
                if matches!(row.meta.state, JobState::Completed | JobState::Expired) {
                    return Err(fail(Failure::Conflict("任务已完成，请查询结果")));
                }
                row.meta.state = JobState::Cancelled;
                row.generation += 1;
                row.meta.cleanup_pending = row.meta.cleanup_pending
                    || row.candidate_id.is_some()
                    || row.lease_token.is_some()
                    || row.meta.output.is_some();
                row.storage_retained = row.meta.cleanup_pending;
                row.lease_token = None;
                row.meta.last_error = None;
                row.meta.version += 1;
                row.meta.updated_at = DateTime::now();
                row.next_run_at = row.meta.updated_at;
                let document =
                    bson::serialize_to_document(&row).map_err(|_| fail(Failure::NotReady))?;
                database
                    .collection::<Document>("data_exports")
                    .replace_one(doc! {"_id":id}, document)
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "dataTools.export.cancel", id, session).await?;
                Ok(row)
            }
            .boxed()
        },
    )
    .await?;
    row.meta.view(DateTime::now())
}
