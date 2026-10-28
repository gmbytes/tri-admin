use super::{
    cleanup,
    dto::UploadCreate,
    models::{CHUNK_SIZE, Chunk, Upload, UploadView},
    validate,
};
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
        storage_profiles::{
            models::{ServiceSettings, stable_id},
            service as settings,
        },
    },
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};

pub async fn get_record(
    database: &Database,
    owner: ObjectId,
    id: ObjectId,
) -> Result<Upload, AppError> {
    database
        .collection::<Upload>("file_uploads")
        .find_one(doc! {
                "_id":id,
        "ownerId":owner})
        .await?
        .ok_or(AppError::NotFound)
}

pub async fn view(database: &Database, row: Upload) -> Result<UploadView, AppError> {
    let indices = database
        .collection::<Chunk>("file_chunks")
        .find(doc! {
        "uploadId":row.id,"state":"accepted"})
        .sort(doc! {
        "index":1})
        .limit(1281)
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(|row| row.index)
        .collect::<Vec<_>>();
    if indices.len() > row.total_chunks as usize {
        return Err(AppError::NotReady);
    }
    row.view(Some(indices))
}

pub async fn get(
    database: &Database,
    owner: ObjectId,
    id: ObjectId,
) -> Result<UploadView, AppError> {
    view(database, get_record(database, owner, id).await?).await
}

pub async fn list(
    database: &Database,
    actor: &Actor,
    input: PageInput,
) -> Result<Page<UploadView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "name"])?;
    let mut filter = doc! {
    "ownerId":actor.id};
    if let Some(search) = input
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
    let total = database
        .collection::<Document>("file_uploads")
        .count_documents(filter.clone())
        .await?;
    let rows = database
        .collection::<Upload>("file_uploads")
        .find(filter)
        .sort(sort)
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(|row| row.view(None))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page::new(rows, &input, total))
}

pub async fn create(
    database: &Database,
    config: &Config,
    actor: Actor,
    input: UploadCreate,
) -> Result<UploadView, AppError> {
    validate::filename(&input.name)?;
    let spec = validate::spec(input.size, &input.sha256)?;
    let media = input
        .media_type
        .parse::<mime::Mime>()
        .map_err(|_| AppError::Validation("文件媒体类型无效".into()))?;
    if media.type_().as_str().contains('*') || media.subtype().as_str().contains('*') {
        return Err(AppError::Validation("文件媒体类型必须是具体类型".into()));
    }
    let media = media.to_string();
    if media.len() > 127 {
        return Err(AppError::Validation("文件媒体类型过长".into()));
    }
    let request = uuid::Uuid::parse_str(&input.request_id)
        .map_err(|_| AppError::Validation("上传requestId需为UUID".into()))?
        .to_string();
    let category = input
        .category_id
        .as_deref()
        .map(validation::object_id)
        .transpose()?;
    let storage = input
        .storage_profile_id
        .as_deref()
        .map(validation::object_id)
        .transpose()?;
    let fingerprint = canonical::checksum(
        canonical::json(&serde_json::json!({
        "name":input.name,
        "mediaType":media,
        "size":spec.size(),
        "sha256":spec.sha256(),
        "categoryId":category.map(ObjectId::to_hex),
        "storageProfileId":storage.map(ObjectId::to_hex)}
        ))?
        .as_bytes(),
    );
    let id = ObjectId::new();
    let row = transaction::run(
        database,
        (
            database.clone(),
            config.clone(),
            actor,
            input,
            request,
            media,
            spec,
            fingerprint,
        ),
        |session, (database, config, actor, input, request, media, spec, fingerprint)| {
            async move {
                shared::lock(database, "file-storage", session).await?;
                shared::lock(database, "files", session).await?;
                if let Some(row) = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                    "ownerId":actor.id,
                    "requestId":&*request})
                    .session(&mut *session)
                    .await?
                {
                    if row.request_hash != *fingerprint {
                        return Err(fail(Failure::Conflict("上传requestId已用于不同请求")));
                    }
                    return Ok(row);
                }
                if database
                    .collection::<Document>("file_uploads")
                    .count_documents(doc! {
                    "ownerId":actor.id,
                    "state":{
                    "$nin":["cancelled",
                    "completed",
                    "failed"]}
                    })
                    .session(&mut *session)
                    .await?
                    >= 32
                {
                    return Err(fail(Failure::RateLimited));
                }
                if let Some(category) = category
                    && database
                        .collection::<Document>("file_categories")
                        .find_one(doc! {
                        "_id":category,
                        "deletedAt":null})
                        .session(&mut *session)
                        .await?
                        .is_none()
                {
                    return Err(fail(Failure::Validation("所选文件分类不存在")));
                }
                let storage = match storage {
                    Some(id) => id,
                    None => database
                        .collection::<ServiceSettings>("service_settings")
                        .find_one(doc! {
                        "_id":stable_id("service")})
                        .session(&mut *session)
                        .await?
                        .ok_or_else(|| fail(Failure::NotReady))?
                        .storage_profile_id
                        .ok_or_else(|| fail(Failure::Validation("请先设置默认文件存储")))?,
                };
                settings::binding(database, Some(storage), true, config, session).await?;
                let now = DateTime::now();
                database
                    .collection::<Document>("file_uploads")
                    .insert_one(doc! {
                    "_id":id,
                    "ownerId":actor.id,
                    "requestId":&*request,
                    "requestHash":&*fingerprint,
                    "name":&input.name,
                    "mediaType":&*media,
                    "size":spec.size()as i64,
                    "sha256":spec.sha256(),
                    "categoryId":category,
                    "storageProfileId":storage,
                    "state":"receiving",
                    "chunkSize":CHUNK_SIZE as i64,
                    "totalChunks":spec.size().div_ceil(CHUNK_SIZE)as i64,
                    "receivedCount":0i64,
                    "receivedBytes":0i64,
                    "attemptCount":0i64,
                    "cleanupPending":false,
                    "publishAttemptId":null,
                    "fileId":null,
                    "lastError":null,
                    "completedAt":null,
                    "version":1i64,
                    "createdAt":now,
                    "updatedAt":now})
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "uploads.create", id, session).await?;
                database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                    "_id":id})
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))
            }
            .boxed()
        },
    )
    .await?;
    view(database, row).await
}

