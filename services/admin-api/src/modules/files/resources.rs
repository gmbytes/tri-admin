use super::{
    cleanup,
    dto::{FileList, FilePatch},
    models::{FileRecord, FileView, Upload},
    storage::{BlobId, BlobSpec, BlobStore},
    validate,
};
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        pagination::Page,
        transaction::{self, Failure, fail},
        validation,
    },
    modules::rbac::shared::{self, Actor},
    state::AppState,
};
use axum::{
    body::Body,
    http::{HeaderValue, header},
    response::Response,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{Bson, DateTime, Document, doc, oid::ObjectId},
};
use tokio_util::sync::CancellationToken;

fn owns_scope(actor: &Actor, owner: ObjectId, user: Option<Document>) -> Result<bool, AppError> {
    if actor.scope == DataScope::All || actor.id == owner {
        return Ok(true);
    }
    let Some(user) = user else { return Ok(false) };
    let roles = user
        .get_array("roleIds")
        .map_err(|_| AppError::NotReady)?
        .iter()
        .filter_map(Bson::as_object_id)
        .collect::<Vec<_>>();
    Ok(actor.require_scope(owner, &roles).is_ok())
}

async fn permitted(database: &Database, actor: &Actor, owner: ObjectId) -> Result<bool, AppError> {
    if actor.scope == DataScope::All || actor.id == owner {
        return Ok(true);
    }
    let user = database
        .collection::<Document>("users")
        .find_one(doc! {
        "_id":owner,
        "deletedAt":null})
        .projection(doc! {
        "roleIds":1})
        .await?;
    owns_scope(actor, owner, user)
}

async fn require_tx(
    database: &Database,
    actor: &Actor,
    owner: ObjectId,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    if actor.scope == DataScope::All || actor.id == owner {
        return Ok(());
    }
    let user = database
        .collection::<Document>("users")
        .find_one(doc! {
        "_id":owner,
        "deletedAt":null})
        .projection(doc! {
        "roleIds":1})
        .session(session)
        .await?;
    if owns_scope(actor, owner, user).map_err(|_| fail(Failure::NotReady))? {
        Ok(())
    } else {
        Err(fail(Failure::NotFound))
    }
}