pub async fn cancel(state: &AppState, actor: Actor, id: ObjectId) -> Result<UploadView, AppError> {
    let database = state.require_database()?;
    let owner = actor.id;
    transaction::run(
        database,
        (database.clone(), actor),
        |session, (database, actor)| {
            async move {
                shared::lock(database, "files", session).await?;
                let row = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                                        "_id":id,
                    "ownerId":actor.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if row.state == "cancelled" {
                    return Ok(());
                }
                if row.state=="queued"&&let Some(attempt)=row.publish_attempt_id {
                    let stopped=database.collection::<Document>("file_attempts").update_one(doc!{"_id":attempt,"state":"queued"},doc!{"$set":{"state":"cleaned","updatedAt":DateTime::now()}}).session(&mut *session).await?;
                    if stopped.matched_count==1{
                        database.collection::<Document>("file_uploads").update_one(doc!{"_id":id},doc!{"$set":{"state":"cancelled","updatedAt":DateTime::now()},"$inc":{"version":1i64}}).session(&mut *session).await?;
                        cleanup::schedule(database,&row,session).await?;shared::audit(database,actor,"uploads.cancel",id,session).await?;return Ok(())
                    }
                }
                if matches!(row.state.as_str(),"queued"|"verifying"|"publishing"|"reconciling"|"unknown") {
                    database.collection::<Document>("file_uploads").update_one(doc!{"_id":id},doc!{"$set":{"state":"cancelRequested","cleanupPending":true,"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).session(&mut *session).await?;
                    if let Some(attempt)=row.publish_attempt_id{database.collection::<Document>("file_attempts").update_one(doc!{"_id":attempt,"state":"unknown"},doc!{"$set":{"nextRunAt":DateTime::now()}}).session(&mut *session).await?;}
                    shared::audit(database,actor,"uploads.cancel",id,session).await?;return Ok(())
                }
                if row.state=="cancelRequested"{return Ok(())}
                if !matches!(row.state.as_str(),"receiving"|"failed") {
                    return Err(fail(Failure::Conflict(
                        "上传已结束或正在发布，请查询当前状态",
                    )));
                }
                database
                    .collection::<Document>("file_uploads")
                    .update_one(
                        doc! {
                        "_id":id},
                        doc! {
                        "$set":{
                        "state":"cancelled",
                        "updatedAt":DateTime::now(),
                        "cleanupPending":row.attempt_count>0}
                        ,
                        "$inc":{
                        "version":1i64}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                if row.attempt_count > 0 {
                    cleanup::schedule(database, &row, session).await?;
                }
                shared::audit(database, actor, "uploads.cancel", id, session).await
            }
            .boxed()
        },
    )
    .await?;
    let _ = cleanup::run(state, owner, id).await;
    let _ = super::jobs::recheck(state, owner, id).await;
    get(database, owner, id).await
}