pub async fn get(database: &Database, actor: &Actor, id: ObjectId) -> Result<FileRecord, AppError> {
    let row = database
        .collection::<FileRecord>("files")
        .find_one(doc! {
        "_id":id,
        "deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?;
    if !permitted(database, actor, row.owner_id).await? {
        return Err(AppError::NotFound);
    }
    Ok(row)
}

pub async fn list(
    database: &Database,
    actor: &Actor,
    input: FileList,
) -> Result<Page<FileView>, AppError> {
    let page = input.page();
    let skip = page.validate()?;
    let sort = page.sort_document(&["createdAt", "name"])?;
    let mut filter = doc! {
    "deletedAt":null};
    if actor.scope == DataScope::Own {
        filter.insert("ownerId", actor.id);
    }
    if let Some(category) = input.category_id {
        filter.insert("categoryId", validation::object_id(&category)?);
    }
    if let Some(search) = page
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        filter.insert(
            "name",
            doc! {
            "$regex":validate::prefix(search)},
        );
    }
    if actor.scope != DataScope::Roles {
        let total = database
            .collection::<Document>("files")
            .count_documents(filter.clone())
            .max_time(std::time::Duration::from_secs(3))
            .await?;
        let data = database
            .collection::<FileRecord>("files")
            .find(filter)
            .max_time(std::time::Duration::from_secs(3))
            .sort(sort)
            .skip(skip)
            .limit(page.page_size as i64)
            .await?
            .try_collect::<Vec<_>>()
            .await?
            .into_iter()
            .map(FileRecord::view)
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Page::new(data, &page, total));
    }
    let pipeline = vec![
        doc! {
        "$match":filter},
        doc! {
        "$sort":sort},
        doc! {
        "$lookup":{
        "from":"users",
        "localField":"ownerId",
        "foreignField":"_id",
        "pipeline":[{
        "$match":{
        "deletedAt":null}
        }
        ,
        {
        "$project":{
        "roleIds":1}
        }
        ],
        "as":"__owner"}
        },
        doc! {
        "$match":{
        "$or":[{
        "ownerId":actor.id}
        ,
        {
        "__owner.roleIds":{
        "$in":&actor.scope_role_ids}
        }
        ]}
        },
        doc! {
        "$facet":{
        "rows":[{
        "$skip":skip as i64}
        ,
        {
        "$limit":page.page_size as i64}
        ,
        {
        "$unset":"__owner"}
        ],
        "count":[{
        "$count":"total"}
        ]}
        },
    ];
    let result = database
        .collection::<Document>("files")
        .aggregate(pipeline)
        .max_time(std::time::Duration::from_secs(3))
        .await?
        .try_next()
        .await?
        .ok_or(AppError::Internal)?;
    let data = result
        .get_array("rows")
        .map_err(|_| AppError::Internal)?
        .iter()
        .cloned()
        .map(|row| {
            mongodb::bson::deserialize_from_bson::<FileRecord>(row)
                .map_err(|_| AppError::NotReady)?
                .view()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let total = result
        .get_array("count")
        .map_err(|_| AppError::Internal)?
        .first()
        .and_then(Bson::as_document)
        .and_then(|row| {
            row.get_i64("total")
                .ok()
                .or_else(|| row.get_i32("total").ok().map(i64::from))
        })
        .unwrap_or(0) as u64;
    Ok(Page::new(data, &page, total))
}

pub async fn patch(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: FilePatch,
) -> Result<FileView, AppError> {
    validate::version(input.version)?;
    if let Some(name) = &input.name {
        validate::filename(name)?;
    }
    let category = input
        .category_id
        .as_ref()
        .map(|value| value.as_deref().map(validation::object_id).transpose())
        .transpose()?;
    transaction::run(
        database,
        (database.clone(), actor, input),
        |session, (database, actor, input)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                shared::lock(database, "files", session).await?;
                let mut row = database
                    .collection::<FileRecord>("files")
                    .find_one(doc! {
                    "_id":id,
                    "deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                require_tx(database, actor, row.owner_id, session).await?;
                if row.version != input.version {
                    return Err(fail(Failure::Conflict("文件已被修改，请刷新后重试")));
                }
                if let Some(Some(category)) = category
                    && database
                        .collection::<Document>("file_categories")
                        .find_one(doc! {
                        "_id":category,
                        "deletedAt":null})
                        .session(&mut *session)
                        .await?
                        .is_none()
                {
                    return Err(fail(Failure::Validation("所选分类不存在")));
                }
                if let Some(name) = &input.name {
                    row.name = name.clone();
                }
                if let Some(category) = category {
                    row.category_id = category;
                }
                row.version += 1;
                row.updated_at = DateTime::now();
                database
                    .collection::<Document>("files")
                    .update_one(
                        doc! {
                        "_id":id},
                        doc! {
                        "$set":{
                        "name":&row.name,
                        "categoryId":row.category_id,
                        "updatedAt":row.updated_at}
                        ,
                        "$inc":{
                        "version":1i64}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "files.update", id, session).await?;
                Ok(row)
            }
            .boxed()
        },
    )
    .await?
    .view()
}

pub async fn delete(
    state: &AppState,
    actor: Actor,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    validate::version(version)?;
    let database = state.require_database()?;
    let initiator = actor.id;
    let (owner, upload) = transaction::run(
        database,
        (database.clone(), actor),
        |session, (database, actor)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                shared::lock(database, "files", session).await?;
                let row = database
                    .collection::<FileRecord>("files")
                    .find_one(doc! {
                    "_id":id,
                    "deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                require_tx(database, actor, row.owner_id, session).await?;
                if row.version != version {
                    return Err(fail(Failure::Conflict("文件已被修改，请刷新后重试")));
                }
                let parent = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                    "_id":row.upload_id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))?;
                let now = DateTime::now();
                database
                    .collection::<Document>("files")
                    .update_one(
                        doc! {
                        "_id":id},
                        doc! {
                        "$set":{
                        "deletedAt":now,
                        "updatedAt":now}
                        ,
                        "$inc":{
                        "version":1i64}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                database
                    .collection::<Document>("file_attempts")
                    .update_one(
                        doc! {
                        "_id":row.publish_attempt_id,
                        "state":"published"},
                        doc! {
                        "$set":{
                        "state":"orphan",
                        "updatedAt":now}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                cleanup::schedule(database, &parent, session).await?;
                shared::audit(database, actor, "files.delete", id, session).await?;
                Ok((row.owner_id, row.upload_id))
            }
            .boxed()
        },
    )
    .await?;
    let _ = cleanup::run_as(state, owner, upload, initiator).await;
    Ok(())
}

pub async fn content(state: &AppState, actor: &Actor, id: ObjectId) -> Result<Response, AppError> {
    let row = get(state.require_database()?, actor, id).await?;
    let spec = BlobSpec::new(row.size as u64, &row.sha256).map_err(|_| AppError::NotReady)?;
    let stream = BlobStore::from_profile(
        state.require_database()?,
        state.config.clone(),
        row.storage_profile_id,
    )
    .await
    .map_err(|_| AppError::FileStorageUnavailable)?
    .read(
        BlobId::parse(&row.blob_id.to_hex()).map_err(|_| AppError::NotReady)?,
        spec,
        CancellationToken::new(),
    )
    .await
    .map_err(|_| AppError::FileStorageUnavailable)?;
    let reporter = crate::common::observability::stream_reporter(state.require_database()?);
    let stream = stream.inspect_err(move |error| {
        if let Some(reporter) = &reporter {
            reporter.record(
                if *error == super::storage::StorageError::IntegrityMismatch {
                    "FILE_CONTENT_INTEGRITY"
                } else {
                    "FILE_CONTENT_IO"
                },
            );
        }
    });
    let fallback = row
        .name
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() || "._-".contains(value) {
                value
            } else {
                '_'
            }
        })
        .collect::<String>();
    let encoded = url::form_urlencoded::byte_serialize(row.name.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    let disposition = format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}");
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&row.size.to_string()).map_err(|_| AppError::NotReady)?,
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).map_err(|_| AppError::NotReady)?,
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{}\"", row.sha256)).map_err(|_| AppError::NotReady)?,
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}
